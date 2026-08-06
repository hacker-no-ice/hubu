//! Temporal registration for the durable execution workflow.
//!
//! Temporal redelivers one durable activity per workflow. The activity runs
//! the persisted state machine in [`crate::workflow`], whose durable boundaries
//! make duplicate workflow/activity delivery side-effect safe.

use crate::{
    execution::Repository,
    workflow::{
        ArtifactActivities, ExecutionWorkflow, HubuActivities, OperatorReconciliationRequest,
        ProviderActivities,
    },
};
use futures::{channel::oneshot, future::poll_fn, Future};
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};
use temporalio_client::{Client, WorkflowSignalOptions, WorkflowStartOptions};
use temporalio_common_wasm::protos::temporal::api::enums::v1::WorkflowIdConflictPolicy;
use temporalio_macros::{activities, workflow, workflow_methods};
use temporalio_sdk::{
    activities::{ActivityContext, ActivityError},
    ActivityOptions, ApplicationFailure, Runtime, SyncWorkflowContext, Worker, WorkerOptions,
    WorkflowContext, WorkflowContextView, WorkflowResult,
};

pub const EXECUTION_TASK_QUEUE: &str = "gongbu-executions";

pub trait DurableExecutionRunner: Send + Sync + 'static {
    fn run_execution(&self, execution_id: &str) -> Result<String, String>;
    fn recover_execution(
        &self,
        execution_id: &str,
        operator: Option<&OperatorReconciliationRequest>,
        exhausted: bool,
    ) -> Result<String, String>;
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
}

