//! Temporal registration for the durable execution workflow.
//!
//! Executions expose each durable phase as a separate activity. The workflow
//! uses the persisted state machine in [`crate::workflow`] to make duplicate
//! activity delivery side-effect safe.

use crate::{
    execution::Repository,
    provider::http_kernel::TEMPORAL_ACTIVITY_TIMEOUT,
    workflow::{
        ArtifactActivities, ExecutionPhaseResult, ExecutionWorkflow, HubuActivities,
        OperatorReconciliationRequest, ProviderActivities, ProviderPhaseOutcome,
    },
};
use futures::{channel::oneshot, future::poll_fn, Future};
use serde::{Deserialize, Serialize};
use std::time::SystemTime;
use std::{sync::Arc, time::Duration};
use temporalio_client::{tonic::Code, Client, WorkflowSignalOptions, WorkflowStartOptions};
use temporalio_common_wasm::protos::temporal::api::enums::v1::{
    TaskQueueType, WorkflowIdConflictPolicy, WorkflowIdReusePolicy,
};
use temporalio_common_wasm::protos::temporal::api::{
    taskqueue::v1::TaskQueue, workflowservice::v1::DescribeTaskQueueRequest,
};
use temporalio_macros::{activities, workflow, workflow_methods};
use temporalio_sdk::{
    activities::{ActivityContext, ActivityError},
    ActivityOptions, ApplicationFailure, Runtime, SyncWorkflowContext, Worker, WorkerOptions,
    WorkflowContext, WorkflowContextView, WorkflowResult,
};

pub const EXECUTION_TASK_QUEUE: &str = "gongbu-executions";
const HUB_170_SUBMIT_POLL_PATCH: &str = "hub-170-flux-submit-poll";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemporalWorkerConfig {
    pub task_queue: String,
    pub recovery_delays_seconds: Vec<u64>,
}

impl Default for TemporalWorkerConfig {
    fn default() -> Self {
        Self {
            task_queue: EXECUTION_TASK_QUEUE.into(),
            recovery_delays_seconds: vec![30, 120, 600],
        }
    }
}

impl TemporalWorkerConfig {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.task_queue.trim().is_empty()
            || self.task_queue.len() > 255
            || self.recovery_delays_seconds.is_empty()
            || self.recovery_delays_seconds.len() > 8
            || self.recovery_delays_seconds.contains(&0)
        {
            return Err("invalid Temporal worker configuration");
        }
        Ok(())
    }
}

/// Ask Temporal for the active workflow pollers on Gongbu's configured queue.
/// This is startup proof only; runtime health uses [`temporal_is_reachable`].
pub async fn worker_is_polling(
    client: &Client,
    namespace: &str,
    task_queue: &str,
) -> Result<bool, temporalio_client::tonic::Status> {
    run_worker_poll_probe(|| describe_worker_pollers(client, namespace, task_queue)).await
}

/// Prove that Temporal remains reachable after startup. A successful task-queue
/// description is reachability evidence even when its historical poller list is
/// empty; only startup uses that list as proof that Gongbu's worker is polling.
pub async fn temporal_is_reachable(
    client: &Client,
    namespace: &str,
    task_queue: &str,
) -> Result<(), temporalio_client::tonic::Status> {
    runtime_reachability(worker_is_polling(client, namespace, task_queue).await)
}

fn runtime_reachability(
    poller_probe: Result<bool, temporalio_client::tonic::Status>,
) -> Result<(), temporalio_client::tonic::Status> {
    poller_probe.map(|_| ())
}

async fn run_worker_poll_probe<F, Fut>(
    mut probe: F,
) -> Result<bool, temporalio_client::tonic::Status>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<bool, temporalio_client::tonic::Status>>,
{
    let result = probe().await;
    // DescribeTaskQueue is a normal RPC, so Temporal client 0.6 forwards a
    // plain transport-rotation cancellation without retrying it. Confirm that
    // one inconclusive result exactly once so a known one-off rotation does not
    // enter dependency degradation. The confirmation is authoritative; the
    // runtime health state machine bounds an unresolved cancellation while
    // every proven negative or other error remains immediately fail-closed.
    if result
        .as_ref()
        .is_err_and(|error| error.code() == Code::Cancelled)
    {
        return probe().await;
    }
    result
}

async fn describe_worker_pollers(
    client: &Client,
    namespace: &str,
    task_queue: &str,
) -> Result<bool, temporalio_client::tonic::Status> {
    #[allow(deprecated)]
    let request = DescribeTaskQueueRequest {
        namespace: namespace.into(),
        task_queue: Some(TaskQueue {
            name: task_queue.into(),
            ..Default::default()
        }),
        task_queue_type: TaskQueueType::Workflow as i32,
        report_pollers: true,
        ..Default::default()
    };
    let response = client
        .connection()
        .workflow_service()
        .describe_task_queue(temporalio_client::tonic::Request::new(request))
        .await?;
    Ok(!response.into_inner().pollers.is_empty())
}

pub trait DurableExecutionRunner: Send + Sync + 'static {
    fn run_execution(
        &self,
        execution_id: &str,
        activity_deadline: Option<SystemTime>,
    ) -> Result<String, String>;
    fn recover_execution(
        &self,
        execution_id: &str,
        operator: Option<&OperatorReconciliationRequest>,
        exhausted: bool,
    ) -> Result<String, String>;
    fn preflight_execution(
        &self,
        execution_id: &str,
        activity_deadline: Option<SystemTime>,
    ) -> Result<ExecutionPhaseResult, String>;
    fn claim_authorization(
        &self,
        execution_id: &str,
        activity_deadline: Option<SystemTime>,
    ) -> Result<ExecutionPhaseResult, String>;
    fn validate_claim(
        &self,
        execution_id: &str,
        activity_deadline: Option<SystemTime>,
    ) -> Result<ExecutionPhaseResult, String>;
    fn execute_provider(
        &self,
        execution_id: &str,
        activity_deadline: Option<SystemTime>,
    ) -> Result<ProviderPhaseOutcome, String>;
    fn submit_provider(
        &self,
        execution_id: &str,
        activity_deadline: Option<SystemTime>,
    ) -> Result<ProviderPhaseOutcome, String>;
    fn poll_provider_operation(
        &self,
        execution_id: &str,
        activity_deadline: Option<SystemTime>,
    ) -> Result<ProviderPhaseOutcome, String>;
    fn persist_artifacts(
        &self,
        execution_id: &str,
        activity_deadline: Option<SystemTime>,
    ) -> Result<ExecutionPhaseResult, String>;
    fn release_authorization(
        &self,
        execution_id: &str,
        activity_deadline: Option<SystemTime>,
    ) -> Result<ExecutionPhaseResult, String>;
    fn settle_spend(
        &self,
        execution_id: &str,
        activity_deadline: Option<SystemTime>,
    ) -> Result<ExecutionPhaseResult, String>;
}

