// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Read-only S3/DynamoDB structural DOM audit. See scripts/document-dom-scanner.md.

mod args;
mod cloud;
mod inspect;
mod worker;

use args::Args;
use cloud::{BlobIdentity, Cloud, Item, metadata_identity};
use inspect::{Check, Violation};
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    io::{self, Write},
    process::ExitCode,
};

#[derive(Serialize)]
struct Transition {
    clock: String,
    user_id: Option<String>,
    client_version: Option<String>,
    created_at: Option<String>,
    payload_sha256: String,
    s3_key: Option<String>,
    violations: Vec<Violation>,
    attribution: &'static str,
}

#[derive(Serialize)]
struct DocumentReport {
    kind: &'static str,
    doc_id: String,
    status: &'static str,
    doc_type: Option<String>,
    deleted: bool,
    snapshot_version: Option<u64>,
    snapshot_blob: Option<BlobIdentity>,
    snapshot: Option<Check>,
    current: Option<Check>,
    payload_bytes: usize,
    updates_read: usize,
    updates_applied: usize,
    update_fingerprint: Option<String>,
    first_invalid_update: Option<Transition>,
    invalid_replay_observations: usize,
    last_valid_update_clock: Option<String>,
    read_stable: bool,
    error: Option<String>,
}

#[derive(Default, Serialize)]
struct Summary {
    kind: &'static str,
    documents: usize,
    valid_current: usize,
    invalid_current: usize,
    invalid_snapshots: usize,
    historical_snapshots: usize,
    invalid_history: usize,
    intermediate_invalid: usize,
    skipped: usize,
    incomplete: usize,
    errors: usize,
    inventory_complete: bool,
    exit_code: u8,
}

fn emit(value: &impl Serialize) -> Result<(), String> {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    serde_json::to_writer(&mut out, value).map_err(|_| "cannot write JSON report".to_string())?;
    writeln!(out)
        .and_then(|_| out.flush())
        .map_err(|_| "cannot write report".into())
}

fn string(item: &Item, key: &str) -> Option<String> {
    item.get(key).and_then(|v| v.as_s().ok()).cloned()
}

fn error(report: &mut DocumentReport, message: String) {
    report.status = "error";
    report.error = Some(message);
}

async fn inspect_document(cloud: &Cloud, args: &Args, id: &str) -> DocumentReport {
    let mut report = DocumentReport {
        kind: "document",
        doc_id: id.into(),
        status: "error",
        doc_type: None,
        deleted: false,
        snapshot_version: None,
        snapshot_blob: None,
        snapshot: None,
        current: None,
        payload_bytes: 0,
        updates_read: 0,
        updates_applied: 0,
        update_fingerprint: None,
        first_invalid_update: None,
        invalid_replay_observations: 0,
        last_valid_update_clock: None,
        read_stable: false,
        error: None,
    };
    if let Err(message) = inspect_current(cloud, args, id, &mut report).await {
        error(&mut report, message);
    }
    report
}

