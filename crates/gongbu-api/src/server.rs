use std::{
    net::{TcpListener, TcpStream},
    sync::atomic::{AtomicU64, Ordering},
};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    config::Config,
    hubu::{
        ExecutorSpendRequest, ExecutorSpendResponse, ExecutorSpendSettlementResponse, HubuClient,
    },
    image_jobs::{create_image_job, image_job_guidance, ImageJobRequest},
    simple_http::{parse_request, read_request, write_response, HttpRequest, HttpResponse},
};

static NEXT_JOB_ID: AtomicU64 = AtomicU64::new(1);

pub fn run_server_from_env() -> Result<()> {
    run_server(Config::from_env()?)
}

pub fn run_server(config: Config) -> Result<()> {
    let listener = TcpListener::bind(&config.bind_addr)
        .with_context(|| format!("bind Gongbu server to {}", config.bind_addr))?;
    let state = ServerState::new(config);
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => handle_connection(stream, &state)?,
            Err(error) => return Err(error).context("accept HTTP connection"),
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct ServerState {
    config: Config,
    hubu: HubuClient,
}

impl ServerState {
    fn new(config: Config) -> Self {
        let hubu = HubuClient::new(config.hubu_base_url.clone());
        Self { config, hubu }
    }
}

fn handle_connection(mut stream: TcpStream, state: &ServerState) -> Result<()> {
    let raw = read_request(&mut stream)?;
    if raw.is_empty() {
        return Ok(());
    }
    let request = parse_request(&raw).context("parse request")?;
    let response = route(request, state);
    write_response(&mut stream, &response).context("write response")?;
    Ok(())
}

fn route(request: HttpRequest, state: &ServerState) -> HttpResponse {
    let result = match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/health") => Ok(health(state)),
        ("POST", "/mock-executor/dry-run") => mock_executor_dry_run(request.body, state),
        ("GET", "/image-jobs/guidance") => {
            to_json(image_job_guidance(&state.config.image_provider))
        }
        ("POST", "/image-jobs") => (|| {
            let request: ImageJobRequest =
                serde_json::from_str(&request.body).context("parse image job request")?;
            to_json(create_image_job(
                request,
                &state.hubu,
                &state.config.image_provider,
            )?)
        })(),
        _ => return HttpResponse::not_found(&request.method, &request.path),
    };

    match result {
        Ok(body) => HttpResponse::ok(body),
        Err(error) => HttpResponse::bad_request(error),
    }
}

fn health(state: &ServerState) -> Value {
    json!({
        "status": "ok",
        "service": "gongbu",
        "hubu_base_url": state.config.hubu_base_url,
        "spend_executor_protocol": "hubu-spend-executor-v1",
        "image_provider": state.config.image_provider.provider,
        "image_model": state.config.image_provider.model,
        "image_provider_ready": state.config.image_provider.readiness().ready,
    })
}

fn mock_executor_dry_run(body: String, state: &ServerState) -> Result<Value> {
    let request: MockExecutorDryRunRequest =
        serde_json::from_str(&body).context("parse mock executor dry-run request")?;
    request.validate()?;

    let spend_request = request.spend_request();
    let validation = state
        .hubu
        .validate(&spend_request)
        .context("validate Hubu spend authorization")?;

    let job_id = format!("dryrun-{}", NEXT_JOB_ID.fetch_add(1, Ordering::Relaxed));
    match request.outcome {
        DryRunOutcome::Success => {
            let settlement = state
                .hubu
                .settle(&spend_request)
                .context("settle Hubu spend authorization")?;
            Ok(to_json(MockExecutorDryRunResponse {
                job_id,
                status: "settled".to_string(),
                validation,
                closure: SpendClosure::Settlement(settlement),
            })?)
        }
        DryRunOutcome::PreWorkFailure => {
            let release = state
                .hubu
                .release(&spend_request)
                .context("release Hubu spend authorization")?;
            Ok(to_json(MockExecutorDryRunResponse {
                job_id,
                status: "released".to_string(),
                validation,
                closure: SpendClosure::Release(release),
            })?)
        }
    }
}

fn to_json<T: Serialize>(value: T) -> Result<Value> {
    serde_json::to_value(value).context("serialize response")
}

#[derive(Debug, Deserialize)]
struct MockExecutorDryRunRequest {
    spend_auth_token_id: String,
    agent_id: Option<String>,
    account_id: Option<String>,
    amount_cents: i64,
    merchant: Option<String>,
    task_id: Option<String>,
    #[serde(default)]
    outcome: DryRunOutcome,
}

