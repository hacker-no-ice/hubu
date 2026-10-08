//! Exercises the real native binary from operation handle to HTTP-fetched files.
//! The model-facing command contains four identifiers/options, never image bytes.
use rusqlite::Connection;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    io::{Read, Write},
    net::TcpListener,
    path::Path,
    process::{Command, Output},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

const HANDLE: &str = "hubu:public-operation:v1:gallery-fixture";

struct Fixture {
    root: tempfile::TempDir,
    endpoint: String,
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
    bytes: Vec<u8>,
}

impl Fixture {
    fn new(provider: &str, correction: bool, multiple: bool) -> Self {
        Self::with_fault(provider, correction, multiple, "none")
    }

    fn with_fault(provider: &str, correction: bool, multiple: bool, fault: &'static str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let db = Connection::open(root.path().join("operations.sqlite3")).unwrap();
        db.execute_batch("PRAGMA application_id=1213547087; PRAGMA user_version=6;
            CREATE TABLE harness_operations(operation_handle TEXT PRIMARY KEY, operation_state TEXT,
              tool_name TEXT, decision_id TEXT, gongbu_execution_id TEXT, gongbu_request_json TEXT);").unwrap();
        // Matches only fields the read-only projection reads; queued decoys prove
        // the native process never starts a worker or allocates/changes state.
        db.execute("INSERT INTO harness_operations VALUES (?1,'succeeded','hubu_submit_governed_execution','decision-1','execution-1',?2)",
            rusqlite::params![HANDLE, json!({"input":{"image_size":"2k"}}).to_string()]).unwrap();
        db.execute("INSERT INTO harness_operations VALUES ('hubu:public-operation:v1:queued','queued','hubu_submit_governed_execution',NULL,NULL,'{}')", []).unwrap();
        drop(db);
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend((0..3 * 1024 * 1024).map(|i| (i % 251) as u8));
        if fault == "jpeg" {
            bytes[..3].copy_from_slice(b"\xff\xd8\xff");
        }
        let media_type = if fault == "jpeg" {
            "image/jpeg"
        } else {
            "image/png"
        };
        let digest = format!("{:x}", Sha256::digest(&bytes));
        let provider_id = if provider == "flux" {
            "provider:black-forest-labs:flux"
        } else {
            "provider:google:gemini-developer"
        };
        let cost = json!({"amount":"1","scale":3,"currency":"usd"});
        let identity = json!({"id":provider_id,"display_name":provider});
        let record = json!({"authorization_id":"decision-1","status":"settled","agent_id":"agt_fixture","account_id":"aga_fixture",
            "provider":identity,"ledger_transaction_ids":["ledger-1"],"receipt":{"settlement_id":"settlement-1","actual_vendor_cost":cost,"budget_charge_cents":1}});
        let transaction = json!({"id":"ledger-1","authorization_id":"decision-1","settlement_id":"settlement-1","provider":identity,"effective_cost":cost,"cost_semantics":"original_total"});
        let mut artifacts = vec![
            json!({"artifact_id":"image-1","execution_id":"execution-1","kind":"image","media_type":media_type,"size_bytes":bytes.len(),"sha256":digest}),
        ];
        if fault == "foreign-artifact" {
            artifacts[0]["execution_id"] = json!("other-execution");
        }
        if multiple {
            let mut second = artifacts[0].clone();
            second["artifact_id"] = json!("image-2");
            artifacts.push(second);
        }
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}/", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let worker_requests = requests.clone();
        let worker_stop = stop.clone();
        let body = bytes.clone();
        let worker = thread::spawn(move || {
            let mut authorization_reads = 0;
            while !worker_stop.load(Ordering::SeqCst) {
                let (mut stream, _) = match listener.accept() {
                    Ok(s) => s,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(e) => panic!("{e}"),
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut raw = Vec::new();
                let mut chunk = [0; 4096];
                loop {
                    let count = stream.read(&mut chunk).unwrap();
                    if count == 0 {
                        break;
                    }
                    raw.extend_from_slice(&chunk[..count]);
                    if raw.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&raw);
                let line = request.lines().next().unwrap().to_owned();
                worker_requests.lock().unwrap().push(line.clone());
                assert!(
                    line.starts_with("GET "),
                    "export attempted a mutating request: {line}"
                );
                assert!(request
                    .to_ascii_lowercase()
                    .contains("authorization: bearer fixture-token"));
                let path = line.split_whitespace().nth(1).unwrap();
                let (media, response) = if path.starts_with("/spend/authorizations/show?") {
                    authorization_reads += 1;
                    let mut current = record.clone();
                    if correction && authorization_reads >= 2 {
                        current["ledger_transaction_ids"] = json!(["ledger-1", "correction-1"]);
                    }
                    ("application/json", serde_json::to_vec(&json!({"schema_version":"hubu-history-v1","authorization_record":current})).unwrap())
                } else if path.starts_with("/ledger/transactions?") {
                    // Cursor-separated original row plus another unrelated row.
                    let last = path.contains("cursor=next");
                    let rows = if last {
                        let mut transaction = transaction.clone();
                        if fault == "cost" {
                            transaction["effective_cost"]["amount"] = json!("2");
                        }
                        let mut rows = vec![transaction.clone()];
                        if fault == "unlisted-correction" {
                            transaction["id"] = json!("correction-1");
                            transaction["cost_semantics"] = json!("corrected_total");
                            rows.push(transaction);
                        }
                        rows
                    } else {
                        Vec::new()
                    };
                    ("application/json", serde_json::to_vec(&json!({"schema_version":"hubu-history-v1","transactions":rows,"next_cursor":if last {Value::Null} else {json!("next")}})).unwrap())
                } else if path == "/v1/executions/execution-1/artifacts" {
                    ("application/json", serde_json::to_vec(&json!({"schema_version":1,"execution_id":"execution-1","artifacts":artifacts})).unwrap())
                } else if matches!(path, "/v1/artifacts/image-1" | "/v1/artifacts/image-2") {
                    let mut bytes = body.clone();
                    if fault == "digest" {
                        bytes[16] ^= 1;
                    }
                    (media_type, bytes)
                } else {
                    panic!("unexpected route (probe or worker?): {line}")
                };
                write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: {media}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", response.len()).unwrap();
                stream.write_all(&response).unwrap();
            }
        });
        Self {
            root,
            endpoint,
            requests,
            stop,
            thread: Some(worker),
            bytes,
        }
    }

    fn command(&self, handle: &str, size: &str) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hubu-unified-mcp"));
        // Completely isolated environment: no real profile or secret files.
        command.env_clear().envs(HashMap::from([
            ("HUBU_UNIFIED_HUBU_ENDPOINT", self.endpoint.clone()),
            ("HUBU_UNIFIED_GONGBU_ENDPOINT", self.endpoint.clone()),
            ("HUBU_UNIFIED_HUBU_BEARER_TOKEN", "fixture-token".into()),
            ("HUBU_UNIFIED_GONGBU_BEARER_TOKEN", "fixture-token".into()),
            (
                "HUBU_UNIFIED_OPERATION_STATE_PATH",
                self.root
                    .path()
                    .join("operations.sqlite3")
                    .display()
                    .to_string(),
            ),
        ]));
        command
            .args([
                "gallery",
                "export",
                "--operation-handle",
                handle,
                "--output",
                self.root.path().join("gallery").to_str().unwrap(),
                "--size",
                size,
                "--tier",
                "draft",
            ])
            .output()
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.thread.take() {
            let result = worker.join();
            if !thread::panicking() {
                result.unwrap();
            }
        }
    }
}