async fn inspect_current(
    cloud: &Cloud,
    args: &Args,
    id: &str,
    report: &mut DocumentReport,
) -> Result<(), String> {
    let meta = cloud
        .metadata(id)
        .await?
        .ok_or("document metadata missing")?;
    if string(&meta, "doc_id").as_deref() != Some(id) {
        return Err("metadata doc_id does not match its key".into());
    }
    let before = metadata_identity(&meta);
    let kind = string(&meta, "doc_type").ok_or("metadata doc_type missing or malformed")?;
    report.doc_type = Some(kind.clone());
    report.deleted = match meta.get("is_deleted") {
        None => false, // Legacy metadata defaults to active, as in DocRepo.
        Some(value) => *value
            .as_bool()
            .map_err(|_| "metadata is_deleted malformed")?,
    };
    match kind.as_str() {
        "document" | "presentation" => {}
        "spreadsheet" | "chat" => {
            report.status = "skipped";
            return Ok(());
        }
        _ => return Err("unknown document type; cannot choose a DOM schema".into()),
    }
    if report.deleted && !args.include_deleted {
        report.status = "skipped";
        return Ok(());
    }
    report.snapshot_version = Some(
        meta.get("snapshot_version")
            .and_then(|v| v.as_n().ok())
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or("metadata snapshot_version missing or malformed")?,
    );
    let key = string(&meta, "snapshot_s3_key")
        .filter(|s| !s.is_empty())
        .ok_or("metadata snapshot pointer missing or malformed")?;
    let snapshot = cloud.blob(&key).await?;
    report.snapshot_blob = Some(snapshot.identity.clone());
    report.payload_bytes = snapshot.bytes.len();
    let updates = cloud
        .updates(id, args.max_bytes.saturating_sub(snapshot.bytes.len()))
        .await?;
    report.payload_bytes += updates.bytes;
    report.updates_read = updates.rows.len();
    report.update_fingerprint = Some(updates.fingerprint.clone());
    let checks = worker::inspect(
        &snapshot.bytes,
        &updates.rows,
        args.max_nodes,
        args.max_depth,
        args.max_memory_bytes,
        std::time::Duration::from_secs(args.timeout_seconds as u64),
    )
    .await?;
    report.updates_applied = checks.updates_applied;
    report.invalid_replay_observations = checks.invalid_replay_observations;
    if let Some(index) = checks.last_valid_index {
        report.last_valid_update_clock = Some(updates.rows[index].clock.clone());
    }
    if let Some(index) = checks.first_invalid_index {
        let row = &updates.rows[index];
        report.first_invalid_update = Some(Transition {
            clock: row.clock.clone(),
            user_id: row.user_id.clone(),
            client_version: row.client_version.clone(),
            created_at: row.created_at.clone(),
            payload_sha256: format!("{:x}", Sha256::digest(&row.bytes)),
            s3_key: row.blob.as_ref().map(|b| b.key.clone()),
            violations: checks.first_invalid_violations,
            attribution: "first invalid retained replay observation; not proof of originating author",
        });
    }
    report.status = match checks.current.status.as_str() {
        "valid" => "valid",
        "invalid" => "invalid",
        "inconclusive" => "inconclusive",
        _ => "error",
    };
    report.snapshot = Some(checks.snapshot);
    report.current = Some(checks.current);
    let mut blob_stable = cloud.unchanged_blob(&snapshot.identity).await?;
    for row in &updates.rows {
        if let Some(identity) = &row.blob {
            blob_stable &= cloud.unchanged_blob(identity).await?;
        }
    }
    let tail_stable = updates.fingerprint == cloud.update_identity(id).await?;
    let after = cloud
        .metadata(id)
        .await?
        .ok_or("metadata disappeared during read")?;
    report.read_stable = blob_stable && tail_stable && before == metadata_identity(&after);
    if !report.read_stable {
        report.status = "inconclusive";
        report.error = Some(
            "metadata, operation log, or S3 object changed during read; rescan after quiescence"
                .into(),
        );
    }
    Ok(())
}

fn count_document(summary: &mut Summary, report: &DocumentReport) {
    summary.documents += 1;
    match report.status {
        "valid" => summary.valid_current += 1,
        "invalid" => summary.invalid_current += 1,
        "skipped" => summary.skipped += 1,
        "error" => summary.errors += 1,
        _ => {}
    }
    if report.status != "skipped" && report.status != "error" {
        if report.status == "inconclusive" || report.current.as_ref().is_some_and(|c| !c.complete) {
            summary.incomplete += 1;
        }
        if report
            .snapshot
            .as_ref()
            .is_some_and(|c| c.status == "invalid")
        {
            summary.invalid_snapshots += 1;
        }
        if report.invalid_replay_observations > 0 {
            summary.intermediate_invalid += 1;
        }
    }
}

