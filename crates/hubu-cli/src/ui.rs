//! `hubu ui`: a loopback-only admin web UI.
//!
//! The Hubu server deliberately refuses browser-origin requests, so the browser
//! never talks to it directly. This process serves the static UI, holds the
//! Hubu bearer and approval credentials itself, and relays an allow-listed set
//! of admin routes. A browser obtains a session only by opening the one-time
//! login link printed to the operator's terminal.
//!
//! Guards on every API request: loopback `Host` matching this listener (DNS
//! rebinding), an `HttpOnly; SameSite=Strict` session cookie, a custom
//! `X-Hubu-UI` header (forces a CORS preflight that is never approved), a
//! same-origin `Origin`/`Sec-Fetch-Site` when present, and JSON bodies for
//! writes. Routes that use the human approval capability additionally require
//! an explicit `X-Hubu-UI-Confirmed` header sent only from a confirmation step.

use std::{
    collections::HashMap,
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
    time::Duration,
};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{ensure_no_args, take_help, take_value, CliContext};

const DEFAULT_LISTEN: &str = "127.0.0.1:8790";
const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_BODY_BYTES: usize = 256 * 1024;
const MAX_QUERY_BYTES: usize = 2048;
const MAX_CONNECTIONS: usize = 32;
const READ_TIMEOUT: Duration = Duration::from_secs(10);
const GONGBU_TIMEOUT: Duration = Duration::from_secs(5);
const UI_HEADER: &str = "x-hubu-ui";
const CONFIRM_HEADER: &str = "x-hubu-ui-confirmed";

const INDEX_HTML: &str = include_str!("ui/assets/index.html");
const APP_JS: &str = include_str!("ui/assets/app.js");
const APP_CSS: &str = include_str!("ui/assets/app.css");
/// The same wordmark hubustack.dev serves, so the admin UI stays on-brand.
const WORDMARK_SVG: &str = include_str!("../../../site/public/brand/hubu-wordmark.svg");

const CONTENT_SECURITY_POLICY: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'";

pub(crate) fn command(client: &CliContext, hubu_home: &Path, mut args: Vec<String>) -> Result<()> {
    if take_help(&mut args) {
        print_help();
        return Ok(());
    }
    let listen = take_value(&mut args, "--listen").unwrap_or_else(|| DEFAULT_LISTEN.to_string());
    let gongbu_endpoint = take_value(&mut args, "--gongbu-endpoint");
    let gongbu_token_file = take_value(&mut args, "--gongbu-token-file").map(PathBuf::from);
    ensure_no_args(args)?;

    let listen = parse_loopback_listen(&listen)?;
    let gongbu = resolve_gongbu(hubu_home, gongbu_endpoint, gongbu_token_file)?;
    let listener =
        TcpListener::bind(listen).with_context(|| format!("bind admin UI listener {listen}"))?;
    // Advertise and guard the address actually bound (port 0 picks one).
    let bound = listener.local_addr()?;
    let backend = LiveBackend {
        client,
        hubu_home: hubu_home.to_path_buf(),
        gongbu,
    };
    let (state, login_code) = UiState::new(bound, backend);

    let style = super::terminal::stdout();
    println!("{}", style.success("Hubu admin UI is running"));
    println!("  listening: http://{}", host_for(bound));
    println!(
        "  hubu: {}",
        client
            .target()
            .map(|target| target.base_url.clone())
            .unwrap_or_else(|error| format!("unavailable ({error:#})"))
    );
    println!("  open this one-time login link in your browser:");
    println!("    http://{}/login?code={login_code}", host_for(bound));
    println!("  the link works once; restart `hubu ui` to get a new one. Press Ctrl-C to stop.");

    serve(listener, &state)
}

fn print_help() {
    println!(
        "Run the local Hubu admin web UI

Usage:
  hubu ui [--listen 127.0.0.1:PORT] [--gongbu-endpoint URL --gongbu-token-file PATH]

Options:
  --listen ADDR             Loopback address to serve on (default: {DEFAULT_LISTEN}; port 0 picks one)
  --gongbu-endpoint URL     Gongbu endpoint for the live execution-target view
                            (default: the active stack profile's handoff)
  --gongbu-token-file PATH  Gongbu caller token file used with --gongbu-endpoint

The UI manages users' spending targets, agents, policies, budgets and spend
approvals, and shows spend history, decisions, provider revisions and secret
reference names. It never displays secret values. It uses the same Hubu
connection and credentials as other CLI commands. Approving spend or renaming an
agent uses the human approval capability and asks for confirmation in the UI.

Open the one-time login link printed at startup. The UI accepts only loopback
connections and refuses requests from other browser origins."
    );
}

fn parse_loopback_listen(value: &str) -> Result<SocketAddr> {
    let address: SocketAddr = value
        .parse()
        .with_context(|| format!("--listen must be an IP:PORT address, got `{value}`"))?;
    if !address.ip().is_loopback() {
        bail!("--listen must be a loopback address such as 127.0.0.1:8790; the admin UI is never exposed to the network");
    }
    Ok(address)
}

fn host_for(address: SocketAddr) -> String {
    match address {
        SocketAddr::V4(v4) => format!("{}:{}", v4.ip(), v4.port()),
        SocketAddr::V6(v6) => format!("[{}]:{}", v6.ip(), v6.port()),
    }
}

#[derive(Clone, Debug)]
struct GongbuTarget {
    endpoint: String,
    token_file: PathBuf,
}

fn resolve_gongbu(
    hubu_home: &Path,
    endpoint: Option<String>,
    token_file: Option<PathBuf>,
) -> Result<Option<GongbuTarget>> {
    match (endpoint, token_file) {
        (Some(endpoint), Some(token_file)) => Ok(Some(GongbuTarget {
            endpoint,
            token_file,
        })),
        (None, None) => {
            // A missing or broken stack profile only disables the live
            // Gongbu view; Hubu admin still works in manual mode.
            let handoff = super::stack::active_client_handoff(hubu_home)
                .ok()
                .flatten();
            Ok(handoff.and_then(|handoff| {
                Some(GongbuTarget {
                    endpoint: handoff.gongbu_endpoint?,
                    token_file: handoff.gongbu_token_file?,
                })
            }))
        }
        _ => bail!("--gongbu-endpoint and --gongbu-token-file must be provided together"),
    }
}

/// Everything the UI needs from outside this process, so routing and guards
/// can be tested without live Hubu or Gongbu servers.
trait Backend: Sync {
    fn hubu(
        &self,
        method: &str,
        path_and_query: &str,
        body: Option<Value>,
        approval: bool,
    ) -> Result<(u16, Value)>;
    fn hubu_endpoint(&self) -> String;
    fn approval_capability_available(&self) -> bool;
    fn providers(&self) -> Value;
    /// Prepare the canonical registration envelope exactly as
    /// `hubu register agent` does, and submit it only when `submit` is true.
    fn register_agent(
        &self,
        name: Option<String>,
        version: String,
        submit: bool,
    ) -> Result<(u16, Value)>;
}

struct LiveBackend<'a> {
    client: &'a CliContext,
    hubu_home: PathBuf,
    gongbu: Option<GongbuTarget>,
}

