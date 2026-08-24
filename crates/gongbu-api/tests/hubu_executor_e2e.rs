use gongbu_api::{
    application::ArtifactServiceActivities,
    artifact::{ArtifactLimits, ArtifactService, LocalFsStorage},
    execution::{Execution, Repository},
    http::{Api, AuthenticatedAccount, ExecutionResponse},
    hubu::{HubuClient, ProductionHubuActivities},
    provider::{
        contract::{
            AdapterCapabilities, AdapterOutcome, NormalizedRequest, NormalizedUsage,
            PricingCatalog, ProviderAdapter, ProviderFailure,
        },
        registry::{ProviderRegistry, ValidatedProviderCatalog},
        targets::ProviderTargetConfig,
    },
    redaction::Redactor,
    secrets::ProviderSecret,
    temporal::ExecutionScheduler,
    workflow::{
        ActivityError, ExecutionWorkflow, HubuActivities, OperatorReconciliationRequest,
        ProviderActivities, ProviderArtifact, ProviderSuccess,
    },
};
use reqwest::blocking::Client;
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    env,
    fs::{self, File},
    io::Cursor,
    net::{SocketAddr, TcpListener},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Arc,
    thread,
    time::Duration,
};
use tempfile::{tempdir, TempDir};

const AUTH_TOKEN: &str = "hubu_e2e_local_auth";
const RECONCILIATION_TOKEN: &str = "hubu_reconcile_e2e_local_auth";
const AUTHORIZED_MINOR: i64 = 40;
const ACTUAL_MINOR: i64 = 40;
const NOW: &str = "2026-08-17T00:00:00Z";
const POLICY: &str = r#"id: hubu_gongbu_e2e
version: v1
default_effect: deny
rules:
  - id: allow_gongbu_executor
    effect: allow
    reason: deterministic local Gongbu executor test
    when:
      op: eq
      field: provider
      value:
        string: provider:local:fixture
"#;

