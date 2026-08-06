//! Temporal registration for the durable execution workflow.
//!
//! Temporal redelivers one durable activity per workflow. The activity runs
//! the persisted state machine in [`crate::workflow`], whose durable boundaries
//! make duplicate workflow/activity delivery side-effect safe.

use crate::{
    execution::Repository,
    workflow::{ArtifactActivities, ExecutionWorkflow, HubuActivities, ProviderActivities},
};
use futures::{channel::oneshot, future::poll_fn, Future};
use std::{sync::Arc, time::Duration};
use temporalio_client::{Client, WorkflowStartOptions};
use temporalio_common_wasm::protos::temporal::api::enums::v1::WorkflowIdConflictPolicy;
use temporalio_macros::{activities, workflow, workflow_methods};
use temporalio_sdk::{
    activities::{ActivityContext, ActivityError},
    ActivityOptions, ApplicationFailure, Runtime, Worker, WorkerOptions, WorkflowContext,
    WorkflowResult,
};

pub const EXECUTION_TASK_QUEUE: &str = "gongbu-executions";

pub trait DurableExecutionRunner: Send + Sync + 'static {
    fn run_execution(&self, execution_id: &str) -> Result<String, String>;
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
}

#[workflow]
#[derive(Default)]
pub struct DurableExecutionWorkflow;

#[workflow_methods]
impl DurableExecutionWorkflow {
    #[run]
    pub async fn run(
        ctx: &mut WorkflowContext<Self>,
        execution_id: String,
    ) -> WorkflowResult<String> {
        let options = ActivityOptions::start_to_close_timeout(Duration::from_secs(300));
        let status = ctx
            .execute_activity(ExecutionActivities::run_execution, execution_id, options)
            .await?;
        Ok(status)
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
type ScheduleRequest = (String, std::sync::mpsc::SyncSender<ScheduleResult>);

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
            while let Ok((execution_id, response)) = request_rx.recv() {
                let result =
                    runtime.block_on(start_execution(scheduler_client.clone(), execution_id));
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
            .send((execution_id.to_owned(), response_tx))
            .map_err(|_| "Temporal scheduler stopped".to_owned())?;
        response_rx
            .recv()
            .map_err(|_| "Temporal scheduler stopped".to_owned())?
    }
}

async fn start_execution(client: Client, execution_id: String) -> ScheduleResult {
    client
        .start_workflow(
            DurableExecutionWorkflow::run,
            execution_id.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;

    struct Runner;
    impl DurableExecutionRunner for Runner {
        fn run_execution(&self, _: &str) -> Result<String, String> {
            Ok("succeeded".into())
        }
    }

    #[test]
    fn registers_workflow_and_activity_on_execution_queue() {
        let options = worker_options(Arc::new(Runner));
        assert_eq!(options.task_queue, EXECUTION_TASK_QUEUE);
    }
}