impl Backend for LiveBackend<'_> {
    fn hubu(
        &self,
        method: &str,
        path_and_query: &str,
        body: Option<Value>,
        approval: bool,
    ) -> Result<(u16, Value)> {
        super::request_json_status(self.client, method, path_and_query, body, approval, false)
    }

    fn hubu_endpoint(&self) -> String {
        self.client
            .target()
            .map(|target| target.base_url.clone())
            .unwrap_or_default()
    }

    fn approval_capability_available(&self) -> bool {
        self.client
            .target()
            .and_then(|target| target.approval_token())
            .is_ok_and(|token| token.is_some())
    }

    fn providers(&self) -> Value {
        let (stack, stack_error) = match super::stack::active_provider_inventory(&self.hubu_home) {
            Ok(inventory) => (inventory.unwrap_or(Value::Null), Value::Null),
            Err(error) => (Value::Null, json!(format!("{error:#}"))),
        };
        let gongbu = match &self.gongbu {
            None => json!({ "configured": false }),
            Some(target) => match gongbu_execution_targets(target) {
                Ok(targets) => json!({
                    "configured": true,
                    "endpoint": target.endpoint,
                    "targets": targets,
                }),
                Err(error) => json!({
                    "configured": true,
                    "endpoint": target.endpoint,
                    "error": format!("{error:#}"),
                }),
            },
        };
        json!({ "stack": stack, "stack_error": stack_error, "gongbu": gongbu })
    }

    fn register_agent(
        &self,
        name: Option<String>,
        version: String,
        submit: bool,
    ) -> Result<(u16, Value)> {
        let prepared = super::build_registration_envelope(self.client, name, &version)?;
        let review = prepared
            .review
            .iter()
            .map(|(field, value)| json!({ "field": field, "value": value }))
            .collect::<Vec<_>>();
        if !submit {
            return Ok((
                200,
                json!({ "review": review, "envelope": prepared.envelope }),
            ));
        }
        let (status, mut response) = super::request_json_status(
            self.client,
            "POST",
            "/agents/register",
            Some(prepared.envelope),
            false,
            false,
        )?;
        if let Value::Object(fields) = &mut response {
            fields.insert("review".into(), Value::Array(review));
        }
        Ok((status, response))
    }
}

/// Gongbu's sanitized execution-target catalog over its versioned HTTP API.
fn gongbu_execution_targets(target: &GongbuTarget) -> Result<Value> {
    let token = std::fs::read_to_string(&target.token_file)
        .with_context(|| "read Gongbu caller token file")?;
    let token = token.trim();
    if token.is_empty() {
        bail!("Gongbu caller token file is empty");
    }
    let url = format!(
        "{}/v2/execution-targets",
        target.endpoint.trim_end_matches('/')
    );
    let response = reqwest::blocking::Client::builder()
        .timeout(GONGBU_TIMEOUT)
        .build()?
        .get(&url)
        .bearer_auth(token)
        .send()
        .with_context(|| format!("connect to Gongbu at {}", target.endpoint))?;
    let status = response.status();
    let body: Value = response.json().context("parse Gongbu response")?;
    if !status.is_success() {
        bail!("Gongbu returned HTTP {}", status.as_u16());
    }
    Ok(body.get("targets").cloned().unwrap_or(Value::Null))
}

struct UiState<B: Backend> {
    bound: SocketAddr,
    backend: B,
    login_code_digest: Mutex<Option<[u8; 32]>>,
    session_token: String,
    session_digest: [u8; 32],
    cookie_name: String,
    connections: AtomicUsize,
}