#[test]
#[ignore = "run through scripts/integration-hubu-gongbu-executor.sh"]
fn deterministic_hubu_to_gongbu_executor_contract() {
    let mut workspace = TestWorkspace::start();
    let provisioned = workspace.admin.provision();
    let repository = Repository::open(workspace.root().join("gongbu.sqlite3"), Redactor::default())
        .expect("open isolated Gongbu state");
    let artifact_service = ArtifactService::new(
        repository.clone(),
        LocalFsStorage::new(workspace.root().join("artifacts")),
        ArtifactLimits::default(),
    );
    let artifact_activities =
        ArtifactServiceActivities::new(artifact_service.clone(), || NOW.to_string());
    let admission_api = Api::new_with_authorization_resolver(
        repository.clone(),
        artifact_service,
        admission_catalog(),
        Arc::new(AdmissionScheduler),
        i64::MAX,
        Arc::new(
            ProductionHubuActivities::new(workspace.hubu_client(), provisioned.agent_id.clone())
                .unwrap(),
        ),
        || NOW.to_string(),
    );
    let owner = AuthenticatedAccount::from_verified_claim(provisioned.account_id.clone()).unwrap();
    let admission = AdmissionContext {
        api: &admission_api,
        owner: &owner,
        repository: &repository,
    };

    let success = run_success(
        &workspace,
        &provisioned,
        &artifact_activities,
        &admission,
        "hub-83:success-replay",
        Fault::None,
    );
    assert_eq!(success.provider_calls, 1);
    assert_eq!(success.hubu_settle_calls, 1);
    assert_eq!(success.hubu_release_calls, 0);
    assert_terminal_claim(&workspace, &success.claim_id, "settled", ACTUAL_MINOR);

    let failed = run_provider_failure(
        &workspace,
        &provisioned,
        &artifact_activities,
        &admission,
        "hub-83:proven-provider-failure",
    );
    assert_eq!(failed.provider_calls, 1);
    assert_eq!(failed.hubu_settle_calls, 0);
    assert_eq!(failed.hubu_release_calls, 1);
    assert_terminal_claim(&workspace, &failed.claim_id, "released", ACTUAL_MINOR);

    let ambiguous_claim = run_ambiguous_claim_recovery(
        &workspace,
        &provisioned,
        &artifact_activities,
        &admission,
        "hub-83:ambiguous-claim",
    );
    assert_eq!(ambiguous_claim.provider_calls, 0);
    assert_eq!(ambiguous_claim.hubu_claim_calls, 2);
    assert_eq!(ambiguous_claim.hubu_release_calls, 1);
    assert_terminal_claim(
        &workspace,
        &ambiguous_claim.claim_id,
        "released",
        ACTUAL_MINOR,
    );

    let ambiguous_settlement = run_success(
        &workspace,
        &provisioned,
        &artifact_activities,
        &admission,
        "hub-83:ambiguous-settlement",
        Fault::DropSettlementResponse,
    );
    assert_eq!(ambiguous_settlement.provider_calls, 1);
    assert_eq!(ambiguous_settlement.hubu_settle_calls, 2);
    assert_eq!(ambiguous_settlement.hubu_release_calls, 0);
    assert_terminal_claim(
        &workspace,
        &ambiguous_settlement.claim_id,
        "settled",
        ACTUAL_MINOR * 2,
    );

    let budget = workspace.admin.get("/budgets")["budgets"][0].clone();
    assert_eq!(budget["consumed_amount_cents"], ACTUAL_MINOR * 2);
    assert_eq!(budget["frozen_amount_cents"], 0);
    assert_eq!(budget["remaining_amount_cents"], 1_000 - ACTUAL_MINOR * 2);

    let concurrent_a = workspace
        .admin
        .authorize(&provisioned, "hub-122:concurrent-a");
    let concurrent_b = workspace
        .admin
        .authorize(&provisioned, "hub-122:concurrent-b");
    let admitted_a = admit_execution(
        admission.api,
        admission.owner,
        admission.repository,
        "hub-122:concurrent-a",
        &concurrent_a,
    );
    let admitted_b = admit_execution(
        admission.api,
        admission.owner,
        admission.repository,
        "hub-122:concurrent-b",
        &concurrent_b,
    );
    assert_ne!(admitted_a.execution_id, admitted_b.execution_id);
    let budget = workspace.admin.get("/budgets")["budgets"][0].clone();
    assert_eq!(budget["consumed_amount_cents"], ACTUAL_MINOR * 2);
    assert_eq!(budget["frozen_amount_cents"], AUTHORIZED_MINOR * 2);
    assert_eq!(
        budget["remaining_amount_cents"],
        1_000 - ACTUAL_MINOR * 2 - AUTHORIZED_MINOR * 2
    );

    let log = fs::read_to_string(&workspace.log_path).expect("read Hubu server log");
    for route in [
        "/spend/authorize",
        "/spend/executor/resolve",
        "/spend/executor/validate",
        "/spend/executor/claim",
        "/spend/executor/settle",
        "/spend/executor/release",
    ] {
        assert!(
            log.contains(&format!(r#""path":"{route}""#)),
            "Hubu audit log omitted {route}; state is preserved at {}",
            workspace.root().display()
        );
    }

    workspace.mark_success();
}

#[derive(Clone, Copy)]
enum Fault {
    None,
    DropSettlementResponse,
}

struct ScenarioResult {
    claim_id: String,
    provider_calls: usize,
    hubu_claim_calls: usize,
    hubu_settle_calls: usize,
    hubu_release_calls: usize,
}

struct AdmissionContext<'a> {
    api: &'a Api,
    owner: &'a AuthenticatedAccount,
    repository: &'a Repository,
}

fn run_success(
    workspace: &TestWorkspace,
    provisioned: &Provisioned,
    artifacts: &ArtifactServiceActivities,
    admission: &AdmissionContext<'_>,
    operation_key: &str,
    fault: Fault,
) -> ScenarioResult {
    let authorization = workspace.admin.authorize(provisioned, operation_key);
    let execution = admit_execution(
        admission.api,
        admission.owner,
        admission.repository,
        operation_key,
        &authorization,
    );
    let replay = admit_execution(
        admission.api,
        admission.owner,
        admission.repository,
        operation_key,
        &authorization,
    );
    assert_eq!(execution.execution_id, replay.execution_id);

    let provider = DeterministicProvider::success();
    let hubu = FaultingProductionHubu::new(
        workspace.base_url(),
        &provisioned.agent_id,
        matches!(fault, Fault::DropSettlementResponse),
        false,
    );
    let workflow = ExecutionWorkflow {
        repository: admission.repository,
        hubu: &hubu,
        provider: &provider,
        artifacts,
    };
    let first = workflow
        .run(&execution.execution_id, NOW)
        .expect("run execution");
    let completed = if matches!(fault, Fault::DropSettlementResponse) {
        assert_eq!(first.status, "reconciliation_required");
        assert_eq!(provider.calls.get(), 1);
        let claim_id = first.hubu_claim_id.as_deref().expect("persisted claim");
        let already_settled = workspace.hubu_client().inspect_claim(claim_id).unwrap();
        assert_eq!(already_settled.status, "settled");
        workflow
            .recover(&execution.execution_id, NOW, None)
            .expect("recover identical settlement")
    } else {
        first
    };
    assert_eq!(completed.status, "succeeded");

    let terminal_replay = workflow
        .run(&execution.execution_id, NOW)
        .expect("terminal replay");
    assert_eq!(terminal_replay.execution_id, execution.execution_id);
    assert_eq!(terminal_replay.status, "succeeded");
    assert_eq!(
        provider.calls.get(),
        1,
        "provider must execute exactly once"
    );
    assert_eq!(
        admission
            .repository
            .count_artifacts_for_execution(&execution.execution_id)
            .unwrap(),
        1
    );
    assert_eq!(
        admission
            .repository
            .get_provider_attempt_for_execution(&execution.execution_id)
            .unwrap()
            .outcome,
        "succeeded"
    );
    let receipt = admission
        .repository
        .get_receipt_for_execution(&execution.execution_id)
        .expect("durable Gongbu receipt");
    assert_eq!(receipt.settlement_minor, ACTUAL_MINOR);
    assert!(receipt.settled_at.is_some());
    let claim_id = completed.hubu_claim_id.expect("persisted Hubu claim");
    assert_eq!(
        receipt.hubu_settlement_id.as_deref(),
        workspace
            .hubu_client()
            .inspect_claim(&claim_id)
            .unwrap()
            .settlement_id
            .as_deref()
    );

    ScenarioResult {
        claim_id,
        provider_calls: provider.calls.get(),
        hubu_claim_calls: hubu.claim_calls.get(),
        hubu_settle_calls: hubu.settle_calls.get(),
        hubu_release_calls: hubu.release_calls.get(),
    }
}

fn run_provider_failure(
    workspace: &TestWorkspace,
    provisioned: &Provisioned,
    artifacts: &ArtifactServiceActivities,
    admission: &AdmissionContext<'_>,
    operation_key: &str,
) -> ScenarioResult {
    let authorization = workspace.admin.authorize(provisioned, operation_key);
    let execution = admit_execution(
        admission.api,
        admission.owner,
        admission.repository,
        operation_key,
        &authorization,
    );
    let provider = DeterministicProvider::proven_failure();
    let hubu =
        FaultingProductionHubu::new(workspace.base_url(), &provisioned.agent_id, false, false);
    let workflow = ExecutionWorkflow {
        repository: admission.repository,
        hubu: &hubu,
        provider: &provider,
        artifacts,
    };
    let done = workflow.run(&execution.execution_id, NOW).unwrap();
    assert_eq!(done.status, "released");
    workflow.run(&execution.execution_id, NOW).unwrap();
    assert_eq!(provider.calls.get(), 1);
    assert_eq!(
        admission
            .repository
            .get_provider_attempt_for_execution(&execution.execution_id)
            .unwrap()
            .outcome,
        "failed"
    );
    assert_eq!(
        admission
            .repository
            .count_artifacts_for_execution(&execution.execution_id)
            .unwrap(),
        0
    );
    assert!(admission
        .repository
        .get_receipt_for_execution(&execution.execution_id)
        .is_err());

    ScenarioResult {
        claim_id: done.hubu_claim_id.expect("persisted Hubu claim"),
        provider_calls: provider.calls.get(),
        hubu_claim_calls: hubu.claim_calls.get(),
        hubu_settle_calls: hubu.settle_calls.get(),
        hubu_release_calls: hubu.release_calls.get(),
    }
}

fn run_ambiguous_claim_recovery(
    workspace: &TestWorkspace,
    provisioned: &Provisioned,
    artifacts: &ArtifactServiceActivities,
    admission: &AdmissionContext<'_>,
    operation_key: &str,
) -> ScenarioResult {
    let authorization = workspace.admin.authorize(provisioned, operation_key);
    let execution = admit_execution(
        admission.api,
        admission.owner,
        admission.repository,
        operation_key,
        &authorization,
    );
    let provider = DeterministicProvider::success();
    let hubu =
        FaultingProductionHubu::new(workspace.base_url(), &provisioned.agent_id, false, true);
    let workflow = ExecutionWorkflow {
        repository: admission.repository,
        hubu: &hubu,
        provider: &provider,
        artifacts,
    };
    let held = workflow.run(&execution.execution_id, NOW).unwrap();
    assert_eq!(held.status, "reconciliation_required");
    assert!(held.hubu_claim_id.is_none());
    assert_eq!(provider.calls.get(), 0);
    let first_claim_id = hubu
        .first_claim_id
        .borrow()
        .clone()
        .expect("server accepted the first claim");

    let recovered = workflow
        .recover(&execution.execution_id, NOW, None)
        .expect("recover claim by immutable operation identity");
    assert_eq!(recovered.status, "released");
    assert_eq!(
        recovered.hubu_claim_id.as_deref(),
        Some(first_claim_id.as_str())
    );
    workflow.run(&execution.execution_id, NOW).unwrap();
    assert_eq!(
        provider.calls.get(),
        0,
        "claim recovery must not invoke provider"
    );

    ScenarioResult {
        claim_id: first_claim_id,
        provider_calls: provider.calls.get(),
        hubu_claim_calls: hubu.claim_calls.get(),
        hubu_settle_calls: hubu.settle_calls.get(),
        hubu_release_calls: hubu.release_calls.get(),
    }
}

fn admit_execution(
    api: &Api,
    owner: &AuthenticatedAccount,
    repository: &Repository,
    operation_key: &str,
    authorization: &Value,
) -> Execution {
    let body = serde_json::to_vec(&json!({
        "schema_version": 2,
        "spend_auth_token_id": string_at(authorization, "auth_token_id"),
        "input": {"prompt":"deterministic blue pixel","image_count":1},
        "input_schema_version": 1,
        "workload_type": "image_generation",
        "provider": "mock",
        "adapter": "deterministic",
        "model": "pixel-v1"
    }))
    .unwrap();
    let response = api.handle("POST", "/v2/executions", Some(owner), &body);
    assert_eq!(
        response.status,
        200,
        "Gongbu rejected {operation_key}: {}",
        String::from_utf8_lossy(&response.body)
    );
    let admitted: ExecutionResponse = serde_json::from_slice(&response.body).unwrap();
    repository
        .get_execution(&admitted.execution_id)
        .expect("load admitted Gongbu execution")
}

fn admission_catalog() -> ValidatedProviderCatalog {
    let targets: ProviderTargetConfig = serde_json::from_value(json!({
        "provider_configs": [{
            "provider_config_version": "mock-pcv-1",
            "workload_type": "image_generation",
            "provider": "mock",
            "adapter": "deterministic",
            "model": "pixel-v1",
            "secret_service": "gongbu.mock",
            "secret_account": "local"
        }]
    }))
    .unwrap();
    let pricing = PricingCatalog::from_json(
        br#"{"schema_version":2,"catalog_version":"hub-83-e2e-v2","rules":[{"rule_id":"one-image","provider":"mock","model":"pixel-v1","currency":"USD","components":[{"unit":"image","rate_numerator_minor":40,"rate_denominator":1}]}]}"#,
    )
    .unwrap();
    let mut registry = ProviderRegistry::new();
    registry.register("mock", "deterministic", |_| Ok(Arc::new(AdmissionAdapter)));
    ValidatedProviderCatalog::bind(targets, pricing, &registry).unwrap()
}

struct AdmissionAdapter;

impl ProviderAdapter for AdmissionAdapter {
    fn adapter_id(&self) -> &str {
        "deterministic"
    }

    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            vendor_enforced_idempotency: false,
        }
    }

    fn invoke(
        &self,
        _: &NormalizedRequest,
        _: &Value,
        _: &ProviderSecret,
        _: Option<&str>,
    ) -> Result<AdapterOutcome, ProviderFailure> {
        unreachable!("HTTP admission does not invoke providers")
    }
}