async fn scan_history(
    cloud: &Cloud,
    args: &Args,
    report: &DocumentReport,
    summary: &mut Summary,
) -> Result<(), String> {
    if report.status == "skipped" {
        return Ok(());
    }
    let records = match cloud.history(&report.doc_id).await {
        Ok(r) => r,
        Err(e) => {
            summary.errors += 1;
            emit(&json!({"kind":"history","doc_id":report.doc_id,"status":"error","error":e}))?;
            return Ok(());
        }
    };
    // History has one aggregate cap, independent of current replay so a
    // failed current load still permits investigating older retained states.
    let mut budget = args.max_bytes;
    let mut remaining_nodes = args.max_nodes;
    for record in records {
        if report
            .snapshot_blob
            .as_ref()
            .is_some_and(|b| b.key == record.key)
        {
            continue;
        }
        summary.historical_snapshots += 1;
        if budget == 0 || remaining_nodes == 0 {
            summary.incomplete += 1;
            emit(
                &json!({"kind":"history","doc_id":report.doc_id,"status":"inconclusive","error":"history budget exhausted; remaining snapshots not inspected"}),
            )?;
            break;
        }
        let result = match cloud.blob_with_limit(&record.key, budget).await {
            Ok(blob) => {
                budget -= blob.bytes.len();
                let check = match worker::inspect(
                    &blob.bytes,
                    &[],
                    remaining_nodes,
                    args.max_depth,
                    args.max_memory_bytes,
                    std::time::Duration::from_secs(args.timeout_seconds as u64),
                )
                .await
                {
                    Ok(checks) => checks.snapshot,
                    Err(e) => Check::error(&e),
                };
                remaining_nodes = remaining_nodes.saturating_sub(check.nodes_visited);
                let (stable, stability_error) = match cloud.unchanged_blob(&blob.identity).await {
                    Ok(stable) => (stable, None),
                    Err(e) => (false, Some(e)),
                };
                let status = if stability_error.is_some() {
                    "error"
                } else if !stable {
                    "inconclusive"
                } else {
                    check.status.as_str()
                };
                match status {
                    "invalid" => summary.invalid_history += 1,
                    "valid" => {}
                    "error" => summary.errors += 1,
                    _ => {}
                }
                if status == "inconclusive" || !check.complete {
                    summary.incomplete += 1;
                }
                json!({"kind":"history","doc_id":report.doc_id,"status":status,"snapshot":record,"blob":blob.identity,"check":check,"read_stable":stable,"stability_error":stability_error})
            }
            Err(e) => {
                // Failed streams may have consumed an unknown prefix. Spend
                // the remaining allowance conservatively before another GET.
                budget = 0;
                summary.errors += 1;
                json!({"kind":"history","doc_id":report.doc_id,"status":"error","snapshot":record,"error":e})
            }
        };
        emit(&result)?;
    }
    Ok(())
}

fn endpoint(sdk: &aws_types::SdkConfig, service_id: &str) -> Option<String> {
    // Match the generated service Builder::from(&SdkConfig), using the SDK's
    // own environment/profile loader rather than a second endpoint parser.
    // The shared loader's ignore-configured-endpoints setting is not an
    // independent filter here: service builders still consult service_config.
    if sdk.get_origin("endpoint_url").is_client_config() {
        return sdk.endpoint_url().map(str::to_owned);
    }
    sdk.service_config()
        .and_then(|config| {
            config.load_config(
                aws_types::service_config::ServiceConfigKey::builder()
                    .service_id(service_id)
                    .env("AWS_ENDPOINT_URL")
                    .profile("endpoint_url")
                    .build()
                    .expect("service config key fields are all specified"),
            )
        })
        .or_else(|| sdk.endpoint_url().map(str::to_owned))
}

fn sanitized_endpoint(raw: Option<&str>) -> Option<String> {
    raw.map(|raw| match url::Url::parse(raw) {
        Ok(mut url) if matches!(url.scheme(), "http" | "https") && url.has_host() => {
            let _ = url.set_username("");
            let _ = url.set_password(None);
            url.set_query(None);
            url.set_fragment(None);
            url.to_string()
        }
        _ => "<invalid endpoint override>".into(),
    })
}

