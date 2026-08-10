use gongbu_api::sandbox::{runtime, BoundaryMode, RunManifest, SandboxConfig};
use reqwest::header::AUTHORIZATION;
use serde_json::{json, Value};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::ExitCode,
    time::Duration,
};

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("gongbu sandbox failed: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut args = env::args().skip(1).collect::<Vec<_>>();
    let command = if args.first().is_some_and(|value| !value.starts_with('-')) {
        args.remove(0)
    } else {
        "start".into()
    };
    match command.as_str() {
        "start" => start(args).await,
        "submit" => submit(args).await,
        "status" => status(args).await,
        "artifacts" => artifacts(args).await,
        "inspect" => inspect(args),
        "help" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        _ => Err(format!("unknown command: {command}").into()),
    }
}

async fn start(args: Vec<String>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut args = Args::new(args);
    let config_path = args.required_path("--config")?;
    let preserve = args.optional_path("--preserve")?;
    let hubu_mode = args.optional("--hubu-mode")?;
    let provider_mode = args.optional("--provider-mode")?;
    let maximum_spend_minor = args.optional("--max-spend-minor")?;
    let live_spend_acknowledgement = args.optional("--live-spend-ack")?;
    args.finish()?;

    let mut config = SandboxConfig::load(config_path)?;
    config.apply_environment_overrides()?;
    if let Some(mode) = hubu_mode {
        config.hubu.mode = mode.parse::<BoundaryMode>()?;
    }
    if let Some(mode) = provider_mode {
        config.provider.mode = mode.parse::<BoundaryMode>()?;
    }
    if let Some(value) = maximum_spend_minor {
        config.provider.maximum_spend_minor = Some(
            value
                .parse()
                .map_err(|_| "--max-spend-minor must be an integer")?,
        );
    }
    if let Some(value) = live_spend_acknowledgement {
        config.provider.live_spend_acknowledgement = Some(value);
    }
    config.validate()?;
    runtime::serve(config, preserve).await
}

async fn submit(args: Vec<String>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut args = Args::new(args);
    let run_dir = args.required_path("--run-dir")?;
    let operation_key = args.required("--operation-key")?;
    let prompt = args.required("--prompt")?;
    args.finish()?;
    let context = OperatorContext::load(&run_dir)?;
    let target = &context.manifest.provider_target;
    let request = json!({
        "schema_version": 1,
        "operation_key": operation_key,
        "hubu_authorization_id": format!("sandbox-auth-{operation_key}"),
        "hubu_claim_id": null,
        "hubu_token_reference": "sandbox-hubu-authorization",
        "authorization": {
            "amount_minor": context.manifest.authorization_amount_minor,
            "currency": context.manifest.authorization_currency,
        },
        "input": {"prompt": prompt, "image_count": 1},
        "input_schema_version": 1,
        "workload_type": target.workload_type,
        "provider": target.provider,
        "adapter": target.adapter,
        "model": target.model,
    });
    let client = reqwest::Client::new();
    let response = send_json(
        client.post(format!("{}/v1/executions", context.manifest.gongbu_url)),
        &context.token,
        &request,
    )
    .await?;
    let execution_id = response
        .get("execution_id")
        .and_then(Value::as_str)
        .ok_or("execution response did not contain execution_id")?
        .to_owned();
    println!("execution_id: {execution_id}");
    println!(
        "temporal: {}/namespaces/{}/workflows/gongbu-execution-{}",
        context.manifest.temporal_ui_url, context.manifest.temporal_namespace, execution_id
    );
    let final_status = poll_execution(&client, &context, &execution_id).await?;
    println!("{}", serde_json::to_string_pretty(&final_status)?);
    Ok(())
}

async fn status(args: Vec<String>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (context, execution_id) = execution_args(args)?;
    let response = get_json(&context, &format!("/v1/executions/{execution_id}")).await?;
    println!("{}", serde_json::to_string_pretty(&response)?);
    Ok(())
}

async fn artifacts(args: Vec<String>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut args = Args::new(args);
    let run_dir = args.required_path("--run-dir")?;
    let execution_id = args.required("--execution-id")?;
    let download_dir = args.optional_path("--download-dir")?;
    args.finish()?;
    let context = OperatorContext::load(run_dir)?;
    let response = get_json(
        &context,
        &format!("/v1/executions/{execution_id}/artifacts"),
    )
    .await?;
    println!("{}", serde_json::to_string_pretty(&response)?);
    if let Some(download_dir) = download_dir {
        fs::create_dir_all(&download_dir)?;
        for artifact in response
            .get("artifacts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let id = artifact
                .get("artifact_id")
                .and_then(Value::as_str)
                .ok_or("artifact has no artifact_id")?;
            let media_type = artifact
                .get("media_type")
                .and_then(Value::as_str)
                .unwrap_or("application/octet-stream");
            let extension = if media_type == "image/png" {
                "png"
            } else {
                "jpg"
            };
            let response = authorized(
                reqwest::Client::new()
                    .get(format!("{}/v1/artifacts/{id}", context.manifest.gongbu_url)),
                &context.token,
            )
            .send()
            .await?
            .error_for_status()?;
            let path = download_dir.join(format!("{id}.{extension}"));
            fs::write(&path, response.bytes().await?)?;
            println!("downloaded {}", path.display());
        }
    }
    Ok(())
}