struct AdmissionScheduler;

impl ExecutionScheduler for AdmissionScheduler {
    fn schedule(&self, _: &str) -> Result<(), String> {
        Ok(())
    }

    fn reconcile(&self, _: &str, _: OperatorReconciliationRequest) -> Result<(), String> {
        Ok(())
    }
}

struct DeterministicProvider {
    calls: Cell<usize>,
    failure: bool,
}

impl DeterministicProvider {
    fn success() -> Self {
        Self {
            calls: Cell::new(0),
            failure: false,
        }
    }

    fn proven_failure() -> Self {
        Self {
            calls: Cell::new(0),
            failure: true,
        }
    }
}

impl ProviderActivities for DeterministicProvider {
    fn preflight(&self, _: &Execution) -> Result<(), ActivityError> {
        Ok(())
    }

    fn invoke(&self, execution: &Execution, _: &str) -> Result<ProviderSuccess, ActivityError> {
        assert!(
            execution.hubu_claim_id.is_some(),
            "Hubu claim must be durable before provider execution"
        );
        self.calls.set(self.calls.get() + 1);
        if self.failure {
            return Err(ActivityError::ProvenWithEvidence {
                code: "mock_provider_rejected".into(),
                request_id: Some(format!("mock-rejected:{}", execution.operation_key)),
                operation_id: None,
            });
        }
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            1,
            1,
            image::Rgba([0, 96, 255, 255]),
        ))
        .write_to(&mut Cursor::new(&mut png), image::ImageOutputFormat::Png)
        .expect("encode deterministic PNG fixture");
        Ok(ProviderSuccess {
            request_id: Some(format!("mock-request:{}", execution.operation_key)),
            operation_id: Some(format!("mock-operation:{}", execution.operation_key)),
            usage: NormalizedUsage {
                images: Some(1),
                ..Default::default()
            },
            provider_amount_minor: Some(ACTUAL_MINOR),
            provider_currency: Some("USD".into()),
            artifacts: vec![ProviderArtifact {
                media_type: "image/png".into(),
                bytes: png,
            }],
        })
    }
}