#[test]
fn native_fetch_multi_megabyte_images_both_providers_and_replay() {
    for provider in ["flux", "gemini"] {
        let fixture = Fixture::new(provider, false, true);
        let database_before =
            std::fs::read(fixture.root.path().join("operations.sqlite3")).unwrap();
        for _ in 0..2 {
            let result = fixture.command(HANDLE, "2k");
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            assert!(
                result.stdout.len() < 4096,
                "model receives image bytes instead of paths"
            );
            let value: Value = serde_json::from_slice(&result.stdout).unwrap();
            let images = value["images"].as_array().unwrap();
            assert_eq!(images.len(), 2);
            for (index, image) in images.iter().enumerate() {
                let path = Path::new(image["path"].as_str().unwrap());
                assert_eq!(
                    path.file_name().unwrap().to_str().unwrap(),
                    format!("{:02}-{provider}-2k-draft-0.1c.png", index + 1)
                );
                assert_eq!(std::fs::read(path).unwrap(), fixture.bytes);
                let receipt: Value = serde_json::from_slice(
                    &std::fs::read(image["receipt_path"].as_str().unwrap()).unwrap(),
                )
                .unwrap();
                assert_eq!(
                    receipt["settled_cost"],
                    json!({"amount":"1","scale":3,"currency":"usd"})
                );
                assert_eq!(receipt["budget_charge_cents"], 1);
            }
        }
        assert_eq!(
            std::fs::read(fixture.root.path().join("operations.sqlite3")).unwrap(),
            database_before
        );
        let manifest: Value = serde_json::from_slice(
            &std::fs::read(fixture.root.path().join("gallery/.gallery.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest.as_array().unwrap().len(), 2);
        assert!(fixture
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|r| r.starts_with("GET ")));
    }
}

#[test]
fn native_final_accounting_read_rejects_correction_outside_cursor_snapshot() {
    let fixture = Fixture::new("flux", true, false);
    let result = fixture.command(HANDLE, "2k");
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("accounting changed"));
    assert!(!fixture.root.path().join("gallery").exists());
    let requests = fixture.requests.lock().unwrap();
    assert!(requests.iter().any(|r| r.contains("cursor=next")));
    assert!(requests.iter().any(|r| r.contains("/v1/artifacts/image-1")));
    assert!(requests
        .last()
        .unwrap()
        .contains("/spend/authorizations/show?"));
}

#[test]
fn native_incomplete_or_size_mismatched_operation_never_calls_backend() {
    let fixture = Fixture::new("flux", false, false);
    for (handle, size) in [("hubu:public-operation:v1:queued", "2k"), (HANDLE, "4k")] {
        let result = fixture.command(handle, size);
        assert!(!result.status.success());
    }
    assert!(fixture.requests.lock().unwrap().is_empty());
    assert!(!fixture.root.path().join("gallery").exists());
}

#[test]
fn native_integrity_and_unlisted_posting_fail_without_output() {
    for fault in ["digest", "foreign-artifact", "cost", "unlisted-correction"] {
        let fixture = Fixture::with_fault("gemini", false, false, fault);
        let result = fixture.command(HANDLE, "2k");
        assert!(!result.status.success(), "{fault} unexpectedly exported");
        assert!(result.stdout.is_empty());
        assert!(!fixture.root.path().join("gallery").exists());
    }
}

#[test]
fn native_jpeg_uses_matching_extension_and_original_bytes() {
    let fixture = Fixture::with_fault("gemini", false, false, "jpeg");
    let result = fixture.command(HANDLE, "2k");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let value: Value = serde_json::from_slice(&result.stdout).unwrap();
    let path = Path::new(value["images"][0]["path"].as_str().unwrap());
    assert_eq!(path.file_name().unwrap(), "01-gemini-2k-draft-0.1c.jpg");
    assert_eq!(std::fs::read(path).unwrap(), fixture.bytes);
}

#[test]
fn concurrent_native_replays_do_not_duplicate_manifest_or_sequences() {
    let fixture = Fixture::new("flux", false, true);
    thread::scope(|scope| {
        let workers: Vec<_> = (0..3)
            .map(|_| scope.spawn(|| fixture.command(HANDLE, "2k")))
            .collect();
        for worker in workers {
            let result = worker.join().unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
    });
    let manifest: Value = serde_json::from_slice(
        &std::fs::read(fixture.root.path().join("gallery/.gallery.json")).unwrap(),
    )
    .unwrap();
    let entries = manifest.as_array().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["sequence"], 1);
    assert_eq!(entries[1]["sequence"], 2);
}