impl MockExecutorDryRunRequest {
    fn validate(&self) -> Result<()> {
        match (&self.agent_id, &self.account_id) {
            (Some(_), None) | (None, Some(_)) => {}
            _ => {
                return Err(anyhow!(
                    "request must include exactly one of agent_id or account_id"
                ))
            }
        }
        if self.spend_auth_token_id.trim().is_empty() {
            return Err(anyhow!("spend_auth_token_id is required"));
        }
        if self.amount_cents <= 0 {
            return Err(anyhow!("amount_cents must be positive"));
        }
        Ok(())
    }

    fn spend_request(&self) -> ExecutorSpendRequest {
        ExecutorSpendRequest {
            spend_auth_token_id: self.spend_auth_token_id.clone(),
            agent_id: self.agent_id.clone(),
            account_id: self.account_id.clone(),
            amount_cents: self.amount_cents,
            merchant: self.merchant.clone(),
            task_id: self.task_id.clone(),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DryRunOutcome {
    #[default]
    Success,
    PreWorkFailure,
}

#[derive(Debug, Serialize)]
struct MockExecutorDryRunResponse {
    job_id: String,
    status: String,
    validation: ExecutorSpendResponse,
    closure: SpendClosure,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum SpendClosure {
    Settlement(ExecutorSpendSettlementResponse),
    Release(ExecutorSpendResponse),
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        sync::{Arc, Mutex},
        thread,
    };

    use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
    use serde_json::Value;

    use super::*;
    use crate::hubu::BudgetHold;
    use crate::image_provider::{
        HttpJsonImageProviderFields, ImageProviderAdapterKind, ImageProviderConfig,
    };

    #[test]
    fn health_reports_contract_and_hubu_url() {
        let state = ServerState::new(Config {
            bind_addr: "127.0.0.1:0".to_string(),
            hubu_base_url: "http://127.0.0.1:8787".to_string(),
            image_provider: mock_provider_config(std::env::temp_dir()),
        });
        let response = route(
            HttpRequest {
                method: "GET".to_string(),
                path: "/health".to_string(),
                body: String::new(),
            },
            &state,
        );

        assert_eq!(response.status, 200);
        assert_eq!(response.body["status"], "ok");
        assert_eq!(
            response.body["spend_executor_protocol"],
            "hubu-spend-executor-v1"
        );
    }

    #[test]
    fn dry_run_success_validates_then_settles() {
        let fake = FakeHubu::start(vec![
            fake_response("/spend/executor/validate", spend_response("frozen")),
            fake_response(
                "/spend/executor/settle",
                json!({
                    "settlement_id": "settlement-1",
                    "spend": spend_response("settled"),
                }),
            ),
        ]);
        let state = ServerState::new(Config {
            bind_addr: "127.0.0.1:0".to_string(),
            hubu_base_url: fake.base_url.clone(),
            image_provider: mock_provider_config(std::env::temp_dir()),
        });

        let response = route(
            HttpRequest {
                method: "POST".to_string(),
                path: "/mock-executor/dry-run".to_string(),
                body: dry_run_body("success"),
            },
            &state,
        );

        assert_eq!(response.status, 200);
        assert_eq!(response.body["status"], "settled");
        assert_eq!(response.body["closure"]["kind"], "settlement");
        assert_eq!(
            fake.paths(),
            vec!["/spend/executor/validate", "/spend/executor/settle"]
        );
    }

    #[test]
    fn dry_run_pre_work_failure_validates_then_releases() {
        let fake = FakeHubu::start(vec![
            fake_response("/spend/executor/validate", spend_response("frozen")),
            fake_response("/spend/executor/release", spend_response("released")),
        ]);
        let state = ServerState::new(Config {
            bind_addr: "127.0.0.1:0".to_string(),
            hubu_base_url: fake.base_url.clone(),
            image_provider: mock_provider_config(std::env::temp_dir()),
        });

        let response = route(
            HttpRequest {
                method: "POST".to_string(),
                path: "/mock-executor/dry-run".to_string(),
                body: dry_run_body("pre_work_failure"),
            },
            &state,
        );

        assert_eq!(response.status, 200);
        assert_eq!(response.body["status"], "released");
        assert_eq!(response.body["closure"]["kind"], "release");
        assert_eq!(
            fake.paths(),
            vec!["/spend/executor/validate", "/spend/executor/release"]
        );
    }

    #[test]
    fn dry_run_rejects_missing_owner_anchor_before_hubu_call() {
        let state = ServerState::new(Config {
            bind_addr: "127.0.0.1:0".to_string(),
            hubu_base_url: "http://127.0.0.1:1".to_string(),
            image_provider: mock_provider_config(std::env::temp_dir()),
        });

        let response = route(
            HttpRequest {
                method: "POST".to_string(),
                path: "/mock-executor/dry-run".to_string(),
                body: json!({
                    "spend_auth_token_id": "token-1",
                    "amount_cents": 500,
                    "merchant": "gongbu.image",
                    "task_id": "hubu-logo-demo",
                })
                .to_string(),
            },
            &state,
        );

        assert_eq!(response.status, 400);
        assert!(response.body["error"]
            .as_str()
            .expect("error should be string")
            .contains("exactly one"));
    }

    #[test]
    fn server_responds_without_waiting_for_client_eof() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let addr = listener.local_addr().expect("server addr");
        let state = ServerState::new(Config {
            bind_addr: "127.0.0.1:0".to_string(),
            hubu_base_url: "http://127.0.0.1:8787".to_string(),
            image_provider: mock_provider_config(std::env::temp_dir()),
        });
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept request");
            handle_connection(stream, &state).expect("handle request");
        });

        let mut client = TcpStream::connect(addr).expect("connect client");
        client
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .expect("set read timeout");
        client
            .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: keep-alive\r\n\r\n")
            .expect("write request");

        let mut response = String::new();
        client
            .read_to_string(&mut response)
            .expect("read response without closing write side first");
        assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
        assert!(response.contains("\"status\":\"ok\""), "{response}");
        server.join().expect("server thread finished");
    }

