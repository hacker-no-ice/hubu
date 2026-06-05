use std::{env, time::Duration};

use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde::Deserialize;

const DEFAULT_GCP_METADATA_TOKEN_URL: &str =
    "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token";
const DEFAULT_GCP_SECRET_MANAGER_BASE_URL: &str = "https://secretmanager.googleapis.com";

#[derive(Debug, Clone)]
pub struct SecretProviderConfig {
    kind: SecretProviderKind,
}

#[derive(Debug, Clone)]
enum SecretProviderKind {
    None,
    EnvDev,
    GcpSecretManager(GcpSecretManagerConfig),
}

#[derive(Debug, Clone)]
struct GcpSecretManagerConfig {
    secret_name: String,
    metadata_token_url: String,
    secret_manager_base_url: String,
}

impl SecretProviderConfig {
    pub fn from_env() -> Result<Self> {
        let kind = env::var("GONGBU_SECRET_PROVIDER").unwrap_or_else(|_| "none".to_string());
        let kind = match kind.as_str() {
            "none" => SecretProviderKind::None,
            "env-dev" => SecretProviderKind::EnvDev,
            "gcp-secret-manager" => {
                let secret_name = env::var("GONGBU_IMAGE_PROVIDER_API_KEY_SECRET")
                    .context("GONGBU_IMAGE_PROVIDER_API_KEY_SECRET is required")?;
                if secret_name.trim().is_empty() {
                    return Err(anyhow!(
                        "GONGBU_IMAGE_PROVIDER_API_KEY_SECRET must not be empty"
                    ));
                }
                SecretProviderKind::GcpSecretManager(GcpSecretManagerConfig {
                    secret_name,
                    metadata_token_url: env::var("GONGBU_GCP_METADATA_TOKEN_URL")
                        .unwrap_or_else(|_| DEFAULT_GCP_METADATA_TOKEN_URL.to_string()),
                    secret_manager_base_url: env::var("GONGBU_GCP_SECRET_MANAGER_BASE_URL")
                        .unwrap_or_else(|_| DEFAULT_GCP_SECRET_MANAGER_BASE_URL.to_string()),
                })
            }
            other => {
                return Err(anyhow!(
                    "GONGBU_SECRET_PROVIDER must be one of none, env-dev, gcp-secret-manager; got {other}"
                ))
            }
        };
        Ok(Self { kind })
    }

    pub fn load_image_provider_api_key(&self) -> Result<Option<String>> {
        match &self.kind {
            SecretProviderKind::None => Ok(None),
            SecretProviderKind::EnvDev => env::var("GONGBU_IMAGE_PROVIDER_API_KEY")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .map(Some)
                .ok_or_else(|| {
                    anyhow!(
                        "GONGBU_IMAGE_PROVIDER_API_KEY is required when GONGBU_SECRET_PROVIDER=env-dev"
                    )
                }),
            SecretProviderKind::GcpSecretManager(config) => {
                let token = fetch_gcp_access_token(&config.metadata_token_url)?;
                let secret = fetch_gcp_secret_payload(
                    &config.secret_manager_base_url,
                    &config.secret_name,
                    &token,
                )?;
                Ok(Some(secret))
            }
        }
    }
}

#[derive(Debug, Deserialize)]
struct GcpMetadataTokenResponse {
    access_token: String,
}

#[derive(Debug, Deserialize)]
struct GcpSecretAccessResponse {
    payload: GcpSecretPayload,
}

#[derive(Debug, Deserialize)]
struct GcpSecretPayload {
    data: String,
}

fn fetch_gcp_access_token(metadata_token_url: &str) -> Result<String> {
    let response: GcpMetadataTokenResponse = ureq::get(metadata_token_url)
        .set("Metadata-Flavor", "Google")
        .timeout(Duration::from_secs(5))
        .call()
        .map_err(|error| anyhow!("fetch GCP metadata access token: {error}"))?
        .into_json()
        .context("parse GCP metadata access token response")?;
    if response.access_token.trim().is_empty() {
        return Err(anyhow!("GCP metadata access token response was empty"));
    }
    Ok(response.access_token)
}

fn fetch_gcp_secret_payload(
    secret_manager_base_url: &str,
    secret_name: &str,
    access_token: &str,
) -> Result<String> {
    let url = format!(
        "{}/v1/{}:access",
        secret_manager_base_url.trim_end_matches('/'),
        secret_name.trim_start_matches('/')
    );
    let response: GcpSecretAccessResponse = ureq::get(&url)
        .set("Authorization", &format!("Bearer {access_token}"))
        .timeout(Duration::from_secs(10))
        .call()
        .map_err(|error| anyhow!("access configured image provider secret: {error}"))?
        .into_json()
        .context("parse Secret Manager access response")?;
    let decoded = BASE64_STANDARD
        .decode(response.payload.data)
        .context("decode Secret Manager payload data")?;
    let secret = String::from_utf8(decoded).context("decode secret payload as UTF-8")?;
    let secret = secret.trim().to_string();
    if secret.is_empty() {
        return Err(anyhow!("configured image provider secret is empty"));
    }
    Ok(secret)
}

#[cfg(test)]
mod tests {
    use std::{
        io::Read,
        net::TcpListener,
        sync::{Arc, Mutex},
        thread,
    };

    use serde_json::json;

    use super::*;
    use crate::simple_http::{parse_request, write_response, HttpResponse};

    #[test]
    fn gcp_secret_manager_fetches_token_then_secret_payload() {
        let metadata = FakeJsonServer::start(json!({
            "access_token": "metadata-token",
        }));
        let secret = FakeJsonServer::start(json!({
            "payload": {
                "data": BASE64_STANDARD.encode("server-side-secret"),
            },
        }));

        let token = fetch_gcp_access_token(&metadata.url).expect("metadata token");
        let payload = fetch_gcp_secret_payload(
            &secret.url,
            "projects/demo/secrets/gemini-api-key/versions/latest",
            &token,
        )
        .expect("secret payload");

        assert_eq!(payload, "server-side-secret");
        assert!(metadata.request().contains("Metadata-Flavor: Google"));
        assert!(secret.request().contains(
            "GET /v1/projects/demo/secrets/gemini-api-key/versions/latest:access HTTP/1.1"
        ));
        assert!(secret
            .request()
            .contains("Authorization: Bearer metadata-token"));
    }

    struct FakeJsonServer {
        url: String,
        request: Arc<Mutex<Option<String>>>,
    }

    impl FakeJsonServer {
        fn start(body: serde_json::Value) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake server");
            let addr = listener.local_addr().expect("fake server addr");
            let request = Arc::new(Mutex::new(None));
            let thread_request = Arc::clone(&request);
            thread::spawn(move || {
                let (mut stream, _) = listener.accept().expect("accept request");
                let raw = read_http_headers(&mut stream).expect("read request");
                let parsed = parse_request(&raw).expect("parse request");
                *thread_request.lock().expect("request lock") = Some(format!(
                    "{} {} HTTP/1.1\n{}",
                    parsed.method, parsed.path, raw
                ));
                write_response(&mut stream, &HttpResponse::ok(body)).expect("write response");
            });
            Self {
                url: format!("http://{addr}"),
                request,
            }
        }

        fn request(&self) -> String {
            for _ in 0..100 {
                if let Some(request) = self.request.lock().expect("request lock").clone() {
                    return request;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            panic!("fake server did not receive request");
        }
    }

    fn read_http_headers(stream: &mut impl Read) -> std::io::Result<String> {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 256];
        loop {
            let read = stream.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..read]);
            if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        Ok(String::from_utf8_lossy(&bytes).to_string())
    }
}