async fn run(args: Args) -> Result<u8, String> {
    let sdk = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_config::Region::new(args.region.clone()))
        .timeout_config(
            aws_config::timeout::TimeoutConfig::builder()
                .operation_timeout(std::time::Duration::from_secs(args.timeout_seconds as u64))
                .operation_attempt_timeout(std::time::Duration::from_secs(
                    args.timeout_seconds.min(15) as u64,
                ))
                .build(),
        )
        .load()
        .await;
    let dynamodb_endpoint = endpoint(&sdk, "DynamoDB");
    let s3_endpoint = endpoint(&sdk, "S3");
    // Pin the exact resolved values used for reporting to the actual clients.
    // Keep credentials/query/fragment only in the client configuration; never
    // emit them in the report's endpoint identity.
    let mut dynamodb = aws_sdk_dynamodb::config::Builder::from(&sdk);
    dynamodb.set_endpoint_url(dynamodb_endpoint.clone());
    let mut s3 = aws_sdk_s3::config::Builder::from(&sdk).force_path_style(s3_endpoint.is_some());
    s3.set_endpoint_url(s3_endpoint.clone());
    let cloud = Cloud::new(
        aws_sdk_dynamodb::Client::from_conf(dynamodb.build()),
        aws_sdk_s3::Client::from_conf(s3.build()),
        &args,
    );
    emit(
        &json!({"kind":"header","schema_version":1,"region":args.region,"table":args.table,"bucket":args.bucket,
        "dynamodb_endpoint":sanitized_endpoint(dynamodb_endpoint.as_deref()),
        "s3_endpoint":sanitized_endpoint(s3_endpoint.as_deref()),
        "all":args.all,"history":args.history,"include_deleted":args.include_deleted,
        "max_bytes":args.max_bytes,"max_memory_bytes":args.max_memory_bytes,"max_rows":args.max_rows,"max_documents":args.max_documents,"max_pages":args.max_pages,"max_nodes":args.max_nodes,"max_depth":args.max_depth,"timeout_seconds":args.timeout_seconds,
        "scope":"persisted structural DOM only; not an atomic backup, live room state, rendered HTML, attributes, or author proof"}),
    )?;
    let mut summary = Summary {
        kind: "summary",
        ..Default::default()
    };
    let mut seen = HashSet::new();
    let mut cursor = None;
    let mut pages = 0usize;
    'inventory: loop {
        if args.all {
            pages += 1;
            if pages > args.max_pages {
                summary.incomplete += 1;
                emit(
                    &json!({"kind":"error","stage":"metadata_inventory","error":"inventory page budget exceeded"}),
                )?;
                break;
            }
        }
        let ids = if args.all {
            match cloud.scan_page(cursor.take()).await {
                Ok((items, next)) => {
                    cursor = next;
                    let mut ids = Vec::new();
                    for item in items {
                        match string(&item, "PK").and_then(|s| {
                            s.strip_prefix("DOC#")
                                .filter(|s| !s.is_empty())
                                .map(str::to_string)
                        }) {
                            Some(id) => ids.push(id),
                            None => {
                                summary.documents += 1;
                                summary.errors += 1;
                                emit(
                                    &json!({"kind":"document","doc_id":null,"status":"error","error":"malformed metadata primary key"}),
                                )?;
                            }
                        }
                    }
                    ids
                }
                Err(e) => {
                    summary.errors += 1;
                    emit(&json!({"kind":"error","stage":"metadata_inventory","error":e}))?;
                    break;
                }
            }
        } else {
            args.ids.iter().cloned().collect()
        };
        for id in ids {
            if seen.len() >= args.max_documents && !seen.contains(&id) {
                summary.incomplete += 1;
                emit(
                    &json!({"kind":"error","stage":"metadata_inventory","error":"inventory document budget exceeded"}),
                )?;
                break 'inventory;
            }
            if !seen.insert(id.clone()) {
                summary.incomplete += 1;
                emit(
                    &json!({"kind":"error","doc_id":id,"stage":"metadata_inventory","error":"duplicate document during inventory; scan is unstable"}),
                )?;
                continue;
            }
            let report = inspect_document(&cloud, &args, &id).await;
            count_document(&mut summary, &report);
            emit(&report)?;
            if args.history {
                scan_history(&cloud, &args, &report, &mut summary).await?;
            }
        }
        if !args.all || cursor.is_none() {
            summary.inventory_complete = true;
            break;
        }
    }
    summary.exit_code =
        if summary.errors > 0 || summary.incomplete > 0 || !summary.inventory_complete {
            2
        } else if summary.invalid_current
            + summary.invalid_snapshots
            + summary.invalid_history
            + summary.intermediate_invalid
            > 0
        {
            1
        } else {
            0
        };
    let code = summary.exit_code;
    emit(&summary)?;
    Ok(code)
}

fn main() -> ExitCode {
    // Panic payloads from decoding damaged content can include user data.
    // The inspector reports sanitized stage errors instead.
    std::panic::set_hook(Box::new(|_| {}));
    if std::env::args().nth(1).as_deref() == Some("--internal-dom-inspect") {
        return worker::run_worker();
    }
    match Args::parse() {
        Ok(None) => {
            print!("{}", args::HELP);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}\n{}", args::HELP);
            ExitCode::from(2)
        }
        Ok(Some(args)) => match tokio::runtime::Runtime::new()
            .map_err(|_| "cannot initialize scanner runtime".to_string())
            .and_then(|runtime| runtime.block_on(run(args)))
        {
            Ok(code) => ExitCode::from(code),
            Err(e) => {
                eprintln!("scanner failed: {e}");
                ExitCode::from(2)
            }
        },
    }
}
