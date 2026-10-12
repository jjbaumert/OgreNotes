// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Decode untrusted CRDT data with kernel-enforced process resource limits.
use super::{
    cloud::StoredUpdate,
    inspect::{self, Check, Inspector, Violation},
};
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Read, Write},
    process::{ExitCode, Stdio},
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const OUTPUT_LIMIT: usize = 1024 * 1024;
pub(super) const WORKER_ARGUMENT: &str = "--internal-dom-inspect";

#[derive(Serialize, Deserialize)]
pub(super) struct ReplayChecks {
    pub snapshot: Check,
    pub current: Check,
    pub updates_applied: usize,
    pub invalid_replay_observations: usize,
    pub first_invalid_index: Option<usize>,
    pub first_invalid_violations: Vec<Violation>,
    pub last_valid_index: Option<usize>,
}

pub(super) async fn inspect(
    snapshot: &[u8],
    updates: &[StoredUpdate],
    max_nodes: usize,
    max_depth: usize,
    memory_bytes: usize,
    timeout: Duration,
) -> Result<ReplayChecks, String> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (
            snapshot,
            updates,
            max_nodes,
            max_depth,
            memory_bytes,
            timeout,
        );
        Err("bounded DOM inspection is unsupported on this platform".into())
    }
    #[cfg(target_os = "linux")]
    {
        let bytes = updates
            .iter()
            .try_fold(snapshot.len(), |n, row| n.checked_add(row.bytes.len()))
            .ok_or("DOM worker input size overflow")?;
        let executable =
            std::env::current_exe().map_err(|_| "cannot locate DOM worker executable")?;
        let mut command = tokio::process::Command::new(executable);
        command
            .arg(WORKER_ARGUMENT)
            .arg(bytes.to_string())
            .arg(updates.len().to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let memory =
            libc::rlim_t::try_from(memory_bytes).map_err(|_| "DOM worker memory limit overflow")?;
        let cpu = libc::rlim_t::try_from(timeout.as_secs().saturating_add(1))
            .map_err(|_| "DOM worker CPU limit overflow")?;
        // Only async-signal-safe calls may run between fork and exec.
        unsafe {
            command.pre_exec(move || {
                for (resource, limit) in [
                    (libc::RLIMIT_CORE, 0),
                    (libc::RLIMIT_AS, memory),
                    (libc::RLIMIT_CPU, cpu),
                ] {
                    let limits = libc::rlimit {
                        rlim_cur: limit,
                        rlim_max: limit,
                    };
                    if libc::setrlimit(resource, &limits) != 0 {
                        return Err(io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
        let mut child = command
            .spawn()
            .map_err(|_| "cannot start bounded DOM worker")?;
        let mut input = child.stdin.take().ok_or("DOM worker input unavailable")?;
        let output = child.stdout.take().ok_or("DOM worker output unavailable")?;
        let operation = async {
            let send = async move {
                for value in [max_nodes, max_depth, bytes, updates.len()] {
                    input.write_all(&(value as u64).to_le_bytes()).await?;
                }
                input
                    .write_all(&(snapshot.len() as u64).to_le_bytes())
                    .await?;
                input.write_all(snapshot).await?;
                for row in updates {
                    input
                        .write_all(&(row.bytes.len() as u64).to_le_bytes())
                        .await?;
                    input.write_all(&row.bytes).await?;
                }
                let result = input.shutdown().await;
                drop(input);
                result
            };
            let receive = async {
                let mut response = Vec::new();
                output
                    .take((OUTPUT_LIMIT + 1) as u64)
                    .read_to_end(&mut response)
                    .await?;
                if response.len() > OUTPUT_LIMIT {
                    return Err(io::Error::other("DOM worker output limit exceeded"));
                }
                Ok(response)
            };
            let (_, response, status) = tokio::try_join!(send, receive, child.wait())
                .map_err(|_| "bounded DOM worker communication failed")?;
            if !status.success() {
                return Err("bounded DOM worker failed or exceeded resource limits");
            }
            serde_json::from_slice(&response).map_err(|_| "invalid bounded DOM worker response")
        };
        match tokio::time::timeout(timeout, operation).await {
            Ok(result) => result.map_err(str::to_string),
            Err(_) => Err("bounded DOM worker exceeded wall-clock deadline".into()),
        }
        // kill_on_drop terminates a worker on timeout or communication failure;
        // Tokio reaps the terminated process without blocking here.
    }
}

fn number(input: &mut impl Read) -> Result<usize, ()> {
    let mut bytes = [0; 8];
    input.read_exact(&mut bytes).map_err(|_| ())?;
    usize::try_from(u64::from_le_bytes(bytes)).map_err(|_| ())
}

fn payload(input: &mut impl Read, remaining: &mut usize) -> Result<Vec<u8>, ()> {
    let len = number(input)?;
    *remaining = remaining.checked_sub(len).ok_or(())?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(len).map_err(|_| ())?;
    bytes.resize(len, 0);
    input.read_exact(&mut bytes).map_err(|_| ())?;
    Ok(bytes)
}

fn replay(input: &mut impl Read, byte_cap: usize, update_cap: usize) -> Result<ReplayChecks, ()> {
    let max_nodes = number(input)?;
    let max_depth = number(input)?;
    let mut remaining = number(input)?;
    let count = number(input)?;
    if remaining > byte_cap || count > update_cap || max_nodes == 0 || max_depth == 0 {
        return Err(());
    }
    let snapshot = payload(input, &mut remaining)?;
    let mut doc = inspect::decode(&snapshot).map_err(|_| ())?;
    drop(snapshot);
    let mut inspector = Inspector {
        remaining_nodes: max_nodes,
        max_depth,
    };
    let snapshot = inspector.check(&doc);
    let mut observed_valid = snapshot.status == "valid";
    let mut result = ReplayChecks {
        current: snapshot.clone(),
        snapshot,
        updates_applied: 0,
        invalid_replay_observations: 0,
        first_invalid_index: None,
        first_invalid_violations: Vec::new(),
        last_valid_index: None,
    };
    for index in 0..count {
        // Pending CRDT dependencies can resolve later; exhausted inspection
        // budgets and inspection failures cannot recover within this request.
        if result.current.limit_hit || result.current.status == "error" {
            let len = number(input)?;
            remaining = remaining.checked_sub(len).ok_or(())?;
            let copied =
                io::copy(&mut (&mut *input).take(len as u64), &mut io::sink()).map_err(|_| ())?;
            if copied != len as u64 {
                return Err(());
            }
            continue;
        }
        let update = payload(input, &mut remaining)?;
        inspect::apply(&mut doc, &update).map_err(|_| ())?;
        result.updates_applied += 1;
        result.current = inspector.check(&doc);
        if result.current.status == "valid" {
            observed_valid = true;
            result.last_valid_index = Some(index);
        }
        if result.current.status == "invalid" && !result.current.unresolved_dependencies {
            result.invalid_replay_observations += 1;
            if observed_valid && result.first_invalid_index.is_none() {
                result.first_invalid_index = Some(index);
                result.first_invalid_violations = result.current.violations.clone();
            }
        }
    }
    let mut trailing = [0];
    if remaining != 0 || input.read(&mut trailing).map_err(|_| ())? != 0 {
        return Err(());
    }
    if result.updates_applied < count {
        result.current.status = "inconclusive".into();
        result.current.complete = false;
        result.current.error =
            Some("replay stopped before all retained updates were applied".into());
    }
    if !result.current.complete {
        result.current.status = "inconclusive".into();
    }
    Ok(result)
}

/// Private worker mode never initializes AWS clients or emits document data.
pub(super) fn run_worker() -> ExitCode {
    // Linux pipe core handlers may ignore RLIMIT_CORE. Disable dumpability
    // after exec, which can reset it, before reading any document bytes.
    #[cfg(target_os = "linux")]
    if unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) } != 0 {
        return ExitCode::from(2);
    }
    std::panic::set_hook(Box::new(|_| {}));
    let args: Vec<_> = std::env::args().collect();
    let caps = if args.len() == 4 && args[1] == WORKER_ARGUMENT {
        args[2]
            .parse::<usize>()
            .ok()
            .zip(args[3].parse::<usize>().ok())
    } else {
        None
    };
    let Some((byte_cap, update_cap)) = caps else {
        return ExitCode::from(2);
    };
    let result = std::panic::catch_unwind(|| replay(&mut io::stdin().lock(), byte_cap, update_cap));
    let Ok(Ok(result)) = result else {
        return ExitCode::from(2);
    };
    let Ok(bytes) = serde_json::to_vec(&result) else {
        return ExitCode::from(2);
    };
    if bytes.len() > OUTPUT_LIMIT || io::stdout().lock().write_all(&bytes).is_err() {
        return ExitCode::from(2);
    }
    ExitCode::SUCCESS
}