fn inspect(args: Vec<String>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut args = Args::new(args);
    let run_dir = args.required_path("--run-dir")?;
    args.finish()?;
    println!(
        "{}",
        fs::read_to_string(run_dir.join("mock-side-effects.json"))?
    );
    Ok(())
}

async fn poll_execution(
    client: &reqwest::Client,
    context: &OperatorContext,
    execution_id: &str,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    for _ in 0..120 {
        let response = send(authorized(
            client.get(format!(
                "{}/v1/executions/{execution_id}",
                context.manifest.gongbu_url
            )),
            &context.token,
        ))
        .await?;
        let status = response.get("status").and_then(Value::as_str).unwrap_or("");
        if matches!(
            status,
            "succeeded" | "released" | "failed" | "reconciliation_required"
        ) {
            return Ok(response);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Err("execution did not reach a terminal state within 30 seconds".into())
}

async fn get_json(
    context: &OperatorContext,
    path: &str,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    send(authorized(
        reqwest::Client::new().get(format!("{}{}", context.manifest.gongbu_url, path)),
        &context.token,
    ))
    .await
}

async fn send_json(
    request: reqwest::RequestBuilder,
    token: &str,
    body: &Value,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    send(authorized(request, token).json(body)).await
}

async fn send(
    request: reqwest::RequestBuilder,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    let response = request.send().await?;
    let status = response.status();
    let bytes = response.bytes().await?;
    if !status.is_success() {
        return Err(format!(
            "Gongbu returned {status}: {}",
            String::from_utf8_lossy(&bytes)
        )
        .into());
    }
    Ok(serde_json::from_slice(&bytes)?)
}

fn authorized(request: reqwest::RequestBuilder, token: &str) -> reqwest::RequestBuilder {
    request.header(AUTHORIZATION, format!("Bearer {token}"))
}

fn execution_args(
    args: Vec<String>,
) -> Result<(OperatorContext, String), Box<dyn std::error::Error + Send + Sync>> {
    let mut args = Args::new(args);
    let run_dir = args.required_path("--run-dir")?;
    let execution_id = args.required("--execution-id")?;
    args.finish()?;
    Ok((OperatorContext::load(run_dir)?, execution_id))
}

struct OperatorContext {
    manifest: RunManifest,
    token: String,
}

impl OperatorContext {
    fn load(run_dir: impl AsRef<Path>) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let run_dir = run_dir.as_ref();
        Ok(Self {
            manifest: serde_json::from_slice(&fs::read(run_dir.join("run-manifest.json"))?)?,
            token: fs::read_to_string(run_dir.join("operator-token"))?
                .trim()
                .to_owned(),
        })
    }
}

struct Args {
    values: Vec<String>,
}

impl Args {
    fn new(values: Vec<String>) -> Self {
        Self { values }
    }
    fn required(&mut self, name: &str) -> Result<String, String> {
        self.optional(name)?
            .ok_or_else(|| format!("{name} is required"))
    }
    fn required_path(&mut self, name: &str) -> Result<PathBuf, String> {
        self.required(name).map(PathBuf::from)
    }
    fn optional_path(&mut self, name: &str) -> Result<Option<PathBuf>, String> {
        self.optional(name).map(|value| value.map(PathBuf::from))
    }
    fn optional(&mut self, name: &str) -> Result<Option<String>, String> {
        if let Some(index) = self.values.iter().position(|value| value == name) {
            if index + 1 >= self.values.len() {
                return Err(format!("{name} requires a value"));
            }
            self.values.remove(index);
            Ok(Some(self.values.remove(index)))
        } else {
            Ok(None)
        }
    }
    fn finish(self) -> Result<(), String> {
        if self.values.is_empty() {
            Ok(())
        } else {
            Err(format!("unknown arguments: {}", self.values.join(" ")))
        }
    }
}

fn print_help() {
    println!(
        "gongbu-sandbox commands:\n\
         \n  start --config PROFILE [--hubu-mode mock|real] [--provider-mode mock|real] [--preserve DIR]\
         \n  submit --run-dir DIR --operation-key KEY --prompt TEXT\
         \n  status --run-dir DIR --execution-id ID\
         \n  artifacts --run-dir DIR --execution-id ID [--download-dir DIR]\
         \n  inspect --run-dir DIR"
    );
}
