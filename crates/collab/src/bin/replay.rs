//! Replay a document's edit history from DynamoDB + S3.
//!
//! Loads the latest snapshot, then applies each pending UPDATE# row one at a time,
//! checking for panics and errors at each step. Useful for post-mortem debugging
//! of crashes that occurred during collaborative editing.
//!
//! Usage:
//!   cargo run -p ogrenotes-collab --features replay --bin replay -- <doc_id>
//!
//! Requires: DYNAMODB_TABLE_PREFIX, S3_BUCKET, AWS_REGION env vars (same as API server).

use ogrenotes_collab::document::OgreDoc;
use ogrenotes_common::config::AppConfig;
use ogrenotes_storage::dynamo::DynamoClient;
use ogrenotes_storage::repo::doc_repo::DocRepo;
use ogrenotes_storage::s3::S3Client;

#[tokio::main]
async fn main() {
    let doc_id = match std::env::args().nth(1) {
        Some(id) => id,
        None => {
            eprintln!("Usage: replay <doc_id>");
            eprintln!("Replays a document's edit history from snapshot + pending updates.");
            eprintln!();
            eprintln!("Required env vars: DYNAMODB_TABLE_PREFIX, S3_BUCKET, AWS_REGION");
            std::process::exit(1);
        }
    };

    let config = AppConfig::from_env();
    let aws_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_config::Region::new(config.aws_region.clone()))
        .load()
        .await;

    let dynamo = DynamoClient::new(
        aws_sdk_dynamodb::Client::new(&aws_config),
        config.table_name(),
    );
    let s3 = S3Client::new(
        aws_sdk_s3::Client::new(&aws_config),
        config.s3_bucket.clone(),
    );
    let doc_repo = DocRepo::new(dynamo, s3);

    // Load snapshot
    println!("Loading snapshot for doc_id={doc_id}...");
    let snapshot_bytes = match doc_repo.load_snapshot(&doc_id).await {
        Ok(Some(bytes)) => {
            println!("  Snapshot loaded: {} bytes", bytes.len());
            Some(bytes)
        }
        Ok(None) => {
            println!("  No snapshot found, starting from empty doc");
            None
        }
        Err(e) => {
            eprintln!("  Failed to load snapshot: {e}");
            std::process::exit(1);
        }
    };

    let mut doc = match &snapshot_bytes {
        Some(bytes) => match OgreDoc::from_state_bytes(bytes) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("  Corrupt snapshot: {e}");
                std::process::exit(1);
            }
        },
        None => OgreDoc::new(),
    };

    // Fetch pending updates
    println!("Fetching pending updates...");
    let updates = match doc_repo.get_pending_updates(&doc_id).await {
        Ok(u) => u,
        Err(e) => {
            eprintln!("  Failed to get updates: {e}");
            std::process::exit(1);
        }
    };
    println!("  Found {} pending updates\n", updates.len());

    if updates.is_empty() {
        println!("Nothing to replay.");
        println!("Final doc state: {} bytes", doc.to_state_bytes().len());
        return;
    }

    // Replay one at a time
    let mut last_good = 0;
    for (i, update) in updates.iter().enumerate() {
        let step = i + 1;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            doc.apply_update(&update.update_bytes)
        }));

        match result {
            Ok(Ok(())) => {
                let size = doc.to_state_bytes().len();
                let ver = update.client_version.as_deref().unwrap_or("-");
                println!(
                    "[{step}/{}] OK    clock={:<30} user={:<20} ver={:<10} update_bytes={:<6} doc_size={}",
                    updates.len(),
                    update.clock,
                    update.user_id,
                    ver,
                    update.update_bytes.len(),
                    size,
                );
                last_good = step;
            }
            Ok(Err(e)) => {
                let ver = update.client_version.as_deref().unwrap_or("-");
                eprintln!(
                    "[{step}/{}] ERROR clock={:<30} user={:<20} ver={:<10} error={}",
                    updates.len(),
                    update.clock,
                    update.user_id,
                    ver,
                    e,
                );
                eprintln!("\nStopping replay at step {step}. Last good step: {last_good}.");
                eprintln!("Failing update clock: {}", update.clock);
                eprintln!("Failing update bytes ({} bytes): {:?}", update.update_bytes.len(), &update.update_bytes[..update.update_bytes.len().min(100)]);
                break;
            }
            Err(panic_info) => {
                let msg = if let Some(s) = panic_info.downcast_ref::<String>() {
                    s.clone()
                } else if let Some(s) = panic_info.downcast_ref::<&str>() {
                    s.to_string()
                } else {
                    format!("{panic_info:?}")
                };
                let ver = update.client_version.as_deref().unwrap_or("-");
                eprintln!(
                    "[{step}/{}] PANIC clock={:<30} user={:<20} ver={:<10} panic={}",
                    updates.len(),
                    update.clock,
                    update.user_id,
                    ver,
                    msg,
                );
                eprintln!("\nStopping replay at step {step}. Last good step: {last_good}.");
                eprintln!("Failing update clock: {}", update.clock);
                eprintln!("Failing update bytes ({} bytes): {:?}", update.update_bytes.len(), &update.update_bytes[..update.update_bytes.len().min(100)]);
                break;
            }
        }
    }

    println!("\nFinal doc state: {} bytes", doc.to_state_bytes().len());
}