impl<B: Backend> UiState<B> {
    fn new(bound: SocketAddr, backend: B) -> (Self, String) {
        let login_code = random_token();
        let session_token = random_token();
        let state = Self {
            bound,
            backend,
            login_code_digest: Mutex::new(Some(sha256(&login_code))),
            session_digest: sha256(&session_token),
            session_token,
            cookie_name: format!("hubu_ui_session_{}", bound.port()),
            connections: AtomicUsize::new(0),
        };
        (state, login_code)
    }

    /// The exact authority of the bound loopback address, plus `localhost`
    /// when the listener is on the address `localhost` resolves to.
    fn allowed_hosts(&self) -> Vec<String> {
        let mut hosts = vec![host_for(self.bound)];
        let ip = self.bound.ip();
        if ip == std::net::Ipv4Addr::LOCALHOST || ip == std::net::Ipv6Addr::LOCALHOST {
            hosts.push(format!("localhost:{}", self.bound.port()));
        }
        hosts
    }
}

fn random_token() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

fn sha256(value: &str) -> [u8; 32] {
    Sha256::digest(value.as_bytes()).into()
}

fn digests_equal(left: &[u8; 32], right: &[u8; 32]) -> bool {
    left.iter()
        .zip(right.iter())
        .fold(0_u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

fn serve<B: Backend>(listener: TcpListener, state: &UiState<B>) -> Result<()> {
    std::thread::scope(|scope| {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            if state.connections.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                state.connections.fetch_sub(1, Ordering::SeqCst);
                let _ = write_response(
                    &mut stream,
                    error_response(503, "too many concurrent admin UI connections"),
                );
                continue;
            }
            scope.spawn(move || {
                let response = match read_request(&mut stream) {
                    Ok(Some(request)) => handle(&request, state),
                    Ok(None) => {
                        state.connections.fetch_sub(1, Ordering::SeqCst);
                        return;
                    }
                    Err(error) => error_response(400, &format!("{error:#}")),
                };
                let _ = write_response(&mut stream, response);
                state.connections.fetch_sub(1, Ordering::SeqCst);
            });
        }
    });
    Ok(())
}

#[derive(Debug, Default)]
struct Request {
    method: String,
    path: String,
    query: Option<String>,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }

    fn query_param(&self, name: &str) -> Option<&str> {
        self.query.as_deref()?.split('&').find_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            (key == name).then_some(value)
        })
    }
}

fn read_request(stream: &mut TcpStream) -> Result<Option<Request>> {
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    let mut raw = Vec::new();
    let mut chunk = [0_u8; 8192];
    let header_end = loop {
        if let Some(position) = raw.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
        if raw.len() > MAX_HEADER_BYTES {
            bail!("request headers are too large");
        }
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            if raw.is_empty() {
                return Ok(None);
            }
            bail!("incomplete request headers");
        }
        raw.extend_from_slice(&chunk[..read]);
    };
    let mut request = parse_head(&raw[..header_end - 4])?;
    let content_length = match request.header("content-length") {
        Some(value) => value
            .trim()
            .parse::<usize>()
            .context("invalid Content-Length")?,
        None => 0,
    };
    if content_length > MAX_BODY_BYTES {
        bail!("request body is too large");
    }
    let mut body = raw[header_end..].to_vec();
    while body.len() < content_length {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            bail!("incomplete request body");
        }
        body.extend_from_slice(&chunk[..read]);
    }
    body.truncate(content_length);
    request.body = body;
    Ok(Some(request))
}

fn parse_head(head: &[u8]) -> Result<Request> {
    let head = std::str::from_utf8(head).context("request headers are not UTF-8")?;
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        bail!("malformed request line");
    };
    if !version.starts_with("HTTP/1.") || !target.starts_with('/') {
        bail!("malformed request line");
    }
    let (path, query) = match target.split_once('?') {
        Some((path, query)) => (path, Some(query.to_string())),
        None => (target, None),
    };
    let mut headers = HashMap::new();
    for line in lines {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| anyhow!("malformed header"))?;
        let name = name.trim().to_ascii_lowercase();
        if name == "transfer-encoding" {
            bail!("Transfer-Encoding is not supported");
        }
        if headers
            .insert(name.clone(), value.trim().to_string())
            .is_some()
            && matches!(
                name.as_str(),
                "host" | "content-length" | "cookie" | "origin"
            )
        {
            bail!("duplicate {name} header");
        }
    }
    Ok(Request {
        method: method.to_string(),
        path: path.to_string(),
        query,
        headers,
        body: Vec::new(),
    })
}