pub struct PersistedExecutionRunner {
    repository: Repository,
    hubu: Arc<dyn HubuActivities + Send + Sync>,
    provider: Arc<dyn ProviderActivities + Send + Sync>,
    artifacts: Arc<dyn ArtifactActivities + Send + Sync>,
    now: Arc<dyn Fn() -> String + Send + Sync>,
}

impl PersistedExecutionRunner {
    pub fn new(
        repository: Repository,
        hubu: Arc<dyn HubuActivities + Send + Sync>,
        provider: Arc<dyn ProviderActivities + Send + Sync>,
        artifacts: Arc<dyn ArtifactActivities + Send + Sync>,
        now: impl Fn() -> String + Send + Sync + 'static,
    ) -> Self {
        Self {
            repository,
            hubu,
            provider,
            artifacts,
            now: Arc::new(now),
        }
    }

    /// Direct/local execution has no Temporal activity deadline to inherit.
    pub fn run_execution(&self, execution_id: &str) -> Result<String, String> {
        <Self as DurableExecutionRunner>::run_execution(self, execution_id, None)
    }
    fn workflow(&self) -> ExecutionWorkflow<'_> {
        ExecutionWorkflow {
            repository: &self.repository,
            hubu: self.hubu.as_ref(),
            provider: self.provider.as_ref(),
            artifacts: self.artifacts.as_ref(),
        }
    }
    fn with_deadline<T>(
        &self,
        activity_deadline: Option<SystemTime>,
        operation: impl FnOnce(
            &ExecutionWorkflow<'_>,
            &str,
        ) -> Result<T, crate::workflow::WorkflowError>,
    ) -> Result<T, String> {
        let _deadline =
            crate::provider::http_kernel::ActivityDeadlineGuard::enter(activity_deadline)
                .map_err(|error| error.to_string())?;
        operation(&self.workflow(), &(self.now)()).map_err(|error| error.to_string())
    }
}

