use serde_json::Value;
use std::{
    collections::{HashMap, VecDeque},
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use hubu_unified_mcp::source_commit;

use super::fixtures::default_responses;

#[derive(Clone, Copy)]
pub enum BackendKind {
    Hubu,
    Gongbu,
}

#[derive(Clone, Debug)]
pub struct CapturedRequest {
    pub method: String,
    pub path: String,
    pub raw: String,
}

#[derive(Clone)]
pub(super) struct StubResponse {
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
}

impl StubResponse {
    pub(super) fn json(status: u16, body: Value) -> Self {
        Self {
            status,
            content_type: "application/json",
            body: serde_json::to_vec(&body).unwrap(),
        }
    }

    pub(super) fn raw(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self::bytes(status, "application/json", body)
    }

    pub(super) fn bytes(status: u16, content_type: &'static str, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            content_type,
            body: body.into(),
        }
    }
}

struct StubState {
    disconnect: bool,
    responses: HashMap<(String, String), StubResponse>,
    response_sequences: HashMap<(String, String), VecDeque<StubResponse>>,
    requests: Vec<CapturedRequest>,
}

pub struct BackendStub {
    endpoint: String,
    state: Arc<Mutex<StubState>>,
    stop: Arc<AtomicBool>,
    workers: Arc<Mutex<Vec<JoinHandle<()>>>>,
    thread: Option<JoinHandle<()>>,
}

impl BackendStub {
    pub fn start(kind: BackendKind) -> Self {
        assert_ne!(
            source_commit(),
            "unknown",
            "run this matrix through scripts/integration-unified-mcp.sh"
        );
        assert_eq!(
            source_commit().len(),
            40,
            "test source stamp must be a full commit-shaped value"
        );

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(StubState {
            disconnect: false,
            responses: default_responses(kind),
            response_sequences: HashMap::new(),
            requests: Vec::new(),
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let workers = Arc::new(Mutex::new(Vec::new()));
        let thread_state = state.clone();
        let thread_stop = stop.clone();
        let thread_workers = workers.clone();
        let (ready_tx, ready_rx) = mpsc::sync_channel(0);
        let thread = thread::spawn(move || {
            ready_tx.send(()).unwrap();
            while !thread_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        if thread_stop.load(Ordering::Relaxed) {
                            break;
                        }
                        let connection_state = thread_state.clone();
                        thread_workers
                            .lock()
                            .unwrap()
                            .push(thread::spawn(move || serve(stream, &connection_state)));
                    }
                    Err(error) => panic!("backend stub accept failed: {error}"),
                }
            }
        });
        ready_rx.recv().unwrap();
        Self {
            endpoint,
            state,
            stop,
            workers,
            thread: Some(thread),
        }
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn disconnect(&self, disconnect: bool) {
        self.state.lock().unwrap().disconnect = disconnect;
    }

    pub fn respond_json(&self, method: &str, path: &str, status: u16, body: Value) {
        self.respond(method, path, StubResponse::json(status, body));
    }

    pub fn respond_raw(&self, method: &str, path: &str, status: u16, body: &str) {
        self.respond(method, path, StubResponse::raw(status, body.as_bytes()));
    }

    #[allow(dead_code)]
    pub fn respond_bytes(
        &self,
        method: &str,
        path: &str,
        status: u16,
        content_type: &'static str,
        body: impl Into<Vec<u8>>,
    ) {
        self.respond(
            method,
            path,
            StubResponse::bytes(status, content_type, body),
        );
    }

    #[allow(dead_code)]
    pub fn respond_sequence_json(
        &self,
        method: &str,
        path: &str,
        responses: impl IntoIterator<Item = (u16, Value)>,
    ) {
        let responses = responses
            .into_iter()
            .map(|(status, body)| StubResponse::json(status, body))
            .collect::<VecDeque<_>>();
        assert!(!responses.is_empty(), "response sequence must not be empty");
        self.state
            .lock()
            .unwrap()
            .response_sequences
            .insert((method.to_owned(), path.to_owned()), responses);
    }

    fn respond(&self, method: &str, path: &str, response: StubResponse) {
        self.state
            .lock()
            .unwrap()
            .responses
            .insert((method.to_owned(), path.to_owned()), response);
    }

    pub fn requests(&self) -> Vec<CapturedRequest> {
        self.state.lock().unwrap().requests.clone()
    }

    pub fn request_count(&self, method: &str, path: &str) -> usize {
        self.requests()
            .iter()
            .filter(|request| request.method == method && request.path == path)
            .count()
    }
}

impl Drop for BackendStub {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(self.endpoint.trim_start_matches("http://"));
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
        for worker in self.workers.lock().unwrap().drain(..) {
            worker.join().unwrap();
        }
    }
}

fn serve(mut stream: TcpStream, state: &Arc<Mutex<StubState>>) {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let Some(raw) = read_request(&mut stream) else {
        return;
    };
    let request_line = raw.lines().next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let path = parts.next().unwrap_or_default().to_owned();
    let response = {
        let mut state = state.lock().unwrap();
        state.requests.push(CapturedRequest {
            method: method.clone(),
            path: path.clone(),
            raw,
        });
        if state.disconnect {
            None
        } else {
            let key = (method, path);
            if let Some(sequence) = state.response_sequences.get_mut(&key) {
                let response = sequence.pop_front();
                if sequence.is_empty() {
                    state.response_sequences.remove(&key);
                }
                response
            } else {
                state.responses.get(&key).cloned()
            }
        }
    };
    let Some(response) = response else {
        return;
    };
    let reason = match response.status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        409 => "Conflict",
        500 => "Internal Server Error",
        _ => "Test Response",
    };
    if write!(
        stream,
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        response.status,
        reason,
        response.content_type,
        response.body.len()
    )
    .is_err()
    {
        return;
    }
    if stream.write_all(&response.body).is_ok() {
        let _ = stream.flush();
        let _ = stream.shutdown(std::net::Shutdown::Write);
    }
}

fn read_request(stream: &mut TcpStream) -> Option<String> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    let header_end = loop {
        let read = match stream.read(&mut buffer) {
            Ok(0) | Err(_) => return None,
            Ok(read) => read,
        };
        bytes.extend_from_slice(&buffer[..read]);
        if let Some(index) = find_bytes(&bytes, b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8_lossy(&bytes[..header_end]);
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    while bytes.len() < header_end + content_length {
        let read = match stream.read(&mut buffer) {
            Ok(0) | Err(_) => return None,
            Ok(read) => read,
        };
        bytes.extend_from_slice(&buffer[..read]);
    }
    Some(String::from_utf8_lossy(&bytes[..header_end + content_length]).into_owned())
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
