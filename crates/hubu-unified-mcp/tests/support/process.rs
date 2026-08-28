use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    env,
    ffi::OsString,
    io::{BufRead, BufReader, Read, Write},
    path::Path,
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

use super::BackendStub;

const RECONCILIATION_TOKEN: &str = "hub107-reconciliation-capability";
const APPROVAL_TOKEN: &str = "hub107-approval-capability";

pub struct McpProcess {
    _state: Option<tempfile::TempDir>,
    state_path: PathBuf,
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: mpsc::Receiver<Value>,
    stdout_thread: Option<thread::JoinHandle<()>>,
    transcript: Vec<Value>,
    notifications: VecDeque<Value>,
}

impl McpProcess {
    pub fn start(hubu: Option<(&BackendStub, &str)>, gongbu: Option<(&BackendStub, &str)>) -> Self {
        let state = tempfile::tempdir().unwrap();
        let state_path = state.path().join("operations.sqlite3");
        Self::start_configured(hubu, gongbu, &state_path, Some(state))
    }

    pub fn start_with_operation_state(
        hubu: Option<(&BackendStub, &str)>,
        gongbu: Option<(&BackendStub, &str)>,
        operation_state_path: &Path,
    ) -> Self {
        Self::start_configured(hubu, gongbu, operation_state_path, None)
    }

    fn start_configured(
        hubu: Option<(&BackendStub, &str)>,
        gongbu: Option<(&BackendStub, &str)>,
        operation_state_path: &Path,
        state: Option<tempfile::TempDir>,
    ) -> Self {
        let executable = env::var_os("HUBU_UNIFIED_MCP_CANARY_BIN")
            .unwrap_or_else(|| OsString::from(env!("CARGO_BIN_EXE_hubu-unified-mcp")));
        let mut command = Command::new(executable);
        command
            .env_remove("HUBU_UNIFIED_HUBU_ENDPOINT")
            .env_remove("HUBU_UNIFIED_HUBU_BEARER_TOKEN")
            .env_remove("HUBU_UNIFIED_GONGBU_ENDPOINT")
            .env_remove("HUBU_UNIFIED_GONGBU_BEARER_TOKEN")
            .env_remove("HUBU_APPROVAL_TOKEN")
            .env_remove("HUBU_APPROVAL_TOKEN_FILE")
            .env("HUBU_UNIFIED_CAPABILITY_POLL_INTERVAL_MS", "1000")
            .env("HUBU_UNIFIED_OPERATION_TICK_MS", "10")
            .env("HUBU_UNIFIED_OPERATION_STATE_PATH", operation_state_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some((stub, token)) = hubu {
            command
                .env("HUBU_UNIFIED_HUBU_ENDPOINT", stub.endpoint())
                .env("HUBU_UNIFIED_HUBU_BEARER_TOKEN", token);
        }
        if let Some((stub, token)) = gongbu {
            command
                .env("HUBU_UNIFIED_GONGBU_ENDPOINT", stub.endpoint())
                .env("HUBU_UNIFIED_GONGBU_BEARER_TOKEN", token);
        }
        command
            .env("HUBU_MCP_TRUST_CLIENT_APPROVAL", "1")
            .env("HUBU_MCP_TRUST_SPEND_APPROVAL", "1")
            .env("HUBU_APPROVAL_TOKEN", APPROVAL_TOKEN)
            .env("HUBU_RECONCILIATION_TOKEN", RECONCILIATION_TOKEN);
        Self::spawn(command, state, operation_state_path.to_path_buf())
    }

    fn spawn(mut command: Command, state: Option<tempfile::TempDir>, state_path: PathBuf) -> Self {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let child_stdout = BufReader::new(child.stdout.take().unwrap());
        let (stdout_tx, stdout) = mpsc::channel();
        let stdout_thread = thread::spawn(move || {
            for line in child_stdout.lines() {
                let line = line.expect("unified MCP stdout read failed");
                let message = serde_json::from_str(&line).expect("unified MCP wrote invalid JSON");
                if stdout_tx.send(message).is_err() {
                    return;
                }
            }
        });
        Self {
            _state: state,
            state_path,
            child,
            stdin: Some(stdin),
            stdout,
            stdout_thread: Some(stdout_thread),
            transcript: Vec::new(),
            notifications: VecDeque::new(),
        }
    }

    pub fn operation_state_path(&self) -> &Path {
        &self.state_path
    }

    pub fn request(&mut self, request: Value) -> Value {
        let expected_id = request["id"].clone();
        let stdin = self.stdin.as_mut().unwrap();
        serde_json::to_writer(&mut *stdin, &request).unwrap();
        stdin.write_all(b"\n").unwrap();
        stdin.flush().unwrap();
        loop {
            let message = self
                .stdout
                .recv_timeout(Duration::from_secs(10))
                .expect("unified MCP exited or timed out without a response");
            self.transcript.push(message.clone());
            if message.get("method").and_then(Value::as_str)
                == Some("notifications/tools/list_changed")
            {
                self.notifications.push_back(message);
                continue;
            }
            assert_eq!(message.get("id"), Some(&expected_id));
            return message;
        }
    }

    pub fn initialize(&mut self) -> Value {
        self.initialize_protocol()
    }

    pub fn initialize_with_monitor(&mut self) -> Value {
        let response = self.initialize_protocol();
        self.send_notification("notifications/initialized");
        let barrier = self.request(json!({"jsonrpc":"2.0","id":0,"method":"ping"}));
        assert_eq!(barrier["result"], json!({}));
        response
    }

    pub fn initialize_protocol(&mut self) -> Value {
        self.request(json!({"jsonrpc":"2.0","id":1,"method":"initialize"}))
    }

    pub fn send_notification(&mut self, method: &str) {
        let stdin = self.stdin.as_mut().unwrap();
        serde_json::to_writer(&mut *stdin, &json!({"jsonrpc":"2.0","method":method})).unwrap();
        stdin.write_all(b"\n").unwrap();
        stdin.flush().unwrap();
    }

    pub fn notification(&mut self, timeout: Duration) -> Value {
        if let Some(notification) = self.notifications.pop_front() {
            return notification;
        }
        let message = self
            .stdout
            .recv_timeout(timeout)
            .expect("timed out waiting for tools/list_changed notification");
        self.transcript.push(message.clone());
        if message.get("method").and_then(Value::as_str) == Some("notifications/tools/list_changed")
        {
            return message;
        }
        panic!("unexpected MCP response while waiting for notification: {message}");
    }

    pub fn assert_no_notification(&mut self, timeout: Duration) {
        assert!(
            self.notifications.is_empty(),
            "unexpected queued notification"
        );
        match self.stdout.recv_timeout(timeout) {
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                panic!("unified MCP exited while checking duplicate suppression")
            }
            Ok(message) => {
                self.transcript.push(message.clone());
                panic!("unexpected MCP message: {message}");
            }
        }
    }