#[derive(Debug)]
struct Response {
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
    headers: Vec<(&'static str, String)>,
}

fn json_response(status: u16, body: &Value) -> Response {
    Response {
        status,
        content_type: "application/json",
        body: body.to_string().into_bytes(),
        headers: Vec::new(),
    }
}

fn error_response(status: u16, message: &str) -> Response {
    json_response(status, &json!({ "error": message }))
}

fn asset(content_type: &'static str, body: &str) -> Response {
    Response {
        status: 200,
        content_type,
        body: body.as_bytes().to_vec(),
        headers: Vec::new(),
    }
}

fn write_response(stream: &mut TcpStream, response: Response) -> std::io::Result<()> {
    let reason = match response.status {
        200 => "OK",
        303 => "See Other",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        415 => "Unsupported Media Type",
        428 => "Precondition Required",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Status",
    };
    let mut head = format!(
        "HTTP/1.1 {} {reason}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\nReferrer-Policy: no-referrer\r\nCross-Origin-Opener-Policy: same-origin\r\nCross-Origin-Resource-Policy: same-origin\r\nContent-Security-Policy: {CONTENT_SECURITY_POLICY}\r\n",
        response.status,
        response.content_type,
        response.body.len(),
    );
    for (name, value) in &response.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(&response.body)?;
    stream.flush()
}

fn handle<B: Backend>(request: &Request, state: &UiState<B>) -> Response {
    // DNS-rebinding defense: only answer for the exact loopback authority
    // this listener was bound to.
    let host_ok = request
        .header("host")
        .is_some_and(|host| state.allowed_hosts().iter().any(|allowed| allowed == host));
    if !host_ok {
        return error_response(403, "the admin UI only answers loopback Host headers");
    }

    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/" | "/index.html") => asset("text/html; charset=utf-8", INDEX_HTML),
        ("GET", "/app.js") => asset("text/javascript; charset=utf-8", APP_JS),
        ("GET", "/app.css") => asset("text/css; charset=utf-8", APP_CSS),
        ("GET", "/hubu-wordmark.svg") => asset("image/svg+xml", WORDMARK_SVG),
        ("GET", "/login") => login(request, state),
        (_, path) if path == "/api" || path.starts_with("/api/") => {
            if let Err(response) = guard_api(request, state) {
                return response;
            }
            api(request, state)
        }
        _ => error_response(404, "not found"),
    }
}

fn login<B: Backend>(request: &Request, state: &UiState<B>) -> Response {
    let Some(code) = request.query_param("code") else {
        return error_response(400, "missing login code");
    };
    let candidate = sha256(code);
    let mut stored = match state.login_code_digest.lock() {
        Ok(stored) => stored,
        Err(_) => return error_response(503, "login state unavailable"),
    };
    let valid = stored
        .as_ref()
        .is_some_and(|expected| digests_equal(expected, &candidate));
    if !valid {
        return error_response(
            403,
            "this login link is invalid or was already used; restart `hubu ui` for a new link",
        );
    }
    *stored = None;
    Response {
        status: 303,
        content_type: "text/plain; charset=utf-8",
        body: b"signed in".to_vec(),
        headers: vec![
            ("Location", "/".to_string()),
            (
                "Set-Cookie",
                format!(
                    "{}={}; Path=/; HttpOnly; SameSite=Strict",
                    state.cookie_name, state.session_token
                ),
            ),
        ],
    }
}

fn guard_api<B: Backend>(request: &Request, state: &UiState<B>) -> Result<(), Response> {
    let session = request.header("cookie").and_then(|cookies| {
        cookies.split(';').find_map(|cookie| {
            let (name, value) = cookie.trim().split_once('=')?;
            (name == state.cookie_name).then_some(value)
        })
    });
    let authenticated =
        session.is_some_and(|value| digests_equal(&sha256(value), &state.session_digest));
    if !authenticated {
        return Err(error_response(
            401,
            "not signed in; open the one-time login link printed by `hubu ui`",
        ));
    }
    if request.header(UI_HEADER) != Some("1") {
        return Err(error_response(403, "missing admin UI request header"));
    }
    if let Some(origin) = request.header("origin") {
        let same_origin = state
            .allowed_hosts()
            .iter()
            .any(|host| origin == format!("http://{host}"));
        if !same_origin {
            return Err(error_response(403, "cross-origin requests are refused"));
        }
    }
    if let Some(site) = request.header("sec-fetch-site") {
        if site != "same-origin" {
            return Err(error_response(403, "cross-site requests are refused"));
        }
    }
    if request.method == "POST" {
        let json = request.header("content-type").is_some_and(|value| {
            value
                .split(';')
                .next()
                .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("application/json"))
        });
        if !json {
            return Err(error_response(
                415,
                "POST requests require Content-Type: application/json",
            ));
        }
    }
    Ok(())
}

fn api<B: Backend>(request: &Request, state: &UiState<B>) -> Response {
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/api/session") => json_response(
            200,
            &json!({
                "authenticated": true,
                "hubu_endpoint": state.backend.hubu_endpoint(),
                "approval_capability": state.backend.approval_capability_available(),
            }),
        ),
        ("GET", "/api/providers") => json_response(200, &state.backend.providers()),
        ("POST", "/api/agents/registration") => register_agent(request, state),
        ("POST", "/api/policies/validate") => policy_tool(request, PolicyTool::Validate),
        ("POST", "/api/policies/render") => policy_tool(request, PolicyTool::Render),
        (_, path) if path.starts_with("/api/hubu/") => proxy(request, state),
        _ => error_response(404, "unknown admin UI API route"),
    }
}

/// Human-in-the-loop agent registration: the human supplies only a name and a
/// version label; the client fills owner/runtime fields, canonicalizes and
/// fingerprints. `submit: false` returns the compact review without writing.
fn register_agent<B: Backend>(request: &Request, state: &UiState<B>) -> Response {
    let Ok(body) = serde_json::from_slice::<Value>(&request.body) else {
        return error_response(400, "request body must be a JSON object");
    };
    let text = |field: &str| -> Result<Option<String>, Response> {
        match body.get(field) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(value)) if value.trim().is_empty() => Ok(None),
            Some(Value::String(value)) if value.len() <= 128 => Ok(Some(value.trim().to_string())),
            _ => Err(error_response(
                400,
                &format!("`{field}` must be a string of at most 128 characters"),
            )),
        }
    };
    let name = match text("name") {
        Ok(name) => name,
        Err(response) => return response,
    };
    let version = match text("version_label") {
        Ok(Some(version)) => version,
        Ok(None) => return error_response(400, "`version_label` is required"),
        Err(response) => return response,
    };
    let submit = body.get("submit").and_then(Value::as_bool) == Some(true);
    match state.backend.register_agent(name, version, submit) {
        Ok((status, body)) => json_response(status, &body),
        Err(error) => error_response(502, &format!("agent registration failed: {error:#}")),
    }
}