struct FaultingProductionHubu {
    inner: ProductionHubuActivities,
    drop_settlement_response: Cell<bool>,
    drop_claim_response: Cell<bool>,
    first_claim_id: RefCell<Option<String>>,
    claim_calls: Cell<usize>,
    settle_calls: Cell<usize>,
    release_calls: Cell<usize>,
}

impl FaultingProductionHubu {
    fn new(base_url: &str, agent_id: &str, drop_settlement: bool, drop_claim: bool) -> Self {
        Self {
            inner: ProductionHubuActivities::new(
                HubuClient::new(base_url).with_bearer_token(AUTH_TOKEN.as_bytes()),
                agent_id,
            )
            .unwrap(),
            drop_settlement_response: Cell::new(drop_settlement),
            drop_claim_response: Cell::new(drop_claim),
            first_claim_id: RefCell::new(None),
            claim_calls: Cell::new(0),
            settle_calls: Cell::new(0),
            release_calls: Cell::new(0),
        }
    }
}

impl HubuActivities for FaultingProductionHubu {
    fn preflight(&self, execution: &Execution) -> Result<(), ActivityError> {
        self.inner.preflight(execution)
    }

    fn claim(&self, execution: &Execution) -> Result<String, ActivityError> {
        self.claim_calls.set(self.claim_calls.get() + 1);
        let claim_id = self.inner.claim(execution)?;
        self.first_claim_id
            .borrow_mut()
            .get_or_insert_with(|| claim_id.clone());
        if self.drop_claim_response.replace(false) {
            Err(ActivityError::Ambiguous(
                "simulated_lost_claim_response".into(),
            ))
        } else {
            Ok(claim_id)
        }
    }

