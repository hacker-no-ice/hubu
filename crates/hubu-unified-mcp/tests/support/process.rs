use serde_json::{json, Value};
use std::{
    env,
    ffi::OsString,
    io::{BufRead, BufReader, Read, Write},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
};

use super::BackendStub;

#[allow(dead_code)]
const HUBU_TOKEN: &str = "hub107-hubu-standalone-credential";
#[allow(dead_code)]
const GONGBU_TOKEN: &str = "hub107-gongbu-standalone-credential";
const RECONCILIATION_TOKEN: &str = "hub107-reconciliation-capability";

pub struct McpProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    transcript: Vec<Value>,
}

impl McpProcess {
    pub fn start(hubu: Option<(&BackendStub, &str)>, gongbu: Option<(&BackendStub, &str)>) -> Self {
        let executable = env::var_os("HUBU_UNIFIED_MCP_CANARY_BIN")
            .unwrap_or_else(|| OsString::from(env!("CARGO_BIN_EXE_hubu-unified-mcp")));
        let mut command = Command::new(executable);
        command
            .env_remove("HUBU_UNIFIED_HUBU_ENDPOINT")
            .env_remove("HUBU_UNIFIED_HUBU_BEARER_TOKEN")
            .env_remove("HUBU_UNIFIED_GONGBU_ENDPOINT")
            .env_remove("HUBU_UNIFIED_GONGBU_BEARER_TOKEN")
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
            .env("HUBU_RECONCILIATION_TOKEN", RECONCILIATION_TOKEN);
        Self::spawn(command)
    }

    #[allow(dead_code)]
    pub fn start_standalone_hubu(hubu: &BackendStub) -> Self {
        let executable = env::var_os("HUBU_STANDALONE_MCP_CANARY_BIN")
            .expect("run the parity matrix through scripts/integration-unified-mcp.sh");
        let mut command = Command::new(executable);
        command
            .env("HUBU_URL", hubu.endpoint())
            .env("HUBU_AUTH_TOKEN", HUBU_TOKEN)
            .env("HUBU_MCP_TRUST_CLIENT_APPROVAL", "1")
            .env("HUBU_RECONCILIATION_TOKEN", RECONCILIATION_TOKEN);
        Self::spawn(command)
    }

    #[allow(dead_code)]
    pub fn start_standalone_gongbu(gongbu: &BackendStub) -> Self {
        let executable = env::var_os("GONGBU_STANDALONE_MCP_CANARY_BIN")
            .expect("run the parity matrix through scripts/integration-unified-mcp.sh");
        let mut command = Command::new(executable);
        command
            .env("GONGBU_MCP_ENDPOINT", gongbu.endpoint())
            .env("GONGBU_MCP_BEARER_TOKEN", GONGBU_TOKEN);
        Self::spawn(command)
    }

    fn spawn(mut command: Command) -> Self {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            stdin: Some(stdin),
            stdout,
            transcript: Vec::new(),
        }
    }

    pub fn request(&mut self, request: Value) -> Value {
        let stdin = self.stdin.as_mut().unwrap();
        serde_json::to_writer(&mut *stdin, &request).unwrap();
        stdin.write_all(b"\n").unwrap();
        stdin.flush().unwrap();
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        assert!(!line.is_empty(), "unified MCP exited without a response");
        let response: Value = serde_json::from_str(&line).unwrap();
        self.transcript.push(response.clone());
        response
    }

    pub fn initialize(&mut self) -> Value {
        self.request(json!({"jsonrpc":"2.0","id":1,"method":"initialize"}))
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