#[derive(Clone, Copy)]
enum PolicyTool {
    /// Parse and validate author YAML with the same loader the server uses.
    Validate,
    /// Render a stored revision (policy JSON from history) as export YAML.
    Render,
}

/// Local policy tooling. Nothing here reaches Hubu or uses a credential: it
/// runs `hubu_core`'s loader, validator and describer on the request body.
fn policy_tool(request: &Request, tool: PolicyTool) -> Response {
    use hubu_core::policy::Policy;
    let Ok(body) = serde_json::from_slice::<Value>(&request.body) else {
        return error_response(400, "request body must be a JSON object");
    };
    let policy = match tool {
        PolicyTool::Validate => {
            let Some(yaml) = body.get("policy_yaml").and_then(Value::as_str) else {
                return error_response(400, "`policy_yaml` must be a string");
            };
            match Policy::from_yaml_str(yaml) {
                Ok(policy) => policy,
                Err(error) => {
                    return json_response(
                        200,
                        &json!({ "valid": false, "error": error.to_string() }),
                    )
                }
            }
        }
        PolicyTool::Render => {
            let Some(policy) = body.get("policy").cloned() else {
                return error_response(400, "`policy` is required");
            };
            match serde_json::from_value::<Policy>(policy) {
                Ok(policy) => policy,
                Err(error) => {
                    return error_response(
                        400,
                        &format!("policy revision is not readable: {error}"),
                    )
                }
            }
        }
    };
    let policy_yaml = match policy.to_yaml_string() {
        Ok(yaml) => yaml,
        Err(error) => return error_response(500, &format!("render policy YAML: {error}")),
    };
    json_response(
        200,
        &json!({
            "valid": true,
            "id": policy.id,
            "version": policy.version,
            "default_effect": policy.default_effect,
            "rules": policy.rules.iter().map(|rule| json!({
                "id": rule.id,
                "effect": rule.effect,
                "reason": rule.reason,
                "condition": rule.when.describe(),
            })).collect::<Vec<_>>(),
            "warnings": policy.amount_warnings(),
            "policy_yaml": policy_yaml,
        }),
    )
}

#[derive(Debug, PartialEq, Eq)]
enum RouteAccess {
    Standard,
    /// Sends the human approval capability; requires an explicit confirmation.
    HumanApproval,
}

/// The only Hubu routes the browser can reach. Anything else is refused here,
/// before a credential is attached.
fn allowed_route(method: &str, path: &str) -> Option<RouteAccess> {
    const GET_ROUTES: &[&str] = &[
        "/health",
        "/version",
        "/user",
        "/users",
        "/user/spending-target",
        "/registration/guidance",
        "/agents",
        "/agents/history",
        "/policies",
        "/policies/show",
        "/policies/export",
        "/policies/history",
        "/policies/diff",
        "/budgets",
        "/spend/approval",
        "/spend/authorizations",
        "/spend/authorizations/show",
        "/ledger/transactions",
        "/governance/summary",
        "/governance/records",
    ];
    const POST_ROUTES: &[&str] = &[
        "/init",
        "/policies",
        "/budgets",
        "/budgets/revoke",
        "/user/spending-target",
        "/user/spending-target/revoke",
    ];
    const APPROVAL_ROUTES: &[&str] = &["/agents/rename", "/spend/approval/resolve"];

    let budget_versions = path
        .strip_prefix("/budgets/")
        .and_then(|rest| rest.strip_suffix("/versions"))
        .is_some_and(safe_identifier);
    let governance_record = path
        .strip_prefix("/governance/records/")
        .is_some_and(safe_identifier);
    match method {
        "GET" if GET_ROUTES.contains(&path) || budget_versions || governance_record => {
            Some(RouteAccess::Standard)
        }
        "POST" if POST_ROUTES.contains(&path) || budget_versions => Some(RouteAccess::Standard),
        "POST" if APPROVAL_ROUTES.contains(&path) => Some(RouteAccess::HumanApproval),
        _ => None,
    }
}

fn safe_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b':'))
}

/// Queries are relayed verbatim onto the Hubu request line, so only
/// URL-encoded characters may pass (no spaces, CR/LF or raw delimiters).
fn safe_query(query: &str) -> bool {
    query.len() <= MAX_QUERY_BYTES
        && query.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'-' | b'.' | b'_' | b'~' | b'%' | b'&' | b'=' | b'+' | b',' | b':'
                )
        })
}