    #[test]
    fn image_job_with_mock_provider_validates_generates_artifact_then_settles() {
        let output_dir = std::env::temp_dir().join(format!(
            "gongbu-mock-image-job-output-{}",
            std::process::id()
        ));
        let fake = FakeHubu::start(vec![
            fake_response("/spend/executor/validate", spend_response("frozen")),
            fake_response(
                "/spend/executor/settle",
                json!({
                    "settlement_id": "settlement-1",
                    "spend": spend_response("settled"),
                }),
            ),
        ]);
        let state = ServerState::new(Config {
            bind_addr: "127.0.0.1:0".to_string(),
            hubu_base_url: fake.base_url.clone(),
            image_provider: mock_provider_config(output_dir.clone()),
        });

        let response = route(
            HttpRequest {
                method: "POST".to_string(),
                path: "/image-jobs".to_string(),
                body: image_job_body("local-mock", "mock-image-v1"),
            },
            &state,
        );

        assert_eq!(response.status, 200, "{}", response.body);
        assert_eq!(response.body["provider"], "local-mock");
        assert_eq!(response.body["model"], "mock-image-v1");
        assert_eq!(
            fake.paths(),
            vec!["/spend/executor/validate", "/spend/executor/settle"]
        );
        let output_path = response.body["output_ref"]
            .as_str()
            .expect("output ref")
            .strip_prefix("file://")
            .expect("file ref");
        assert!(std::fs::read_to_string(output_path)
            .expect("mock artifact")
            .contains("Create a crisp logo for Project Hubu"));
        std::fs::remove_file(output_path).ok();
        std::fs::remove_dir(output_dir).ok();
    }

    #[test]
    fn image_job_with_gemini_provider_keeps_api_key_server_side_and_settles() {
        let provider = FakeProvider::start(json!({
            "candidates": [{
                "content": {
                    "parts": [{
                        "inlineData": {
                            "mimeType": "image/png",
                            "data": BASE64_STANDARD.encode(b"png-bytes"),
                        },
                    }],
                },
            }],
        }));
        let output_dir = std::env::temp_dir().join(format!(
            "gongbu-gemini-image-job-output-{}",
            std::process::id()
        ));
        let fake_hubu = FakeHubu::start(vec![
            fake_response("/spend/executor/validate", spend_response("frozen")),
            fake_response(
                "/spend/executor/settle",
                json!({
                    "settlement_id": "settlement-1",
                    "spend": spend_response("settled"),
                }),
            ),
        ]);
        let state = ServerState::new(Config {
            bind_addr: "127.0.0.1:0".to_string(),
            hubu_base_url: fake_hubu.base_url.clone(),
            image_provider: gemini_provider_config(provider.endpoint.clone(), output_dir.clone()),
        });

        let response = route(
            HttpRequest {
                method: "POST".to_string(),
                path: "/image-jobs".to_string(),
                body: image_job_body("google-gemini", "gemini-2.5-flash-image"),
            },
            &state,
        );

        assert_eq!(response.status, 200, "{}", response.body);
        assert_eq!(response.body["provider"], "google-gemini");
        assert_eq!(response.body["settlement"]["settlement_id"], "settlement-1");
        let provider_request = provider.request();
        let provider_request_lower = provider_request.to_ascii_lowercase();
        assert!(provider_request
            .starts_with("POST /v1beta/models/gemini-2.5-flash-image:generateContent HTTP/1.1"));
        assert!(provider_request_lower.contains("x-goog-api-key: server-side-secret"));
        assert!(provider_request.contains("Create a crisp logo for Project Hubu"));
        assert!(provider_request.contains("\"responseModalities\":[\"IMAGE\"]"));
        assert!(!response.body.to_string().contains("server-side-secret"));

        let output_path = response.body["output_ref"]
            .as_str()
            .expect("output ref")
            .strip_prefix("file://")
            .expect("file ref");
        assert_eq!(
            std::fs::read(output_path).expect("image artifact"),
            b"png-bytes"
        );
        assert_eq!(
            fake_hubu.paths(),
            vec!["/spend/executor/validate", "/spend/executor/settle"]
        );
        std::fs::remove_file(output_path).ok();
        std::fs::remove_dir(output_dir).ok();
    }