    fn validate_claim(&self, execution: &Execution) -> Result<(), ActivityError> {
        self.inner.validate_claim(execution)
    }

    fn settle(
        &self,
        execution: &Execution,
        receipt_id: &str,
        amount_minor: i64,
    ) -> Result<String, ActivityError> {
        self.settle_calls.set(self.settle_calls.get() + 1);
        let settlement_id = self.inner.settle(execution, receipt_id, amount_minor)?;
        if self.drop_settlement_response.replace(false) {
            Err(ActivityError::Ambiguous(
                "simulated_lost_settlement_response".into(),
            ))
        } else {
            Ok(settlement_id)
        }
    }

    fn release(&self, execution: &Execution) -> Result<(), ActivityError> {
        self.release_calls.set(self.release_calls.get() + 1);
        self.inner.release(execution)
    }
}

struct Provisioned {
    agent_id: String,
    account_id: String,
}

struct HubuAdmin {
    base_url: String,
    client: Client,
}

impl HubuAdmin {
    fn provision(&self) -> Provisioned {
        let user = self.post(
            "/init",
            json!({
                "username":"hub-83-e2e",
                "display_name":"HUB-83 E2E",
                "email":"hub-83@example.invalid"
            }),
        );
        assert!(string_at(&user, "user_id").starts_with("usr_"));
        let registration = self.post(
            "/agents/register",
            json!({"name":"hub-83-e2e-agent","version":"HUB-83"}),
        );
        let provisioned = Provisioned {
            agent_id: string_at(&registration, "agent_id"),
            account_id: string_at(&registration, "account_id"),
        };
        self.post("/policies", json!({"policy_yaml":POLICY}));
        self.post(
            "/budgets",
            json!({
                "agent_id":provisioned.agent_id,
                "amount_cents":1_000,
                "ending_before":"2999-01-01T00:00:00Z"
            }),
        );
        provisioned
    }