fn proxy<B: Backend>(request: &Request, state: &UiState<B>) -> Response {
    let path = &request.path["/api/hubu".len()..];
    let Some(access) = allowed_route(&request.method, path) else {
        return error_response(
            404,
            &format!("{} {path} is not available in the admin UI", request.method),
        );
    };
    if access == RouteAccess::HumanApproval && request.header(CONFIRM_HEADER) != Some("1") {
        return error_response(
            428,
            "this action uses the human approval capability and must be confirmed in the UI",
        );
    }
    let mut target = path.to_string();
    if let Some(query) = request.query.as_deref().filter(|query| !query.is_empty()) {
        if !safe_query(query) {
            return error_response(400, "query string contains unsupported characters");
        }
        target.push('?');
        target.push_str(query);
    }
    let body = if request.method == "POST" {
        match serde_json::from_slice::<Value>(&request.body) {
            Ok(body @ Value::Object(_)) => Some(body),
            _ => return error_response(400, "request body must be a JSON object"),
        }
    } else {
        None
    };
    match state.backend.hubu(
        &request.method,
        &target,
        body,
        access == RouteAccess::HumanApproval,
    ) {
        Ok((status, body)) => json_response(status, &body),
        Err(error) => error_response(502, &format!("Hubu request failed: {error:#}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    /// (method, path and query, body, carries approval capability or submits)
    type Call = (String, String, Option<Value>, bool);

    #[derive(Default)]
    struct FakeBackend {
        calls: StdMutex<Vec<Call>>,
    }

    impl Backend for FakeBackend {
        fn hubu(
            &self,
            method: &str,
            path_and_query: &str,
            body: Option<Value>,
            approval: bool,
        ) -> Result<(u16, Value)> {
            self.calls.lock().unwrap().push((
                method.to_string(),
                path_and_query.to_string(),
                body,
                approval,
            ));
            Ok((200, json!({ "ok": true })))
        }
        fn hubu_endpoint(&self) -> String {
            "http://127.0.0.1:8787".into()
        }
        fn approval_capability_available(&self) -> bool {
            true
        }
        fn providers(&self) -> Value {
            json!({ "stack": null })
        }
        fn register_agent(
            &self,
            name: Option<String>,
            version: String,
            submit: bool,
        ) -> Result<(u16, Value)> {
            self.calls.lock().unwrap().push((
                "REGISTER".into(),
                format!("{name:?}/{version}"),
                None,
                submit,
            ));
            Ok((200, json!({ "review": [] })))
        }
    }

    fn state() -> (UiState<FakeBackend>, String) {
        UiState::new("127.0.0.1:8790".parse().unwrap(), FakeBackend::default())
    }

    fn request(method: &str, target: &str) -> Request {
        let mut raw = format!("{method} {target} HTTP/1.1\r\nHost: 127.0.0.1:8790");
        raw.push_str("\r\n");
        let mut request = parse_head(raw.trim_end().as_bytes()).unwrap();
        request
            .headers
            .insert("host".into(), "127.0.0.1:8790".into());
        request
    }

    fn signed_in(state: &UiState<FakeBackend>, method: &str, target: &str) -> Request {
        let mut request = request(method, target);
        request.headers.insert(
            "cookie".into(),
            format!("other=1; {}={}", state.cookie_name, state.session_token),
        );
        request.headers.insert(UI_HEADER.into(), "1".into());
        if method == "POST" {
            request
                .headers
                .insert("content-type".into(), "application/json".into());
            request.body = b"{}".to_vec();
        }
        request
    }

    #[test]
    fn listen_address_must_be_loopback() {
        assert!(parse_loopback_listen("127.0.0.1:0").is_ok());
        assert!(parse_loopback_listen("[::1]:8790").is_ok());
        assert!(parse_loopback_listen("0.0.0.0:8790").is_err());
        assert!(parse_loopback_listen("192.168.1.2:8790").is_err());
        assert!(parse_loopback_listen("localhost:8790").is_err());
    }

    #[test]
    fn static_assets_are_served_with_strict_headers_but_need_matching_host() {
        let (state, _) = state();
        let response = handle(&request("GET", "/"), &state);
        assert_eq!(response.status, 200);
        assert!(String::from_utf8_lossy(&response.body).contains("app.js"));

        let mut rebound = request("GET", "/");
        rebound
            .headers
            .insert("host".into(), "attacker.example:8790".into());
        assert_eq!(handle(&rebound, &state).status, 403);
        let mut other_port = request("GET", "/");
        other_port
            .headers
            .insert("host".into(), "127.0.0.1:9999".into());
        assert_eq!(handle(&other_port, &state).status, 403);
    }

    #[test]
    fn host_guard_follows_the_bound_loopback_address() {
        let (state, _) = UiState::new("127.0.0.2:41001".parse().unwrap(), FakeBackend::default());
        assert_eq!(state.allowed_hosts(), vec!["127.0.0.2:41001".to_string()]);
        let mut ok = request("GET", "/");
        ok.headers.insert("host".into(), "127.0.0.2:41001".into());
        assert_eq!(handle(&ok, &state).status, 200);
        let mut other = request("GET", "/");
        other
            .headers
            .insert("host".into(), "127.0.0.1:41001".into());
        assert_eq!(handle(&other, &state).status, 403);

        let (v6, _) = UiState::new("[::1]:41002".parse().unwrap(), FakeBackend::default());
        assert_eq!(
            v6.allowed_hosts(),
            vec!["[::1]:41002".to_string(), "localhost:41002".to_string()]
        );
    }

    #[test]
    fn login_code_is_single_use_and_sets_strict_http_only_cookie() {
        let (state, code) = state();
        assert_eq!(
            handle(&request("GET", "/login?code=wrong"), &state).status,
            403
        );
        let response = handle(&request("GET", &format!("/login?code={code}")), &state);
        assert_eq!(response.status, 303);
        let cookie = response
            .headers
            .iter()
            .find(|(name, _)| *name == "Set-Cookie")
            .map(|(_, value)| value.clone())
            .unwrap();
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Strict"));
        assert!(cookie.starts_with(&format!("{}={}", state.cookie_name, state.session_token)));
        assert_eq!(
            handle(&request("GET", &format!("/login?code={code}")), &state).status,
            403
        );
    }

    #[test]
    fn api_requires_session_ui_header_and_same_origin() {
        let (state, _) = state();
        assert_eq!(handle(&request("GET", "/api/session"), &state).status, 401);

        let ok = signed_in(&state, "GET", "/api/session");
        assert_eq!(handle(&ok, &state).status, 200);

        let mut wrong_cookie = signed_in(&state, "GET", "/api/session");
        wrong_cookie
            .headers
            .insert("cookie".into(), format!("{}=forged", state.cookie_name));
        assert_eq!(handle(&wrong_cookie, &state).status, 401);

        let mut no_header = signed_in(&state, "GET", "/api/session");
        no_header.headers.remove(UI_HEADER);
        assert_eq!(handle(&no_header, &state).status, 403);

        let mut cross_origin = signed_in(&state, "GET", "/api/session");
        cross_origin
            .headers
            .insert("origin".into(), "http://evil.example".into());
        assert_eq!(handle(&cross_origin, &state).status, 403);

        let mut same_origin = signed_in(&state, "GET", "/api/session");
        same_origin
            .headers
            .insert("origin".into(), "http://127.0.0.1:8790".into());
        same_origin
            .headers
            .insert("sec-fetch-site".into(), "same-origin".into());
        assert_eq!(handle(&same_origin, &state).status, 200);

        let mut cross_site = signed_in(&state, "GET", "/api/session");
        cross_site
            .headers
            .insert("sec-fetch-site".into(), "cross-site".into());
        assert_eq!(handle(&cross_site, &state).status, 403);

        let mut form_post = signed_in(&state, "POST", "/api/hubu/budgets");
        form_post
            .headers
            .insert("content-type".into(), "text/plain".into());
        assert_eq!(handle(&form_post, &state).status, 415);
    }

    #[test]
    fn proxy_relays_only_allow_listed_routes() {
        let (state, _) = state();
        for (method, target) in [
            ("GET", "/api/hubu/agents?all=true"),
            ("GET", "/api/hubu/budgets/bgt_abc123/versions"),
            ("GET", "/api/hubu/governance/records/gov_1"),
            (
                "GET",
                "/api/hubu/governance/records?categories=spend,approval&limit=50",
            ),
            ("POST", "/api/hubu/budgets"),
            ("POST", "/api/hubu/budgets/bgt_abc123/versions"),
            ("POST", "/api/hubu/policies"),
        ] {
            let response = handle(&signed_in(&state, method, target), &state);
            assert_eq!(response.status, 200, "{method} {target}");
        }
        for (method, target) in [
            ("POST", "/api/hubu/spend"),
            ("POST", "/api/hubu/spend/authorize"),
            ("POST", "/api/hubu/spend/executor/claim"),
            ("POST", "/api/hubu/agents/register"),
            ("GET", "/api/hubu/budgets/../versions"),
            ("GET", "/api/hubu/governance/records/a/b"),
            ("DELETE", "/api/hubu/budgets"),
        ] {
            let response = handle(&signed_in(&state, method, target), &state);
            assert_eq!(response.status, 404, "{method} {target}");
        }
        let calls = state.backend.calls.lock().unwrap();
        assert_eq!(calls.len(), 7);
        assert_eq!(calls[0].1, "/agents?all=true");
        assert!(calls.iter().all(|call| !call.3));
    }

    #[test]
    fn proxy_rejects_request_line_injection_and_non_object_bodies() {
        let (state, _) = state();
        let mut injected = signed_in(&state, "GET", "/api/hubu/agents");
        injected.query = Some("all=true HTTP/1.1\r\nX-Evil: 1".into());
        assert_eq!(handle(&injected, &state).status, 400);

        let mut array_body = signed_in(&state, "POST", "/api/hubu/budgets");
        array_body.body = b"[1]".to_vec();
        assert_eq!(handle(&array_body, &state).status, 400);
        assert!(state.backend.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn approval_capability_routes_require_explicit_confirmation() {
        let (state, _) = state();
        let unconfirmed = signed_in(&state, "POST", "/api/hubu/spend/approval/resolve");
        assert_eq!(handle(&unconfirmed, &state).status, 428);
        assert!(state.backend.calls.lock().unwrap().is_empty());

        let mut confirmed = signed_in(&state, "POST", "/api/hubu/spend/approval/resolve");
        confirmed.headers.insert(CONFIRM_HEADER.into(), "1".into());
        assert_eq!(handle(&confirmed, &state).status, 200);

        // The confirmation header never upgrades a standard route.
        let mut budgets = signed_in(&state, "POST", "/api/hubu/budgets");
        budgets.headers.insert(CONFIRM_HEADER.into(), "1".into());
        assert_eq!(handle(&budgets, &state).status, 200);

        let calls = state.backend.calls.lock().unwrap();
        assert_eq!(calls[0].1, "/spend/approval/resolve");
        assert!(calls[0].3, "approval route must carry the capability");
        assert!(!calls[1].3, "standard route must not carry the capability");
    }

    #[test]
    fn agent_registration_needs_version_label_and_defaults_to_review_only() {
        let (state, _) = state();
        let mut missing = signed_in(&state, "POST", "/api/agents/registration");
        missing.body = br#"{"name":"Builder"}"#.to_vec();
        assert_eq!(handle(&missing, &state).status, 400);

        let mut review = signed_in(&state, "POST", "/api/agents/registration");
        review.body = br#"{"name":"Builder","version_label":"v1"}"#.to_vec();
        assert_eq!(handle(&review, &state).status, 200);

        let mut submit = signed_in(&state, "POST", "/api/agents/registration");
        submit.body = br#"{"name":" ","version_label":"v2","submit":true}"#.to_vec();
        assert_eq!(handle(&submit, &state).status, 200);

        let calls = state.backend.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].1, "Some(\"Builder\")/v1");
        assert!(!calls[0].3);
        assert_eq!(calls[1].1, "None/v2");
        assert!(calls[1].3);
    }

    #[test]
    fn decision_category_filter_matches_governance_categories() {
        use hubu_core::spend::governance_query::GovernanceCategory;
        let start = APP_JS
            .find("const CATEGORIES = [")
            .expect("CATEGORIES list");
        let list = &APP_JS[start..];
        let list = &list[list.find('[').unwrap() + 1..list.find("];").unwrap()];
        let ui: Vec<&str> = list
            .split(',')
            .map(|item| item.trim().trim_matches('"'))
            .filter(|item| !item.is_empty())
            .collect();
        for category in &ui {
            serde_json::from_value::<GovernanceCategory>(json!(category))
                .unwrap_or_else(|_| panic!("UI category `{category}` is not a GovernanceCategory"));
        }
        // Every server category is offered exactly once.
        let mut unique = ui.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), ui.len());
        assert_eq!(
            ui.len(),
            12,
            "update the UI list when GovernanceCategory changes"
        );
    }

    #[test]
    fn policy_tools_validate_and_render_without_calling_hubu() {
        let (state, _) = state();
        let yaml = "id: p\nversion: v1\ndefault_effect: needs_approval\nrules:\n  - id: small\n    effect: allow\n    reason: small spend\n    when:\n      op: lte\n      field: amount\n      value:\n        money: \"5.00\"\n";
        let mut valid = signed_in(&state, "POST", "/api/policies/validate");
        valid.body = serde_json::to_vec(&json!({ "policy_yaml": yaml })).unwrap();
        let response = handle(&valid, &state);
        assert_eq!(response.status, 200);
        let body: Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(body["valid"], true);
        assert_eq!(body["default_effect"], "needs_approval");
        assert_eq!(body["rules"][0]["id"], "small");
        assert!(body["rules"][0]["condition"]
            .as_str()
            .unwrap()
            .contains("amount"));

        let mut invalid = signed_in(&state, "POST", "/api/policies/validate");
        invalid.body = br#"{"policy_yaml":"id: p\nrules: nope"}"#.to_vec();
        let body: Value = serde_json::from_slice(&handle(&invalid, &state).body).unwrap();
        assert_eq!(body["valid"], false);
        assert!(body["error"].as_str().is_some());

        // A stored revision renders back to YAML that validates again.
        let stored = hubu_core::policy::Policy::from_yaml_str(yaml).unwrap();
        let mut render = signed_in(&state, "POST", "/api/policies/render");
        render.body = serde_json::to_vec(&json!({ "policy": stored })).unwrap();
        let body: Value = serde_json::from_slice(&handle(&render, &state).body).unwrap();
        let rendered = body["policy_yaml"].as_str().unwrap();
        assert!(hubu_core::policy::Policy::from_yaml_str(rendered).is_ok());
        assert!(state.backend.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn serves_the_site_wordmark() {
        let (state, _) = state();
        let response = handle(&request("GET", "/hubu-wordmark.svg"), &state);
        assert_eq!(response.status, 200);
        assert_eq!(response.content_type, "image/svg+xml");
        assert!(String::from_utf8_lossy(&response.body).contains("<svg"));
    }

    #[test]
    fn decision_outcome_filter_matches_governance_outcomes() {
        use hubu_core::spend::governance_query::GovernanceOutcome;
        let start = APP_JS.find("const OUTCOMES = [").expect("OUTCOMES list");
        let list = &APP_JS[start..];
        let list = &list[list.find('[').unwrap() + 1..list.find("];").unwrap()];
        let ui: Vec<&str> = list
            .split(',')
            .map(|item| item.trim().trim_matches('"'))
            .filter(|item| !item.is_empty())
            .collect();
        for outcome in &ui {
            serde_json::from_value::<GovernanceOutcome>(json!(outcome))
                .unwrap_or_else(|_| panic!("UI outcome `{outcome}` is not a GovernanceOutcome"));
        }
        assert_eq!(
            ui.len(),
            17,
            "update the UI list when GovernanceOutcome changes"
        );
        // The overview's denial tile links to a real outcome filter.
        assert!(APP_JS.contains("#/decisions?outcomes=automatic_deny"));
    }

    #[test]
    fn request_head_parser_rejects_ambiguous_framing() {
        assert!(parse_head(b"GET / HTTP/1.1\r\nTransfer-Encoding: chunked").is_err());
        assert!(parse_head(b"GET / HTTP/1.1\r\nHost: a\r\nHost: b").is_err());
        assert!(parse_head(b"GET http://x/ HTTP/1.1").is_err());
        assert!(parse_head(b"GET / HTTP/1.1 extra").is_err());
        let parsed = parse_head(b"GET /api/hubu/agents?all=true HTTP/1.1\r\nHost: x").unwrap();
        assert_eq!(parsed.path, "/api/hubu/agents");
        assert_eq!(parsed.query.as_deref(), Some("all=true"));
    }
}