    pub fn list_tools(&mut self) -> Value {
        self.request(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}))
    }

    pub fn call(&mut self, id: u64, name: &str, arguments: Value) -> Value {
        self.request(json!({
            "jsonrpc":"2.0",
            "id":id,
            "method":"tools/call",
            "params":{"name":name,"arguments":arguments}
        }))
    }

    pub fn call_with_meta(&mut self, id: u64, name: &str, arguments: Value, meta: Value) -> Value {
        self.request(json!({
            "jsonrpc":"2.0",
            "id":id,
            "method":"tools/call",
            "params":{"name":name,"arguments":arguments,"_meta":meta}
        }))
    }

    pub fn finish(mut self, secret_canaries: &[&str]) {
        drop(self.stdin.take());
        let status = self.child.wait().unwrap();
        if let Some(stdout_thread) = self.stdout_thread.take() {
            stdout_thread.join().unwrap();
        }
        while let Ok(message) = self.stdout.try_recv() {
            self.transcript.push(message);
        }
        let mut stderr = String::new();
        self.child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut stderr)
            .unwrap();
        assert!(status.success(), "unified MCP stderr: {stderr}");
        let public_output = format!(
            "{}\n{stderr}",
            serde_json::to_string(&self.transcript).unwrap()
        );
        for secret in secret_canaries {
            assert!(
                !public_output.contains(secret),
                "secret canary leaked into public MCP output: {secret}"
            );
        }
    }
}

impl Drop for McpProcess {
    fn drop(&mut self) {
        if self.stdin.is_some() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}
