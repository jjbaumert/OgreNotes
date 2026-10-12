// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Exercise the shipped CLI, HTTP/SSE parsing, CSV output, and report together.
//! Every response comes from a loopback fixture; no model or AWS calls occur.
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone)]
struct Reply {
    status: u16,
    body: String,
}
impl Reply {
    fn search(ids: &[&str]) -> Self {
        Self {
            status: 200,
            body: json!({"results": ids.iter().map(|id| json!({"id": id})).collect::<Vec<_>>()})
                .to_string(),
        }
    }
    fn ask(ids: &[&str], done: bool) -> Self {
        let mut body = ids
            .iter()
            .map(|id| format!("event: source\ndata: {}\n\n", json!({"docId": id})))
            .collect::<String>();
        if done {
            body.push_str("event: done\ndata: {}\n\n");
        }
        Self { status: 200, body }
    }
    fn failed(status: u16) -> Self {
        Self {
            status,
            body: "fixture failure".into(),
        }
    }
}
struct Case {
    expected: Vec<&'static str>,
    search: Reply,
    ask: Reply,
}
fn case(expected: &[&'static str], search: Reply, ask: Reply) -> Case {
    Case {
        expected: expected.to_vec(),
        search,
        ask,
    }
}

struct Fixture {
    base: String,
    stop: Arc<AtomicBool>,
    task: Option<thread::JoinHandle<()>>,
}
impl Fixture {
    fn new(cases: &[Case]) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let replies: Vec<_> = cases
            .iter()
            .map(|c| (c.search.clone(), c.ask.clone()))
            .collect();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let task = thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => serve(stream, &replies),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(e) => panic!("fixture accept: {e}"),
                }
            }
        });
        Self {
            base,
            stop,
            task: Some(task),
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.task.take().unwrap().join().unwrap();
    }
}
fn serve(mut stream: TcpStream, replies: &[(Reply, Reply)]) {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let path = line.split_whitespace().nth(1).unwrap().to_owned();
    let mut length = 0;
    loop {
        line.clear();
        reader.read_line(&mut line).unwrap();
        if line == "\r\n" || line.is_empty() {
            break;
        }
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            length = value.trim().parse::<usize>().unwrap();
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).unwrap();
    let reply = if path == "/api/v1/auth/dev-login" {
        Reply {
            status: 200,
            body: json!({"accessToken": "local-fixture"}).to_string(),
        }
    } else if path.starts_with("/api/v1/search?") {
        let index = path
            .split("q=q")
            .nth(1)
            .unwrap()
            .split('&')
            .next()
            .unwrap()
            .parse::<usize>()
            .unwrap();
        replies[index].0.clone()
    } else if path == "/api/v1/ask" {
        let question: Value = serde_json::from_slice(&body).unwrap();
        let index = question["question"]
            .as_str()
            .unwrap()
            .strip_prefix('q')
            .unwrap()
            .parse::<usize>()
            .unwrap();
        replies[index].1.clone()
    } else {
        panic!("unexpected fixture request: {path}")
    };
    write!(stream, "HTTP/1.1 {} Fixture\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        reply.status, if path == "/api/v1/ask" { "text/event-stream" } else { "application/json" }, reply.body.len(), reply.body).unwrap();
}

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "rag-eval-cli-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
struct Report {
    rows: Vec<BTreeMap<String, String>>,
    stdout: String,
}
fn run(cases: Vec<Case>, no_ask: bool) -> Report {
    let fixture = Fixture::new(&cases);
    let scratch = Scratch::new();
    let catalog = json!({
        "corpus": [
            {"id": "doc-a", "title": "A", "content": "alpha"},
            {"id": "doc-b", "title": "B", "content": "beta"}
        ],
        "queries": cases.iter().enumerate().map(|(i,c)| json!({
            "id": format!("q{i}"), "text": format!("q{i}"),
            "category": if c.expected.is_empty() { "gap-analysis" } else { "keyword" },
            "expectedDocIds": c.expected
        })).collect::<Vec<_>>()
    });
    std::fs::write(scratch.0.join("catalog.json"), catalog.to_string()).unwrap();
    std::fs::write(
        scratch.0.join("state.json"),
        json!({
            "baseUrl": fixture.base, "email": "eval@fixture.invalid",
            "docIds": {"doc-a": "server-a", "doc-b": "server-b"}
        })
        .to_string(),
    )
    .unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_rag-eval"));
    command
        .current_dir(&scratch.0)
        .env("RAG_EVAL_CATALOG", scratch.0.join("catalog.json"))
        .env("NO_PROXY", "127.0.0.1")
        .env("no_proxy", "127.0.0.1")
        .args([
            "run",
            "--base-url",
            &fixture.base,
            "--state",
            "state.json",
            "--out",
            "report.csv",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if no_ask {
        command.arg("--no-ask");
    }
    let mut child = command.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            panic!("CLI timed out: {}", String::from_utf8_lossy(&output.stderr));
        }
        thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "completed evaluation must emit its report even when requests fail: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rows = csv::Reader::from_path(scratch.0.join("report.csv"))
        .unwrap()
        .deserialize::<BTreeMap<String, String>>()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(rows.len(), cases.len());
    Report {
        rows,
        stdout: String::from_utf8(output.stdout).unwrap(),
    }
}

#[test]
fn failed_gap_search_scores_zero_and_is_counted() {
    let report = run(
        vec![case(&[], Reply::failed(500), Reply::ask(&[], true))],
        true,
    );
    assert_eq!(report.rows[0]["search_recall_at_10"], "0.000");
    assert!(report.rows[0]["error"].contains("search:"));
    assert!(
        report.stdout.contains("search failures:        1"),
        "{}",
        report.stdout
    );
}

#[test]
fn unknown_search_and_citation_ids_are_not_erased_from_gap_results() {
    let report = run(
        vec![case(
            &[],
            Reply::search(&["alien-server"]),
            Reply::ask(&["alien-server"], true),
        )],
        false,
    );
    let row = &report.rows[0];
    assert_eq!(row["search_recall_at_10"], "0.000");
    assert_eq!(row["ask_recall_at_10"], "0.000");
    assert!(row["search_top10"].contains("alien-server"));
    assert!(row["ask_cites"].contains("alien-server"));
}

#[test]
fn duplicate_hits_and_citations_cannot_inflate_recall() {
    let report = run(
        vec![case(
            &["doc-a", "doc-b"],
            Reply::search(&["server-a", "server-a", "server-a"]),
            Reply::ask(&["server-a", "server-a", "server-a"], true),
        )],
        false,
    );
    assert_eq!(report.rows[0]["search_recall_at_10"], "0.500");
    assert_eq!(report.rows[0]["ask_recall_at_10"], "0.500");
}

#[test]
fn failed_ask_requests_remain_in_summary_denominator() {
    let report = run(
        vec![
            case(
                &["doc-a"],
                Reply::search(&["server-a"]),
                Reply::ask(&["server-a"], true),
            ),
            case(&["doc-a"], Reply::search(&["server-a"]), Reply::failed(500)),
        ],
        false,
    );
    assert_eq!(report.rows[1]["ask_skipped"], "false");
    assert_eq!(report.rows[1]["ask_recall_at_10"], "0.000");
    assert!(report.rows[1]["error"].contains("ask:"));
    assert!(
        report
            .stdout
            .contains("mean ask recall@10:     0.500 (n=2)"),
        "{}",
        report.stdout
    );
    assert!(
        report.stdout.contains("ask failures:           1"),
        "{}",
        report.stdout
    );
}

#[test]
fn clean_http_eof_without_done_is_a_failed_ask_even_after_a_source() {
    let report = run(
        vec![
            case(&[], Reply::search(&[]), Reply::ask(&[], false)),
            case(
                &["doc-a"],
                Reply::search(&["server-a"]),
                Reply::ask(&["server-a"], false),
            ),
        ],
        false,
    );
    for row in &report.rows {
        assert_eq!(row["ask_recall_at_10"], "0.000");
        assert_eq!(row["ask_skipped"], "false");
        assert!(row["error"].contains("done"), "{row:?}");
    }
    assert!(
        report
            .stdout
            .contains("mean ask recall@10:     0.000 (n=2)")
    );
}

#[test]
fn explicit_skips_and_completed_empty_results_keep_their_contract() {
    let report = run(
        vec![
            case(&[], Reply::search(&[]), Reply::ask(&[], true)),
            case(&[], Reply::search(&[]), Reply::failed(503)),
        ],
        false,
    );
    assert_eq!(report.rows[0]["ask_recall_at_10"], "1.000");
    assert_eq!(report.rows[1]["ask_skipped"], "true");
    assert!(report.rows.iter().all(|r| r["error"].is_empty()));
    assert!(
        report
            .stdout
            .contains("mean ask recall@10:     1.000 (n=1)")
    );
    let skipped = run(
        vec![case(&[], Reply::search(&[]), Reply::failed(500))],
        true,
    );
    assert_eq!(skipped.rows[0]["ask_skipped"], "true");
    assert!(skipped.rows[0]["error"].is_empty());
}

#[test]
fn unknown_server_id_cannot_impersonate_a_catalog_id() {
    let report = run(
        vec![case(
            &["doc-a"],
            Reply::search(&["doc-a"]),
            Reply::ask(&["doc-a"], true),
        )],
        false,
    );
    assert_eq!(report.rows[0]["search_recall_at_10"], "0.000");
    assert_eq!(report.rows[0]["ask_recall_at_10"], "0.000");
    assert_eq!(report.rows[0]["search_top10"], "doc-a");
    assert_eq!(report.rows[0]["ask_cites"], "doc-a");
}

#[test]
fn both_request_failures_remain_visible_in_the_row() {
    let report = run(
        vec![case(&[], Reply::failed(500), Reply::failed(500))],
        false,
    );
    assert_eq!(report.rows[0]["search_recall_at_10"], "0.000");
    assert_eq!(report.rows[0]["ask_recall_at_10"], "0.000");
    assert!(report.rows[0]["error"].contains("search:"));
    assert!(report.rows[0]["error"].contains("ask:"));
    assert!(
        report
            .stdout
            .contains("mean ask recall@10:     0.000 (n=1)")
    );
}