impl DurableExecutionRunner for PersistedExecutionRunner {
    fn run_execution(
        &self,
        execution_id: &str,
        activity_deadline: Option<SystemTime>,
    ) -> Result<String, String> {
        let _deadline =
            crate::provider::http_kernel::ActivityDeadlineGuard::enter(activity_deadline)
                .map_err(|error| error.to_string())?;
        self.workflow()
            .run_with_clock(execution_id, self.now.as_ref())
            .map(|execution| execution.status)
            .map_err(|error| error.to_string())
    }
    fn recover_execution(
        &self,
        execution_id: &str,
        operator: Option<&OperatorReconciliationRequest>,
        exhausted: bool,
    ) -> Result<String, String> {
        if operator.is_none() {
            self.repository
                .mark_recovery_attempt(execution_id, &(self.now)(), exhausted)
                .map_err(|e| e.to_string())?;
        }
        self.workflow()
            .recover(execution_id, &(self.now)(), operator)
            .map(|e| e.status)
            .map_err(|e| e.to_string())
    }
    fn preflight_execution(
        &self,
        execution_id: &str,
        activity_deadline: Option<SystemTime>,
    ) -> Result<ExecutionPhaseResult, String> {
        self.with_deadline(activity_deadline, |workflow, now| {
            workflow.preflight_phase(execution_id, now).map(Into::into)
        })
    }
    fn claim_authorization(
        &self,
        execution_id: &str,
        activity_deadline: Option<SystemTime>,
    ) -> Result<ExecutionPhaseResult, String> {
        self.with_deadline(activity_deadline, |workflow, now| {
            workflow.claim_phase(execution_id, now).map(Into::into)
        })
    }
    fn validate_claim(
        &self,
        execution_id: &str,
        activity_deadline: Option<SystemTime>,
    ) -> Result<ExecutionPhaseResult, String> {
        self.with_deadline(activity_deadline, |workflow, now| {
            workflow
                .validate_claim_phase(execution_id, now)
                .map(Into::into)
        })
    }
    fn execute_provider(
        &self,
        execution_id: &str,
        activity_deadline: Option<SystemTime>,
    ) -> Result<ProviderPhaseOutcome, String> {
        let _deadline =
            crate::provider::http_kernel::ActivityDeadlineGuard::enter(activity_deadline)
                .map_err(|error| error.to_string())?;
        let phase_at = (self.now)();
        self.workflow()
            .provider_phase_with_clock(execution_id, &phase_at, self.now.as_ref())
            .map_err(|error| error.to_string())
    }
    fn submit_provider(
        &self,
        execution_id: &str,
        activity_deadline: Option<SystemTime>,
    ) -> Result<ProviderPhaseOutcome, String> {
        let _deadline =
            crate::provider::http_kernel::ActivityDeadlineGuard::enter(activity_deadline)
                .map_err(|error| error.to_string())?;
        let phase_at = (self.now)();
        self.workflow()
            .provider_submit_phase_with_clock(execution_id, &phase_at, self.now.as_ref())
            .map_err(|error| error.to_string())
    }
    fn poll_provider_operation(
        &self,
        execution_id: &str,
        activity_deadline: Option<SystemTime>,
    ) -> Result<ProviderPhaseOutcome, String> {
        let _deadline =
            crate::provider::http_kernel::ActivityDeadlineGuard::enter(activity_deadline)
                .map_err(|error| error.to_string())?;
        let phase_at = (self.now)();
        self.workflow()
            .provider_poll_phase_with_clock(execution_id, &phase_at, self.now.as_ref())
            .map_err(|error| error.to_string())
    }
    fn persist_artifacts(
        &self,
        execution_id: &str,
        activity_deadline: Option<SystemTime>,
    ) -> Result<ExecutionPhaseResult, String> {
        self.with_deadline(activity_deadline, |workflow, now| {
            workflow.artifact_phase(execution_id, now).map(Into::into)
        })
    }
    fn release_authorization(
        &self,
        execution_id: &str,
        activity_deadline: Option<SystemTime>,
    ) -> Result<ExecutionPhaseResult, String> {
        self.with_deadline(activity_deadline, |workflow, now| {
            workflow.release_phase(execution_id, now).map(Into::into)
        })
    }
    fn settle_spend(
        &self,
        execution_id: &str,
        activity_deadline: Option<SystemTime>,
    ) -> Result<ExecutionPhaseResult, String> {
        self.with_deadline(activity_deadline, |workflow, now| {
            workflow.settlement_phase(execution_id, now).map(Into::into)
        })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ExecutionWorkflowInput {
    pub execution_id: String,
    pub recovery_delays_seconds: Vec<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RecoveryActivityInput {
    pub execution_id: String,
    pub operator: Option<OperatorReconciliationRequest>,
    pub exhausted: bool,
}

#[workflow]
#[derive(Default)]
pub struct GranularExecutionWorkflow {
    pending: Vec<OperatorReconciliationRequest>,
}

#[workflow_methods]
impl GranularExecutionWorkflow {
    #[run]
    pub async fn run(
        ctx: &mut WorkflowContext<Self>,
        input: ExecutionWorkflowInput,
    ) -> WorkflowResult<String> {
        let ExecutionWorkflowInput {
            execution_id,
            recovery_delays_seconds,
        } = input;
        let options = ActivityOptions::start_to_close_timeout(TEMPORAL_ACTIVITY_TIMEOUT);
        let mut status = ctx
            .execute_activity(
                GranularExecutionActivities::preflight_execution,
                execution_id.clone(),
                options.clone(),
            )
            .await?
            .status;
        if status == "preflighting" {
            status = ctx
                .execute_activity(
                    GranularExecutionActivities::claim_authorization,
                    execution_id.clone(),
                    options.clone(),
                )
                .await?
                .status;
        }
        if status == "claimed" {
            status = ctx
                .execute_activity(
                    GranularExecutionActivities::validate_claim,
                    execution_id.clone(),
                    options.clone(),
                )
                .await?
                .status;
        }
        if status == "executing" {
            let provider_outcome = if ctx.patched(HUB_170_SUBMIT_POLL_PATCH) {
                let submitted = ctx
                    .execute_activity(
                        GranularExecutionActivities::submit_provider,
                        execution_id.clone(),
                        options.clone(),
                    )
                    .await?;
                if submitted == ProviderPhaseOutcome::PollExisting {
                    ctx.execute_activity(
                        GranularExecutionActivities::poll_provider_operation,
                        execution_id.clone(),
                        options.clone(),
                    )
                    .await?
                } else {
                    submitted
                }
            } else {
                // Existing Temporal histories retain the original activity
                // command and its single-activity submit/poll wrapper.
                ctx.execute_activity(
                    GranularExecutionActivities::execute_provider,
                    execution_id.clone(),
                    options.clone(),
                )
                .await?
            };
            match provider_outcome {
                ProviderPhaseOutcome::PersistArtifacts => {
                    status = ctx
                        .execute_activity(
                            GranularExecutionActivities::persist_artifacts,
                            execution_id.clone(),
                            options.clone(),
                        )
                        .await?
                        .status;
                }
                ProviderPhaseOutcome::ReleaseAuthorization => {
                    status = ctx
                        .execute_activity(
                            GranularExecutionActivities::release_authorization,
                            execution_id.clone(),
                            options.clone(),
                        )
                        .await?
                        .status;
                }
                ProviderPhaseOutcome::PollExisting => {
                    return Err(ApplicationFailure::new(std::io::Error::other(
                        "provider poll phase remained pending",
                    ))
                    .into());
                }
                ProviderPhaseOutcome::Complete(completed) => status = completed.status,
            }
        }
        if status == "settling" {
            status = ctx
                .execute_activity(
                    GranularExecutionActivities::settle_spend,
                    execution_id.clone(),
                    options.clone(),
                )
                .await?
                .status;
        }
        if status != "reconciliation_required" {
            return Ok(status);
        }

        let delay_count = recovery_delays_seconds.len();
        for (index, delay) in recovery_delays_seconds.into_iter().enumerate() {
            let mut timer = ctx.timer(Duration::from_secs(delay));
            loop {
                let automatic = temporalio_sdk::workflows::select! {
                    _ = &mut timer => true,
                    _ = ctx.wait_condition(|state: &Self| !state.pending.is_empty()) => false,
                };
                let operator = if automatic {
                    None
                } else {
                    ctx.state_mut(|state| state.pending.pop())
                };
                status = ctx
                    .execute_activity(
                        GranularExecutionActivities::perform_reconciliation,
                        RecoveryActivityInput {
                            execution_id: execution_id.clone(),
                            exhausted: automatic && index + 1 == delay_count,
                            operator,
                        },
                        options.clone(),
                    )
                    .await?;
                if status != "reconciliation_required" {
                    return Ok(status);
                }
                if automatic {
                    break;
                }
            }
        }
        loop {
            ctx.wait_condition(|state: &Self| !state.pending.is_empty())
                .await;
            let operator = ctx.state_mut(|state| state.pending.pop());
            status = ctx
                .execute_activity(
                    GranularExecutionActivities::perform_reconciliation,
                    RecoveryActivityInput {
                        execution_id: execution_id.clone(),
                        operator,
                        exhausted: false,
                    },
                    options.clone(),
                )
                .await?;
            if status != "reconciliation_required" {
                return Ok(status);
            }
        }
    }

    #[signal]
    pub fn reconcile(
        &mut self,
        _ctx: &mut SyncWorkflowContext<Self>,
        request: OperatorReconciliationRequest,
    ) {
        self.pending.push(request);
    }

    #[query]
    pub fn pending_reconciliation_actions(&self, _ctx: &WorkflowContextView) -> usize {
        self.pending.len()
    }
}

#[derive(Clone)]
pub struct GranularExecutionActivities {
    runner: Arc<dyn DurableExecutionRunner>,
}

impl GranularExecutionActivities {
    pub fn new(runner: Arc<dyn DurableExecutionRunner>) -> Self {
        Self { runner }
    }
}

fn activity_failure(message: impl ToString) -> ActivityError {
    ActivityError::application(ApplicationFailure::new(std::io::Error::other(
        message.to_string(),
    )))
}

#[activities]
impl GranularExecutionActivities {
    #[activity]
    pub async fn preflight_execution(
        self: Arc<Self>,
        ctx: ActivityContext,
        execution_id: String,
    ) -> Result<ExecutionPhaseResult, ActivityError> {
        let runner = Arc::clone(&self.runner);
        let deadline = ctx.info().deadline;
        tokio::task::spawn_blocking(move || runner.preflight_execution(&execution_id, deadline))
            .await
            .map_err(activity_failure)?
            .map_err(activity_failure)
    }

    #[activity]
    pub async fn claim_authorization(
        self: Arc<Self>,
        ctx: ActivityContext,
        execution_id: String,
    ) -> Result<ExecutionPhaseResult, ActivityError> {
        let runner = Arc::clone(&self.runner);
        let deadline = ctx.info().deadline;
        tokio::task::spawn_blocking(move || runner.claim_authorization(&execution_id, deadline))
            .await
            .map_err(activity_failure)?
            .map_err(activity_failure)
    }

    #[activity]
    pub async fn validate_claim(
        self: Arc<Self>,
        ctx: ActivityContext,
        execution_id: String,
    ) -> Result<ExecutionPhaseResult, ActivityError> {
        let runner = Arc::clone(&self.runner);
        let deadline = ctx.info().deadline;
        tokio::task::spawn_blocking(move || runner.validate_claim(&execution_id, deadline))
            .await
            .map_err(activity_failure)?
            .map_err(activity_failure)
    }

    #[activity]
    pub async fn execute_provider(
        self: Arc<Self>,
        ctx: ActivityContext,
        execution_id: String,
    ) -> Result<ProviderPhaseOutcome, ActivityError> {
        let runner = Arc::clone(&self.runner);
        let deadline = ctx.info().deadline;
        tokio::task::spawn_blocking(move || runner.execute_provider(&execution_id, deadline))
            .await
            .map_err(activity_failure)?
            .map_err(activity_failure)
    }

    #[activity]
    pub async fn submit_provider(
        self: Arc<Self>,
        ctx: ActivityContext,
        execution_id: String,
    ) -> Result<ProviderPhaseOutcome, ActivityError> {
        let runner = Arc::clone(&self.runner);
        let deadline = ctx.info().deadline;
        tokio::task::spawn_blocking(move || runner.submit_provider(&execution_id, deadline))
            .await
            .map_err(activity_failure)?
            .map_err(activity_failure)
    }

    #[activity]
    pub async fn poll_provider_operation(
        self: Arc<Self>,
        ctx: ActivityContext,
        execution_id: String,
    ) -> Result<ProviderPhaseOutcome, ActivityError> {
        let runner = Arc::clone(&self.runner);
        let deadline = ctx.info().deadline;
        tokio::task::spawn_blocking(move || runner.poll_provider_operation(&execution_id, deadline))
            .await
            .map_err(activity_failure)?
            .map_err(activity_failure)
    }

    #[activity]
    pub async fn persist_artifacts(
        self: Arc<Self>,
        ctx: ActivityContext,
        execution_id: String,
    ) -> Result<ExecutionPhaseResult, ActivityError> {
        let runner = Arc::clone(&self.runner);
        let deadline = ctx.info().deadline;
        tokio::task::spawn_blocking(move || runner.persist_artifacts(&execution_id, deadline))
            .await
            .map_err(activity_failure)?
            .map_err(activity_failure)
    }

    #[activity]
    pub async fn release_authorization(
        self: Arc<Self>,
        ctx: ActivityContext,
        execution_id: String,
    ) -> Result<ExecutionPhaseResult, ActivityError> {
        let runner = Arc::clone(&self.runner);
        let deadline = ctx.info().deadline;
        tokio::task::spawn_blocking(move || runner.release_authorization(&execution_id, deadline))
            .await
            .map_err(activity_failure)?
            .map_err(activity_failure)
    }

    #[activity]
    pub async fn settle_spend(
        self: Arc<Self>,
        ctx: ActivityContext,
        execution_id: String,
    ) -> Result<ExecutionPhaseResult, ActivityError> {
        let runner = Arc::clone(&self.runner);
        let deadline = ctx.info().deadline;
        tokio::task::spawn_blocking(move || runner.settle_spend(&execution_id, deadline))
            .await
            .map_err(activity_failure)?
            .map_err(activity_failure)
    }

    #[activity]
    pub async fn perform_reconciliation(
        self: Arc<Self>,
        _ctx: ActivityContext,
        input: RecoveryActivityInput,
    ) -> Result<String, ActivityError> {
        let runner = Arc::clone(&self.runner);
        tokio::task::spawn_blocking(move || {
            runner.recover_execution(
                &input.execution_id,
                input.operator.as_ref(),
                input.exhausted,
            )
        })
        .await
        .map_err(activity_failure)?
        .map_err(activity_failure)
    }
}

pub fn worker_options(runner: Arc<dyn DurableExecutionRunner>) -> WorkerOptions {
    worker_options_for(runner, EXECUTION_TASK_QUEUE)
}

pub fn worker_options_for(
    runner: Arc<dyn DurableExecutionRunner>,
    task_queue: &str,
) -> WorkerOptions {
    WorkerOptions::new(task_queue)
        .register_workflow::<GranularExecutionWorkflow>()
        .expect("granular execution workflow registration is valid")
        .register_activities(GranularExecutionActivities::new(runner))
        .build()
}

pub async fn run_worker(
    runtime: &Runtime,
    client: Client,
    runner: Arc<dyn DurableExecutionRunner>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut worker = Worker::new(runtime, client, worker_options(runner))
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    worker
        .run()
        .await
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    Ok(())
}

#[derive(Clone, Copy, Debug, thiserror::Error, PartialEq, Eq)]
pub enum ScheduleError {
    #[error("execution scheduler unavailable")]
    Unavailable,
}

pub trait ExecutionScheduler: Send + Sync + 'static {
    fn schedule(&self, execution_id: &str) -> Result<(), ScheduleError>;
    fn reconcile(
        &self,
        execution_id: &str,
        request: OperatorReconciliationRequest,
    ) -> Result<(), ScheduleError>;
}

#[derive(Clone)]
pub struct TemporalExecutionScheduler {
    requests: std::sync::mpsc::SyncSender<ScheduleRequest>,
}

impl TemporalExecutionScheduler {
    fn new(requests: std::sync::mpsc::SyncSender<ScheduleRequest>) -> Self {
        Self { requests }
    }
}

type ScheduleResult = Result<(), ScheduleError>;
enum SchedulerCommand {
    Start(String),
    Reconcile(String, OperatorReconciliationRequest),
}
type ScheduleRequest = (
    SchedulerCommand,
    std::sync::mpsc::SyncSender<ScheduleResult>,
);

pub struct StartedTemporalWorker {
    pub scheduler: Arc<TemporalExecutionScheduler>,
    pub thread: std::thread::JoinHandle<Result<(), Box<dyn std::error::Error + Send + Sync>>>,
    scheduler_thread: std::thread::JoinHandle<()>,
    shutdown: Arc<dyn Fn() + Send + Sync>,
    completion: Option<oneshot::Receiver<()>>,
}

impl StartedTemporalWorker {
    pub fn shutdown(&self) {
        (self.shutdown)();
    }

    pub fn take_completion(&mut self) -> oneshot::Receiver<()> {
        self.completion
            .take()
            .expect("worker completion receiver is taken once")
    }

    pub fn join(self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let Self {
            scheduler,
            thread,
            scheduler_thread,
            ..
        } = self;
        drop(scheduler);
        scheduler_thread
            .join()
            .map_err(|_| std::io::Error::other("Temporal scheduler thread panicked"))?;
        thread
            .join()
            .map_err(|_| std::io::Error::other("Temporal worker thread panicked"))??;
        Ok(())
    }
}

pub fn start_worker(
    runtime: Arc<Runtime>,
    client: Client,
    runner: Arc<dyn DurableExecutionRunner>,
) -> Result<StartedTemporalWorker, Box<dyn std::error::Error + Send + Sync>> {
    start_worker_with_config(runtime, client, runner, TemporalWorkerConfig::default())
}

pub fn start_worker_with_config(
    runtime: Arc<Runtime>,
    client: Client,
    runner: Arc<dyn DurableExecutionRunner>,
    config: TemporalWorkerConfig,
) -> Result<StartedTemporalWorker, Box<dyn std::error::Error + Send + Sync>> {
    config.validate().map_err(std::io::Error::other)?;
    let worker_task_queue = config.task_queue.clone();
    let scheduler_task_queue = config.task_queue;
    let recovery_delays = config.recovery_delays_seconds;
    let scheduler_client = client.clone();
    let (startup_tx, startup_rx) = std::sync::mpsc::sync_channel(1);
    let (completion_tx, completion_rx) = oneshot::channel();
    let thread = std::thread::Builder::new()
        .name("gongbu-temporal-worker".into())
        .spawn(move || {
            let tokio_runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            let run_result = tokio_runtime.block_on(async move {
                // Worker construction starts Tokio tasks in newer SDK releases,
                // so it must happen after entering this thread's runtime.
                let mut worker = Worker::new(
                    &runtime,
                    client,
                    worker_options_for(runner, &worker_task_queue),
                )
                .map_err(|error| std::io::Error::other(error.to_string()))?;
                let shutdown: Arc<dyn Fn() + Send + Sync> = Arc::new(worker.shutdown_handle());
                let mut run = Box::pin(worker.run());
                let mut readiness = Some((startup_tx, shutdown));
                poll_fn(move |cx| match run.as_mut().poll(cx) {
                    std::task::Poll::Ready(result) => std::task::Poll::Ready(result),
                    std::task::Poll::Pending => {
                        if let Some((ready, shutdown)) = readiness.take() {
                            ready.send(shutdown).map_err(|_| {
                                std::io::Error::other("worker startup receiver dropped")
                            })?;
                        }
                        std::task::Poll::Pending
                    }
                })
                .await
                .map_err(|error| std::io::Error::other(error.to_string()))?;
                Ok(())
            });
            let _ = completion_tx.send(());
            run_result
        })?;
    let (request_tx, request_rx) = std::sync::mpsc::sync_channel::<ScheduleRequest>(16);
    let scheduler_thread = std::thread::Builder::new()
        .name("gongbu-temporal-scheduler".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            let Ok(runtime) = runtime else { return };
            while let Ok((command, response)) = request_rx.recv() {
                let result = runtime.block_on(async {
                    match command {
                        SchedulerCommand::Start(execution_id) => {
                            start_execution(
                                scheduler_client.clone(),
                                execution_id,
                                &scheduler_task_queue,
                                &recovery_delays,
                            )
                            .await
                        }
                        SchedulerCommand::Reconcile(execution_id, request) => {
                            signal_reconciliation(
                                scheduler_client.clone(),
                                execution_id,
                                request,
                                &scheduler_task_queue,
                                &recovery_delays,
                            )
                            .await
                        }
                    }
                });
                let _ = response.send(result);
            }
        })?;
    let shutdown = startup_rx.recv()?;
    let scheduler = Arc::new(TemporalExecutionScheduler::new(request_tx));
    Ok(StartedTemporalWorker {
        scheduler,
        thread,
        scheduler_thread,
        shutdown,
        completion: Some(completion_rx),
    })
}

impl ExecutionScheduler for TemporalExecutionScheduler {
    fn schedule(&self, execution_id: &str) -> Result<(), ScheduleError> {
        let (response_tx, response_rx) = std::sync::mpsc::sync_channel(1);
        self.requests
            .send((
                SchedulerCommand::Start(execution_id.to_owned()),
                response_tx,
            ))
            .map_err(|_| ScheduleError::Unavailable)?;
        response_rx.recv().map_err(|_| ScheduleError::Unavailable)?
    }
    fn reconcile(
        &self,
        execution_id: &str,
        request: OperatorReconciliationRequest,
    ) -> Result<(), ScheduleError> {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        self.requests
            .send((
                SchedulerCommand::Reconcile(execution_id.to_owned(), request),
                tx,
            ))
            .map_err(|_| ScheduleError::Unavailable)?;
        rx.recv().map_err(|_| ScheduleError::Unavailable)?
    }
}

async fn start_execution(
    client: Client,
    execution_id: String,
    task_queue: &str,
    recovery_delays_seconds: &[u64],
) -> ScheduleResult {
    client
        .start_workflow(
            GranularExecutionWorkflow::run,
            ExecutionWorkflowInput {
                execution_id: execution_id.clone(),
                recovery_delays_seconds: recovery_delays_seconds.to_vec(),
            },
            execution_start_options(&execution_id, task_queue),
        )
        .await
        .map(|_| ())
        .map_err(|_| ScheduleError::Unavailable)
}

fn execution_start_options(execution_id: &str, task_queue: &str) -> WorkflowStartOptions {
    WorkflowStartOptions::new(task_queue, format!("gongbu-execution-{execution_id}"))
        .id_conflict_policy(WorkflowIdConflictPolicy::UseExisting)
        .id_reuse_policy(WorkflowIdReusePolicy::AllowDuplicate)
        .build()
}

async fn signal_reconciliation(
    client: Client,
    execution_id: String,
    request: OperatorReconciliationRequest,
    task_queue: &str,
    recovery_delays_seconds: &[u64],
) -> ScheduleResult {
    // UseExisting preserves a live workflow; AllowDuplicate starts a new run
    // when a pre-HUB-19 workflow with this stable ID already completed.
    start_execution(
        client.clone(),
        execution_id.clone(),
        task_queue,
        recovery_delays_seconds,
    )
    .await?;
    client
        .get_workflow_handle::<GranularExecutionWorkflow>(format!(
            "gongbu-execution-{execution_id}"
        ))
        .signal(
            GranularExecutionWorkflow::reconcile,
            request,
            WorkflowSignalOptions::default(),
        )
        .await
        .map_err(|_| ScheduleError::Unavailable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream;
    use temporalio_common::{
        data_converters::DataConverter,
        protos::{
            constants::PATCH_MARKER_NAME,
            coresdk::{common::build_has_change_marker_details, AsJsonPayloadExt, IntoPayloadsExt},
            temporal::api::{
                common::v1::{ActivityType, Payload, WorkflowType},
                enums::v1::{EventType, TaskQueueKind},
                history::v1::{
                    ActivityTaskCompletedEventAttributes, ActivityTaskScheduledEventAttributes,
                    ActivityTaskStartedEventAttributes, History, HistoryEvent,
                    MarkerRecordedEventAttributes, WorkflowExecutionCompletedEventAttributes,
                    WorkflowExecutionStartedEventAttributes, WorkflowTaskCompletedEventAttributes,
                    WorkflowTaskScheduledEventAttributes, WorkflowTaskStartedEventAttributes,
                },
                taskqueue::v1::TaskQueue,
            },
        },
        worker::WorkerTaskTypes,
        WorkflowDefinition,
    };
    use temporalio_sdk::runtime::{
        replay::{HistoryForReplay, ReplayWorkerInput},
        WorkerVersioningStrategy,
    };

    #[tokio::test]
    async fn cancelled_worker_poll_probe_requires_one_healthy_confirmation() {
        let calls = std::cell::Cell::new(0);
        let mut results = std::collections::VecDeque::from([
            Err(temporalio_client::tonic::Status::cancelled(
                "connection rotation",
            )),
            Ok(true),
        ]);

        let result = run_worker_poll_probe(|| {
            calls.set(calls.get() + 1);
            futures::future::ready(results.pop_front().expect("bounded probe result"))
        })
        .await;

        assert!(result.unwrap());
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn empty_poller_list_is_runtime_reachability_evidence() {
        assert!(runtime_reachability(Ok(false)).is_ok());
        assert!(runtime_reachability(Ok(true)).is_ok());
    }

    #[tokio::test]
    async fn cancelled_worker_poll_confirmation_preserves_negative_and_error_results() {
        for (confirmation, expected_code) in [
            (Ok(false), None),
            (
                Err(temporalio_client::tonic::Status::cancelled(
                    "still cancelled",
                )),
                Some(Code::Cancelled),
            ),
            (
                Err(temporalio_client::tonic::Status::unavailable(
                    "dependency unavailable",
                )),
                Some(Code::Unavailable),
            ),
        ] {
            let calls = std::cell::Cell::new(0);
            let mut results = std::collections::VecDeque::from([
                Err(temporalio_client::tonic::Status::cancelled(
                    "connection rotation",
                )),
                confirmation,
            ]);

            let result = run_worker_poll_probe(|| {
                calls.set(calls.get() + 1);
                futures::future::ready(results.pop_front().expect("bounded probe result"))
            })
            .await;

            match expected_code {
                Some(code) => assert_eq!(result.unwrap_err().code(), code),
                None => assert!(!result.unwrap()),
            }
            assert_eq!(calls.get(), 2);
        }
    }

    #[tokio::test]
    async fn proven_negative_and_non_cancelled_errors_are_not_confirmed() {
        for (initial, expected_code) in [
            (Ok(false), None),
            (
                Err(temporalio_client::tonic::Status::unavailable(
                    "dependency unavailable",
                )),
                Some(Code::Unavailable),
            ),
        ] {
            let calls = std::cell::Cell::new(0);
            let mut results = std::collections::VecDeque::from([initial]);

            let result = run_worker_poll_probe(|| {
                calls.set(calls.get() + 1);
                futures::future::ready(results.pop_front().expect("unexpected confirmation"))
            })
            .await;

            match expected_code {
                Some(code) => assert_eq!(result.unwrap_err().code(), code),
                None => assert!(!result.unwrap()),
            }
            assert_eq!(calls.get(), 1);
        }

        let calls = std::cell::Cell::new(0);
        let mut results = std::collections::VecDeque::from([Ok(true)]);
        let result = run_worker_poll_probe(|| {
            calls.set(calls.get() + 1);
            futures::future::ready(results.pop_front().expect("unexpected confirmation"))
        })
        .await;
        assert!(result.unwrap());
        assert_eq!(calls.get(), 1);
    }

    struct Runner;
    impl DurableExecutionRunner for Runner {
        fn run_execution(&self, _: &str, _: Option<SystemTime>) -> Result<String, String> {
            Ok("succeeded".into())
        }
        fn recover_execution(
            &self,
            _: &str,
            _: Option<&OperatorReconciliationRequest>,
            _: bool,
        ) -> Result<String, String> {
            Ok("reconciliation_required".into())
        }
        fn preflight_execution(
            &self,
            _: &str,
            _: Option<SystemTime>,
        ) -> Result<ExecutionPhaseResult, String> {
            Ok(phase("preflighting"))
        }
        fn claim_authorization(
            &self,
            _: &str,
            _: Option<SystemTime>,
        ) -> Result<ExecutionPhaseResult, String> {
            Ok(phase("claimed"))
        }
        fn validate_claim(
            &self,
            _: &str,
            _: Option<SystemTime>,
        ) -> Result<ExecutionPhaseResult, String> {
            Ok(phase("executing"))
        }
        fn execute_provider(
            &self,
            _: &str,
            _: Option<SystemTime>,
        ) -> Result<ProviderPhaseOutcome, String> {
            Ok(ProviderPhaseOutcome::PersistArtifacts)
        }
        fn submit_provider(
            &self,
            _: &str,
            _: Option<SystemTime>,
        ) -> Result<ProviderPhaseOutcome, String> {
            Ok(ProviderPhaseOutcome::PollExisting)
        }
        fn poll_provider_operation(
            &self,
            _: &str,
            _: Option<SystemTime>,
        ) -> Result<ProviderPhaseOutcome, String> {
            Ok(ProviderPhaseOutcome::PersistArtifacts)
        }
        fn persist_artifacts(
            &self,
            _: &str,
            _: Option<SystemTime>,
        ) -> Result<ExecutionPhaseResult, String> {
            Ok(phase("settling"))
        }
        fn release_authorization(
            &self,
            _: &str,
            _: Option<SystemTime>,
        ) -> Result<ExecutionPhaseResult, String> {
            Ok(phase("released"))
        }
        fn settle_spend(
            &self,
            _: &str,
            _: Option<SystemTime>,
        ) -> Result<ExecutionPhaseResult, String> {
            Ok(phase("succeeded"))
        }
    }

    fn phase(status: &str) -> ExecutionPhaseResult {
        ExecutionPhaseResult {
            status: status.into(),
            failure_code: None,
        }
    }

    struct ReplayHistoryBuilder {
        events: Vec<HistoryEvent>,
        event_id: i64,
        workflow_task_scheduled_event_id: i64,
        previous_workflow_task_completed_event_id: i64,
    }

    impl ReplayHistoryBuilder {
        fn new() -> Self {
            let mut history = Self {
                events: Vec::new(),
                event_id: 0,
                workflow_task_scheduled_event_id: 0,
                previous_workflow_task_completed_event_id: 0,
            };
            history.add(
                EventType::WorkflowExecutionStarted,
                WorkflowExecutionStartedEventAttributes {
                    original_execution_run_id: "17000000-0000-4000-8000-000000000001".into(),
                    workflow_type: Some(WorkflowType {
                        name: GranularExecutionWorkflow::run.name().to_owned(),
                    }),
                    input: vec![ExecutionWorkflowInput {
                        execution_id: "execution-170".into(),
                        recovery_delays_seconds: vec![1],
                    }
                    .as_json_payload()
                    .unwrap()]
                    .into_payloads(),
                    workflow_task_timeout: Some(Duration::from_secs(5).try_into().unwrap()),
                    task_queue: Some(TaskQueue {
                        name: "hub-170-replay".into(),
                        kind: TaskQueueKind::Normal as i32,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            );
            history.add_full_workflow_task();
            history
        }

        fn add(
            &mut self,
            event_type: EventType,
            attributes: impl Into<HistoryEventAttributes>,
        ) -> i64 {
            self.event_id += 1;
            self.events.push(HistoryEvent {
                event_id: self.event_id,
                event_type: event_type as i32,
                attributes: Some(attributes.into()),
                ..Default::default()
            });
            self.event_id
        }

        fn add_full_workflow_task(&mut self) {
            self.workflow_task_scheduled_event_id = self.add(
                EventType::WorkflowTaskScheduled,
                WorkflowTaskScheduledEventAttributes::default(),
            );
            self.add(
                EventType::WorkflowTaskStarted,
                WorkflowTaskStartedEventAttributes {
                    scheduled_event_id: self.workflow_task_scheduled_event_id,
                    ..Default::default()
                },
            );
            self.previous_workflow_task_completed_event_id = self.add(
                EventType::WorkflowTaskCompleted,
                WorkflowTaskCompletedEventAttributes {
                    scheduled_event_id: self.workflow_task_scheduled_event_id,
                    ..Default::default()
                },
            );
        }

        fn add_patch(&mut self) {
            self.add(
                EventType::MarkerRecorded,
                MarkerRecordedEventAttributes {
                    marker_name: PATCH_MARKER_NAME.into(),
                    details: build_has_change_marker_details(HUB_170_SUBMIT_POLL_PATCH, false)
                        .unwrap(),
                    workflow_task_completed_event_id: self
                        .previous_workflow_task_completed_event_id,
                    ..Default::default()
                },
            );
        }

        fn complete(self) -> HistoryForReplay {
            HistoryForReplay::new(
                History {
                    events: self.events,
                },
                "hub-170-replay",
            )
        }
    }

    type HistoryEventAttributes =
        temporalio_common::protos::temporal::api::history::v1::history_event::Attributes;

    fn add_completed_activity(
        history: &mut ReplayHistoryBuilder,
        sequence: u32,
        activity_type: &str,
        output: Payload,
    ) {
        let scheduled = history.add(
            EventType::ActivityTaskScheduled,
            ActivityTaskScheduledEventAttributes {
                activity_id: sequence.to_string(),
                activity_type: Some(ActivityType {
                    name: activity_type.to_owned(),
                }),
                input: vec!["execution-170".as_json_payload().unwrap()].into_payloads(),
                start_to_close_timeout: Some(TEMPORAL_ACTIVITY_TIMEOUT.try_into().unwrap()),
                workflow_task_completed_event_id: history.previous_workflow_task_completed_event_id,
                ..Default::default()
            },
        );
        let started = history.add(
            EventType::ActivityTaskStarted,
            ActivityTaskStartedEventAttributes {
                scheduled_event_id: scheduled,
                ..Default::default()
            },
        );
        history.add(
            EventType::ActivityTaskCompleted,
            ActivityTaskCompletedEventAttributes {
                scheduled_event_id: scheduled,
                started_event_id: started,
                result: vec![output].into_payloads(),
                ..Default::default()
            },
        );
        history.add_full_workflow_task();
    }

    fn completed_replay_history(split_submit_poll: bool) -> HistoryForReplay {
        let mut history = ReplayHistoryBuilder::new();
        add_completed_activity(
            &mut history,
            1,
            GranularExecutionActivities::preflight_execution.name(),
            phase("preflighting").as_json_payload().unwrap(),
        );
        add_completed_activity(
            &mut history,
            2,
            GranularExecutionActivities::claim_authorization.name(),
            phase("claimed").as_json_payload().unwrap(),
        );
        add_completed_activity(
            &mut history,
            3,
            GranularExecutionActivities::validate_claim.name(),
            phase("executing").as_json_payload().unwrap(),
        );
        if split_submit_poll {
            history.add_patch();
            add_completed_activity(
                &mut history,
                4,
                GranularExecutionActivities::submit_provider.name(),
                ProviderPhaseOutcome::PollExisting
                    .as_json_payload()
                    .unwrap(),
            );
            add_completed_activity(
                &mut history,
                5,
                GranularExecutionActivities::poll_provider_operation.name(),
                ProviderPhaseOutcome::PersistArtifacts
                    .as_json_payload()
                    .unwrap(),
            );
        } else {
            add_completed_activity(
                &mut history,
                4,
                GranularExecutionActivities::execute_provider.name(),
                ProviderPhaseOutcome::PersistArtifacts
                    .as_json_payload()
                    .unwrap(),
            );
        }
        let next_sequence = if split_submit_poll { 6 } else { 5 };
        add_completed_activity(
            &mut history,
            next_sequence,
            GranularExecutionActivities::persist_artifacts.name(),
            phase("settling").as_json_payload().unwrap(),
        );
        add_completed_activity(
            &mut history,
            next_sequence + 1,
            GranularExecutionActivities::settle_spend.name(),
            phase("succeeded").as_json_payload().unwrap(),
        );
        history.add(
            EventType::WorkflowExecutionCompleted,
            WorkflowExecutionCompletedEventAttributes {
                workflow_task_completed_event_id: history.previous_workflow_task_completed_event_id,
                ..Default::default()
            },
        );
        history.complete()
    }

    async fn replay(history: HistoryForReplay) {
        let config = temporalio_sdk::runtime::WorkerConfig::builder()
            .namespace("default")
            .task_queue("hub-170-replay")
            .max_outstanding_activities(1_usize)
            .max_outstanding_local_activities(1_usize)
            .max_outstanding_workflow_tasks(1_usize)
            .versioning_strategy(WorkerVersioningStrategy::None {
                build_id: "hub-170-test".into(),
            })
            .task_types(WorkerTaskTypes::workflow_only())
            .skip_client_worker_set_check(true)
            .build()
            .unwrap();
        let core = temporalio_sdk::runtime::init_replay_worker(ReplayWorkerInput::new(
            config,
            stream::iter([history]),
        ))
        .unwrap();
        let mut worker = Worker::new_from_core(Arc::new(core), DataConverter::default());
        worker
            .register_workflow::<GranularExecutionWorkflow>()
            .unwrap();
        worker.run().await.unwrap();
    }

    #[test]
    fn registers_workflow_and_activity_on_execution_queue() {
        let options = worker_options(Arc::new(Runner));
        assert_eq!(options.task_queue, EXECUTION_TASK_QUEUE);
        let registered = format!("{:?}", options.activities());
        for activity in [
            GranularExecutionActivities::execute_provider.name(),
            GranularExecutionActivities::submit_provider.name(),
            GranularExecutionActivities::poll_provider_operation.name(),
        ] {
            assert!(registered.contains(activity), "missing activity {activity}");
        }
    }

    #[test]
    fn submit_poll_workflow_patch_id_is_stable() {
        assert_eq!(HUB_170_SUBMIT_POLL_PATCH, "hub-170-flux-submit-poll");
    }

    #[tokio::test]
    async fn legacy_and_split_provider_histories_both_replay_deterministically() {
        replay(completed_replay_history(false)).await;
        replay(completed_replay_history(true)).await;
    }

    #[test]
    fn workflow_input_requires_granular_configuration() {
        assert!(serde_json::from_str::<ExecutionWorkflowInput>("\"execution-1\"").is_err());
        let input: ExecutionWorkflowInput = serde_json::from_value(serde_json::json!({
            "execution_id":"execution-2", "recovery_delays_seconds":[1,2,3]
        }))
        .unwrap();
        assert_eq!(input.execution_id, "execution-2");
        assert_eq!(input.recovery_delays_seconds, vec![1, 2, 3]);
    }

    #[test]
    fn reconciliation_start_reuses_live_ids_and_restarts_closed_ids() {
        let options = execution_start_options("execution-1", EXECUTION_TASK_QUEUE);
        assert_eq!(options.workflow_id, "gongbu-execution-execution-1");
        assert_eq!(
            options.id_conflict_policy,
            WorkflowIdConflictPolicy::UseExisting
        );
        assert_eq!(
            options.id_reuse_policy,
            WorkflowIdReusePolicy::AllowDuplicate
        );
    }
}