    fn authorize(&self, provisioned: &Provisioned, operation_key: &str) -> Value {
        let task_id = format!("e2e-task:{operation_key}");
        let response = self.post(
            "/spend/authorize",
            json!({
                "operation_key":operation_key,
                "account_id":provisioned.account_id,
                "amount_cents":AUTHORIZED_MINOR,
                "task_id":task_id,
                "reason":"Deterministic Gongbu executor test",
                "workload_profile":"image_generation",
                "execution_scope": {
                    "schema_version":1,
                    "provider":"provider:local:fixture",
                    "executor":"executor:gongbu:image",
                    "capability":"capability:image:generate",
                    "billing_merchant":"merchant:local"
                }
            }),
        );
        assert_eq!(response["decision"], "allow");
        assert_eq!(response["task_id"], task_id);
        assert_eq!(response["reason"], "Deterministic Gongbu executor test");
        assert_eq!(response["payment"], Value::Null);
        assert_eq!(response["budget_hold"]["status"], "frozen");
        response
    }

    fn get(&self, path: &str) -> Value {
        self.response(
            self.client
                .get(format!("{}{path}", self.base_url))
                .bearer_auth(AUTH_TOKEN)
                .send()
                .expect("send Hubu GET"),
        )
    }

    fn post(&self, path: &str, body: Value) -> Value {
        self.response(
            self.client
                .post(format!("{}{path}", self.base_url))
                .bearer_auth(AUTH_TOKEN)
                .json(&body)
                .send()
                .expect("send Hubu POST"),
        )
    }

    fn response(&self, response: reqwest::blocking::Response) -> Value {
        let status = response.status();
        let body = response.text().expect("read Hubu response");
        assert!(status.is_success(), "Hubu returned {status}: {body}");
        serde_json::from_str(&body).expect("parse Hubu response")
    }
}