impl DurableExecutionRunner for PersistedExecutionRunner {
    fn run_execution(&self, execution_id: &str) -> Result<String, String> {
        ExecutionWorkflow {
            repository: &self.repository,
            hubu: self.hubu.as_ref(),
            provider: self.provider.as_ref(),
            artifacts: self.artifacts.as_ref(),
        }
        .run(execution_id, &(self.now)())
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
        ExecutionWorkflow {
            repository: &self.repository,
            hubu: self.hubu.as_ref(),
            provider: self.provider.as_ref(),
            artifacts: self.artifacts.as_ref(),
        }
        .recover(execution_id, &(self.now)(), operator)
        .map(|e| e.status)
        .map_err(|e| e.to_string())
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
pub struct DurableExecutionWorkflow {
    pending: Vec<OperatorReconciliationRequest>,
}

#[workflow_methods]
impl DurableExecutionWorkflow {
    #[run]
    pub async fn run(
        ctx: &mut WorkflowContext<Self>,
        input: ExecutionWorkflowInput,
    ) -> WorkflowResult<String> {
        let options = ActivityOptions::start_to_close_timeout(Duration::from_secs(300));
        let mut status = ctx
            .execute_activity(
                ExecutionActivities::run_execution,
                input.execution_id.clone(),
                options.clone(),
            )
            .await?;
        if status != "reconciliation_required" {
            return Ok(status);
        }
        let delay_count = input.recovery_delays_seconds.len();
        for (index, delay) in input.recovery_delays_seconds.into_iter().enumerate() {
            temporalio_sdk::workflows::select! {
                _ = ctx.timer(Duration::from_secs(delay)) => {}
                _ = ctx.wait_condition(|s: &Self| !s.pending.is_empty()) => {}
            }
            let operator = ctx.state_mut(|s| s.pending.pop());
            status = ctx
                .execute_activity(
                    ExecutionActivities::recover_execution,
                    RecoveryActivityInput {
                        execution_id: input.execution_id.clone(),
                        exhausted: operator.is_none() && index + 1 == delay_count,
                        operator,
                    },
                    options.clone(),
                )
                .await?;
            if status != "reconciliation_required" {
                return Ok(status);
            }
        }
        loop {
            ctx.wait_condition(|s: &Self| !s.pending.is_empty()).await;
            let operator = ctx.state_mut(|s| s.pending.pop());
            status = ctx
                .execute_activity(
                    ExecutionActivities::recover_execution,
                    RecoveryActivityInput {
                        execution_id: input.execution_id.clone(),
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
pub struct ExecutionActivities {
    runner: Arc<dyn DurableExecutionRunner>,
}

impl ExecutionActivities {
    pub fn new(runner: Arc<dyn DurableExecutionRunner>) -> Self {
        Self { runner }
    }
}

#[activities]
impl ExecutionActivities {
    #[activity]
    pub async fn recover_execution(
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
        .map_err(|e| {
            ActivityError::application(ApplicationFailure::new(std::io::Error::other(
                e.to_string(),
            )))
        })?
        .map_err(|m| ActivityError::application(ApplicationFailure::new(std::io::Error::other(m))))
    }
    #[activity]
    pub async fn run_execution(
        self: Arc<Self>,
        _ctx: ActivityContext,
        execution_id: String,
    ) -> Result<String, ActivityError> {
        let runner = Arc::clone(&self.runner);
        tokio::task::spawn_blocking(move || runner.run_execution(&execution_id))
            .await
            .map_err(|error| {
                ActivityError::application(ApplicationFailure::new(std::io::Error::other(
                    error.to_string(),
                )))
            })?
            .map_err(|message| {
                ActivityError::application(ApplicationFailure::new(std::io::Error::other(message)))
            })
    }
}

pub fn worker_options(runner: Arc<dyn DurableExecutionRunner>) -> WorkerOptions {
    WorkerOptions::new(EXECUTION_TASK_QUEUE)
        .register_workflow::<DurableExecutionWorkflow>()
        .expect("durable execution workflow registration is valid")
        .register_activities(ExecutionActivities::new(runner))
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

pub trait ExecutionScheduler: Send + Sync + 'static {
    fn schedule(&self, execution_id: &str) -> Result<(), String>;
    fn reconcile(
        &self,
        execution_id: &str,
        request: OperatorReconciliationRequest,
    ) -> Result<(), String>;
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

type ScheduleResult = Result<(), String>;
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
    let scheduler_client = client.clone();
    let (startup_tx, startup_rx) = std::sync::mpsc::sync_channel(1);
    let (completion_tx, completion_rx) = oneshot::channel();
    let thread = std::thread::Builder::new()
        .name("gongbu-temporal-worker".into())
        .spawn(move || {
            let tokio_runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            let mut worker = Worker::new(&runtime, client, worker_options(runner))
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            let shutdown: Arc<dyn Fn() + Send + Sync> = Arc::new(worker.shutdown_handle());
            let run_result = tokio_runtime.block_on(async move {
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
                            start_execution(scheduler_client.clone(), execution_id).await
                        }
                        SchedulerCommand::Reconcile(execution_id, request) => {
                            signal_reconciliation(scheduler_client.clone(), execution_id, request)
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
    fn schedule(&self, execution_id: &str) -> Result<(), String> {
        let (response_tx, response_rx) = std::sync::mpsc::sync_channel(1);
        self.requests
            .send((
                SchedulerCommand::Start(execution_id.to_owned()),
                response_tx,
            ))
            .map_err(|_| "Temporal scheduler stopped".to_owned())?;
        response_rx
            .recv()
            .map_err(|_| "Temporal scheduler stopped".to_owned())?
    }
    fn reconcile(
        &self,
        execution_id: &str,
        request: OperatorReconciliationRequest,
    ) -> Result<(), String> {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        self.requests
            .send((
                SchedulerCommand::Reconcile(execution_id.to_owned(), request),
                tx,
            ))
            .map_err(|_| "Temporal scheduler stopped".to_owned())?;
        rx.recv()
            .map_err(|_| "Temporal scheduler stopped".to_owned())?
    }
}

fn recovery_delays_seconds() -> Vec<u64> {
    std::env::var("GONGBU_RECONCILIATION_DELAYS_SECONDS")
        .ok()
        .and_then(|v| {
            let parsed: Option<Vec<u64>> = v
                .split(',')
                .map(|p| p.trim().parse::<u64>().ok().filter(|n| *n > 0))
                .collect();
            parsed.filter(|v| !v.is_empty() && v.len() <= 8)
        })
        .unwrap_or_else(|| vec![30, 120, 600])
}

async fn start_execution(client: Client, execution_id: String) -> ScheduleResult {
    client
        .start_workflow(
            DurableExecutionWorkflow::run,
            ExecutionWorkflowInput {
                execution_id: execution_id.clone(),
                recovery_delays_seconds: recovery_delays_seconds(),
            },
            WorkflowStartOptions::new(
                EXECUTION_TASK_QUEUE,
                format!("gongbu-execution-{execution_id}"),
            )
            .id_conflict_policy(WorkflowIdConflictPolicy::UseExisting)
            .build(),
        )
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
}

async fn signal_reconciliation(
    client: Client,
    execution_id: String,
    request: OperatorReconciliationRequest,
) -> ScheduleResult {
    client
        .get_workflow_handle::<DurableExecutionWorkflow>(format!("gongbu-execution-{execution_id}"))
        .signal(
            DurableExecutionWorkflow::reconcile,
            request,
            WorkflowSignalOptions::default(),
        )
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Runner;
    impl DurableExecutionRunner for Runner {
        fn run_execution(&self, _: &str) -> Result<String, String> {
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
    }

    #[test]
    fn registers_workflow_and_activity_on_execution_queue() {
        let options = worker_options(Arc::new(Runner));
        assert_eq!(options.task_queue, EXECUTION_TASK_QUEUE);
    }
}
