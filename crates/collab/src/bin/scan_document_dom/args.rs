// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use ogrenotes_common::config::table_name_for_prefix;
use std::collections::BTreeSet;

pub(super) const HELP: &str = "Read-only persisted OgreNotes DOM scanner
Usage: scan_document_dom (--doc-id ID [--doc-id ID ...] | --all) [OPTIONS]
  --table NAME           Full DynamoDB table; defaults to DYNAMODB_TABLE_PREFIX + ogrenote
  --bucket NAME          S3 bucket; defaults to S3_BUCKET
  --region REGION        Defaults to AWS_REGION, then us-east-1
  --history              Inspect retained historical S3 snapshots, including unindexed blobs
  --include-deleted      Include soft-deleted documents (otherwise reported as skipped)
  --max-bytes N          Current snapshot/tail bytes; separate aggregate history cap (default 33554432)
  --max-rows N           Pending/history row cap per doc (default 10000)
  --max-nodes N          Node visits per current replay/history sweep (default 1000000)
  --max-depth N          Tree depth cap (default 128)
  --max-memory-bytes N   Isolated decoder address-space cap (default 536870912)
  --timeout-seconds N    AWS operation/S3 body deadline (default 45)
  --max-documents N      Inventory document cap (default 100000)
  --max-pages N          Inventory scan page cap (default 10000)
  --page-size N          DynamoDB evaluated rows per page, 1..1000 (default 100)
  --help                 Show this help
Output: NDJSON header/document/history/summary. No raw document text.
Exit: 0 completed clean scope, 1 invalid structures, 2 incomplete/operational failure.
Uses normal AWS credentials/profile and AWS_ENDPOINT_URL[_DYNAMODB|_S3] overrides.
Only structural DOM rules are checked, not rendered HTML or attribute semantics.
";

pub(super) struct Args {
    pub table: String,
    pub bucket: String,
    pub region: String,
    pub ids: BTreeSet<String>,
    pub all: bool,
    pub history: bool,
    pub include_deleted: bool,
    pub max_bytes: usize,
    pub max_rows: usize,
    pub max_nodes: usize,
    pub max_depth: usize,
    pub max_memory_bytes: usize,
    pub timeout_seconds: usize,
    pub page_size: i32,
    pub max_documents: usize,
    pub max_pages: usize,
}

impl Args {
    pub fn parse() -> Result<Option<Self>, String> {
        let mut args = std::env::args().skip(1);
        let mut out = Self {
            table: std::env::var("DYNAMODB_TABLE_PREFIX")
                .ok()
                .map(|s| table_name_for_prefix(&s))
                .unwrap_or_default(),
            bucket: std::env::var("S3_BUCKET").unwrap_or_default(),
            region: std::env::var("AWS_REGION").unwrap_or_else(|_| "us-east-1".into()),
            ids: BTreeSet::new(),
            all: false,
            history: false,
            include_deleted: false,
            max_bytes: 33554432,
            max_rows: 10000,
            max_nodes: 1000000,
            max_depth: 128,
            max_memory_bytes: 536870912,
            timeout_seconds: 45,
            page_size: 100,
            max_documents: 100000,
            max_pages: 10000,
        };
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--help" | "-h" => return Ok(None),
                "--all" => out.all = true,
                "--history" => out.history = true,
                "--include-deleted" => out.include_deleted = true,
                "--table" | "--bucket" | "--region" | "--doc-id" => {
                    let value = args
                        .next()
                        .filter(|v| !v.is_empty() && !v.starts_with("--"))
                        .ok_or_else(|| format!("{arg} requires a value"))?;
                    match arg.as_str() {
                        "--table" => out.table = value,
                        "--bucket" => out.bucket = value,
                        "--region" => out.region = value,
                        _ => {
                            out.ids.insert(value);
                        }
                    }
                }
                "--max-bytes" | "--max-rows" | "--max-nodes" | "--max-depth" | "--page-size"
                | "--max-documents" | "--max-pages" | "--timeout-seconds"
                | "--max-memory-bytes" => {
                    let value = args
                        .next()
                        .and_then(|s| s.parse::<usize>().ok())
                        .filter(|v| *v > 0)
                        .ok_or_else(|| format!("{arg} requires a positive integer"))?;
                    match arg.as_str() {
                        "--max-bytes" => out.max_bytes = value,
                        "--max-rows" => out.max_rows = value,
                        "--max-nodes" => out.max_nodes = value,
                        "--max-depth" => out.max_depth = value,
                        "--max-memory-bytes" => out.max_memory_bytes = value,
                        "--timeout-seconds" => out.timeout_seconds = value,
                        "--max-documents" => out.max_documents = value,
                        "--max-pages" => out.max_pages = value,
                        _ if value <= 1000 => out.page_size = value as i32,
                        _ => return Err("--page-size must be between 1 and 1000".into()),
                    }
                }
                _ => return Err(format!("unknown option: {arg}")),
            }
        }
        if out.all != out.ids.is_empty() {
            return Err("choose either --all or one or more --doc-id values".into());
        }
        if out.table.trim().is_empty()
            || out.bucket.trim().is_empty()
            || out.region.trim().is_empty()
        {
            return Err("table, bucket and region must be specified by flags or documented environment variables".into());
        }
        Ok(Some(out))
    }
}