    #[test]
    fn image_job_releases_validated_hold_when_output_preflight_fails_before_provider_call() {
        let blocker = std::env::temp_dir().join(format!(
            "gongbu-image-output-blocker-{}",
            std::process::id()
        ));
        std::fs::write(&blocker, b"not a directory").expect("write blocker");
        let provider = FakeProvider::start(json!({ "unused": true }));
        let fake_hubu = FakeHubu::start(vec![
            fake_response("/spend/executor/validate", spend_response("frozen")),
            fake_response("/spend/executor/release", spend_response("released")),
        ]);
        let state = ServerState::new(Config {
            bind_addr: "127.0.0.1:0".to_string(),
            hubu_base_url: fake_hubu.base_url.clone(),
            image_provider: gemini_provider_config(provider.endpoint.clone(), blocker.clone()),
        });

        let response = route(
            HttpRequest {
                method: "POST".to_string(),
                path: "/image-jobs".to_string(),
                body: image_job_body("google-gemini", "gemini-2.5-flash-image"),
            },
            &state,
        );

        assert_eq!(response.status, 400);
        assert!(response.body["error"]
            .as_str()
            .expect("error")
            .contains("provider_artifact_write_failed"));
        assert_eq!(
            fake_hubu.paths(),
            vec!["/spend/executor/validate", "/spend/executor/release"]
        );
        assert!(provider.request_if_any().is_none());
        std::fs::remove_file(blocker).ok();
    }

    fn dry_run_body(outcome: &str) -> String {
        json!({
            "spend_auth_token_id": "token-1",
            "agent_id": "agt_example",
            "amount_cents": 500,
            "merchant": "gongbu.image",
            "task_id": "hubu-logo-demo",
            "outcome": outcome,
        })
        .to_string()
    }

    fn image_job_body(provider: &str, model: &str) -> String {
        json!({
            "spend_auth_token_id": "token-1",
            "agent_id": "agt_example",
            "amount_cents": 500,
            "merchant": "gongbu.image",
            "task_id": "hubu-logo-demo",
            "prompt": "Create a crisp logo for Project Hubu",
            "provider": provider,
            "model": model,
        })
        .to_string()
    }

    fn mock_provider_config(output_dir: std::path::PathBuf) -> ImageProviderConfig {
        ImageProviderConfig {
            provider: "local-mock".to_string(),
            model: "mock-image-v1".to_string(),
            merchant: "gongbu.image".to_string(),
            api_key: None,
            endpoint: None,
            price_cents: 500,
            timeout_ms: 30_000,
            max_retries: 0,
            http_json_fields: HttpJsonImageProviderFields::defaults(),
            output_dir,
            adapter_kind: ImageProviderAdapterKind::Mock,
        }
    }

    fn gemini_provider_config(
        endpoint: String,
        output_dir: std::path::PathBuf,
    ) -> ImageProviderConfig {
        ImageProviderConfig {
            provider: "google-gemini".to_string(),
            model: "gemini-2.5-flash-image".to_string(),
            merchant: "gongbu.image".to_string(),
            api_key: Some("server-side-secret".to_string()),
            endpoint: Some(endpoint),
            price_cents: 500,
            timeout_ms: 30_000,
            max_retries: 0,
            http_json_fields: HttpJsonImageProviderFields::defaults(),
            output_dir,
            adapter_kind: ImageProviderAdapterKind::GeminiGenerateContent,
        }
    }