struct TestWorkspace {
    directory: Option<TempDir>,
    server: Child,
    address: SocketAddr,
    admin: HubuAdmin,
    log_path: PathBuf,
    success: bool,
}

impl TestWorkspace {
    fn start() -> Self {
        let directory = tempdir().expect("create E2E state directory");
        let address = reserve_address();
        let base_url = format!("http://{address}");
        let log_path = directory.path().join("hubu-server.jsonl");
        let spend_timing = directory.path().join("spend-timing.yaml");
        fs::write(
            &spend_timing,
            "default_profile: default\nprofiles:\n  default:\n    authorization_ttl_seconds: 300\n    claim_ttl_seconds: 900\n  image_generation:\n    authorization_ttl_seconds: 300\n    claim_ttl_seconds: 900\n",
        )
        .expect("write spend timing config");
        let log = File::create(&log_path).expect("create Hubu log");
        let server_bin = env::var_os("HUBU_SERVER_BIN")
            .map(PathBuf::from)
            .expect("HUBU_SERVER_BIN is set by the E2E runner");
        let mut server = Command::new(server_bin)
            .arg(address.to_string())
            .env("HUBU_DB_PATH", directory.path().join("hubu.sqlite3"))
            .env("HUBU_AUTH_TOKEN", AUTH_TOKEN)
            .env("HUBU_RECONCILIATION_TOKEN", RECONCILIATION_TOKEN)
            .env("HUBU_SPEND_TIMING_CONFIG", &spend_timing)
            .stdout(Stdio::from(log.try_clone().unwrap()))
            .stderr(Stdio::from(log))
            .spawn()
            .expect("start real local hubu-server");
        let client = Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        for _ in 0..100 {
            if client
                .get(format!("{base_url}/health"))
                .send()
                .is_ok_and(|response| response.status().is_success())
            {
                return Self {
                    directory: Some(directory),
                    server,
                    address,
                    admin: HubuAdmin { base_url, client },
                    log_path,
                    success: false,
                };
            }
            thread::sleep(Duration::from_millis(25));
        }
        let _ = server.kill();
        let _ = server.wait();
        panic!(
            "hubu-server did not become ready; diagnostic state: {}",
            directory.path().display()
        );
    }

    fn root(&self) -> &Path {
        self.directory.as_ref().unwrap().path()
    }

    fn base_url(&self) -> &str {
        &self.admin.base_url
    }

    fn hubu_client(&self) -> HubuClient {
        HubuClient::new(self.base_url()).with_bearer_token(AUTH_TOKEN.as_bytes())
    }

    fn mark_success(&mut self) {
        self.success = true;
    }
}

impl Drop for TestWorkspace {
    fn drop(&mut self) {
        let _ = self.server.kill();
        let _ = self.server.wait();
        if !self.success || thread::panicking() {
            if let Some(directory) = self.directory.take() {
                let path = directory.keep();
                eprintln!(
                    "HUB-83 E2E diagnostic state preserved at {} (Hubu address was {})",
                    path.display(),
                    self.address
                );
            }
        }
    }
}

fn reserve_address() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("reserve loopback port");
    listener.local_addr().unwrap()
}

fn assert_terminal_claim(
    workspace: &TestWorkspace,
    claim_id: &str,
    status: &str,
    expected_total_consumed: i64,
) {
    let claim = workspace
        .hubu_client()
        .inspect_claim(claim_id)
        .expect("inspect Hubu claim over HTTP");
    assert_eq!(claim.status, status);
    assert!(claim.finalized_at.is_some());
    assert!(!claim.reconciliation_required);
    assert_eq!(claim.spend.budget_hold.status, status);
    assert_eq!(claim.spend.budget_hold.frozen_amount_cents, 0);
    assert_eq!(
        claim.spend.budget_hold.consumed_amount_cents,
        expected_total_consumed
    );
}

fn string_at(value: &Value, field: &str) -> String {
    value[field]
        .as_str()
        .unwrap_or_else(|| panic!("missing string field {field}: {value}"))
        .to_string()
}
