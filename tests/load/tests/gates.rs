// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! End-to-end gate checks against a tiny HTTP server. These exercise
//! the compiled driver, Goose aggregation, and process exit status.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

#[derive(Clone, Copy)]
enum StubMode {
    Healthy,
    BadLogin,
    EmptyToken,
    MalformedToken,
    ServerErrors,
    Slow,
}

fn respond(mut stream: TcpStream, mode: StubMode) {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut buf = [0; 8192];
    let mut bytes = Vec::new();
    while bytes.len() < 8192 {
        let size = stream.read(&mut buf).unwrap_or(0);
        if size == 0 {
            break;
        }
        bytes.extend_from_slice(&buf[..size]);
        if bytes.windows(4).any(|part| part == b"\r\n\r\n") {
            break;
        }
    }
    let request = String::from_utf8_lossy(&bytes);
    let (status, body) = if request.starts_with("POST /api/v1/auth/dev-login ") {
        match mode {
            StubMode::BadLogin => ("401 Unauthorized", "denied"),
            StubMode::EmptyToken => ("200 OK", r#"{"accessToken":""}"#),
            StubMode::MalformedToken => ("200 OK", "{invalid-json"),
            _ => ("200 OK", r#"{"accessToken":"test-token"}"#),
        }
    } else if request.starts_with("GET /api/v1/documents ")
        && request
            .to_ascii_lowercase()
            .contains("authorization: bearer test-token")
    {
        match mode {
            StubMode::ServerErrors => ("500 Internal Server Error", "failure"),
            _ => ("200 OK", "[]"),
        }
    } else if request.starts_with("GET /api/v1/users/me ")
        && request
            .to_ascii_lowercase()
            .contains("authorization: bearer test-token")
    {
        match mode {
            StubMode::ServerErrors => ("500 Internal Server Error", "failure"),
            _ => ("200 OK", r#"{"id":"test-user"}"#),
        }
    } else {
        ("404 Not Found", "missing")
    };
    if matches!(mode, StubMode::Slow) && request.starts_with("GET /api/v1/documents ") {
        thread::sleep(Duration::from_millis(304));
    }
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
}

fn run(mode: StubMode) -> (bool, String, serde_json::Value) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let server_stop = Arc::clone(&stop);
    let server = thread::spawn(move || {
        while !server_stop.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((stream, _)) => respond(stream, mode),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("stub server: {error}"),
            }
        }
    });
    let summary = std::env::temp_dir().join(format!(
        "ogrenotes-load-gate-{}-{:?}.json",
        std::process::id(),
        thread::current().id()
    ));
    let goose_report = summary.with_extension("goose.html");
    let output = Command::new(env!("CARGO_BIN_EXE_load"))
        .args([
            "--host",
            &format!("http://{address}"),
            "--scenarios",
            "readheavy",
            "--users",
            "1",
            "--hatch-rate",
            "1",
            "--run-time",
            "5s",
            "--no-reset-metrics",
        ])
        .arg("--report-file")
        .arg(&goose_report)
        .env("LOAD_MIN_AUTHENTICATED_REQUESTS", "2")
        .env("LOAD_SUMMARY_FILE", &summary)
        .output()
        .unwrap();
    stop.store(true, Ordering::Relaxed);
    server.join().unwrap();
    let report = std::fs::read_to_string(&summary).unwrap_or_else(|_| {
        panic!(
            "driver did not write summary: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    });
    assert!(std::fs::metadata(&goose_report).unwrap().len() > 0);
    std::fs::remove_file(goose_report).unwrap();
    std::fs::remove_file(summary).unwrap();
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        serde_json::from_str(&report).unwrap(),
    )
}

#[test]
fn healthy_authenticated_traffic_passes() {
    let (success, output, report) = run(StubMode::Healthy);
    assert!(success, "{output}");
    assert!(
        report["successful_authenticated_requests"]
            .as_u64()
            .unwrap()
            >= 2
    );
    assert_eq!(report["failures"].as_array().unwrap().len(), 0);
}

#[test]
fn rejected_login_fails_without_authenticated_traffic() {
    let (success, _, report) = run(StubMode::BadLogin);
    assert!(!success);
    assert_eq!(report["successful_logins"], 0);
    assert_eq!(report["successful_authenticated_requests"], 0);
}

#[test]
fn empty_login_token_fails_without_authenticated_traffic() {
    let (success, _, report) = run(StubMode::EmptyToken);
    assert!(!success);
    assert_eq!(report["successful_logins"], 0);
    assert_eq!(report["successful_authenticated_requests"], 0);
}

#[test]
fn malformed_login_token_fails_without_authenticated_traffic() {
    let (success, _, report) = run(StubMode::MalformedToken);
    assert!(!success);
    assert_eq!(report["successful_logins"], 0);
    assert_eq!(report["successful_authenticated_requests"], 0);
}

#[test]
fn repeated_server_errors_fail() {
    let (success, _, report) = run(StubMode::ServerErrors);
    assert!(!success);
    assert!(report["failed_requests"].as_u64().unwrap() > 0);
    assert!(report["error_rate"].as_f64().unwrap() > 0.01);
    assert!(
        report["failures"]
            .as_array()
            .unwrap()
            .iter()
            .any(|failure| failure.as_str().unwrap().contains("error rate"))
    );
}

#[test]
fn slow_document_responses_fail_the_sla() {
    let (success, _, report) = run(StubMode::Slow);
    assert!(!success);
    assert!(report["documents_p95_ms"].as_u64().unwrap() > 300);
    assert!(
        report["failures"]
            .as_array()
            .unwrap()
            .iter()
            .any(|failure| failure.as_str().unwrap().contains("GET /documents p95"))
    );
}