    fn spend_response(status: &str) -> Value {
        serde_json::to_value(ExecutorSpendResponse {
            spend_auth_token_id: "token-1".to_string(),
            decision_id: "decision-1".to_string(),
            account_id: "acct_example".to_string(),
            agent_id: "agt_example".to_string(),
            amount_cents: 500,
            currency: "USD".to_string(),
            merchant: Some("gongbu.image".to_string()),
            task_id: Some("hubu-logo-demo".to_string()),
            expires_at: "2026-06-05T12:00:00Z".to_string(),
            budget_hold: BudgetHold {
                hold_id: "hold-1".to_string(),
                budget_id: "budget-1".to_string(),
                status: status.to_string(),
                amount_cents: 500,
                consumed_amount_cents: if status == "settled" { 500 } else { 0 },
                frozen_amount_cents: if status == "frozen" { 500 } else { 0 },
                remaining_amount_cents: if status == "released" { 500 } else { 0 },
            },
        })
        .expect("spend response should serialize")
    }

    fn fake_response(path: &str, body: Value) -> (&'static str, Value) {
        (Box::leak(path.to_string().into_boxed_str()), body)
    }

    struct FakeHubu {
        base_url: String,
        seen_paths: Arc<Mutex<Vec<String>>>,
    }

    impl FakeHubu {
        fn start(responses: Vec<(&'static str, Value)>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake Hubu");
            let addr = listener.local_addr().expect("fake Hubu addr");
            let seen_paths = Arc::new(Mutex::new(Vec::new()));
            let thread_paths = Arc::clone(&seen_paths);
            thread::spawn(move || {
                for (expected_path, body) in responses {
                    let (mut stream, _) = listener.accept().expect("accept fake Hubu request");
                    let mut raw = String::new();
                    stream
                        .read_to_string(&mut raw)
                        .expect("read fake Hubu request");
                    let request = parse_request(&raw).expect("parse fake Hubu request");
                    assert_eq!(request.method, "POST");
                    assert_eq!(request.path, expected_path);
                    thread_paths
                        .lock()
                        .expect("paths lock")
                        .push(request.path.clone());
                    write_response(&mut stream, &HttpResponse::ok(body))
                        .expect("write fake Hubu response");
                }
            });
            Self {
                base_url: format!("http://{addr}"),
                seen_paths,
            }
        }

        fn paths(&self) -> Vec<String> {
            self.seen_paths.lock().expect("paths lock").clone()
        }
    }

    struct FakeProvider {
        endpoint: String,
        seen_request: Arc<Mutex<Option<String>>>,
    }

    impl FakeProvider {
        fn start(body: Value) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake provider");
            let addr = listener.local_addr().expect("fake provider addr");
            let seen_request = Arc::new(Mutex::new(None));
            let thread_request = Arc::clone(&seen_request);
            thread::spawn(move || {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let request = read_content_length_http_request(&mut stream)
                    .expect("read fake provider request");
                *thread_request.lock().expect("request lock") = Some(request);
                write_response(&mut stream, &HttpResponse::ok(body))
                    .expect("write fake provider response");
            });
            Self {
                endpoint: format!(
                    "http://{addr}/v1beta/models/gemini-2.5-flash-image:generateContent"
                ),
                seen_request,
            }
        }

        fn request(&self) -> String {
            for _ in 0..100 {
                if let Some(request) = self.request_if_any() {
                    return request;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            panic!("fake provider did not receive a request");
        }

        fn request_if_any(&self) -> Option<String> {
            self.seen_request.lock().expect("request lock").clone()
        }
    }

    fn read_content_length_http_request(stream: &mut impl Read) -> std::io::Result<String> {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 512];
        loop {
            let read = stream.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..read]);
            if let Some(header_end) = find_header_end(&bytes) {
                let headers = String::from_utf8_lossy(&bytes[..header_end]).to_string();
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        if name.eq_ignore_ascii_case("content-length") {
                            value.trim().parse::<usize>().ok()
                        } else {
                            None
                        }
                    })
                    .unwrap_or(0);
                let body_start = header_end + 4;
                while bytes.len() < body_start + content_length {
                    let read = stream.read(&mut buffer)?;
                    if read == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&buffer[..read]);
                }
                break;
            }
        }
        Ok(String::from_utf8_lossy(&bytes).to_string())
    }

    fn find_header_end(bytes: &[u8]) -> Option<usize> {
        bytes.windows(4).position(|window| window == b"\r\n\r\n")
    }
}
