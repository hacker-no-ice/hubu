//! Temporal registration for the durable execution workflow.
//!
//! Temporal delivers one non-retrying activity per workflow. The activity runs
//! the persisted state machine in [`crate::workflow`], whose durable boundaries
//! make duplicate workflow/activity delivery side-effect safe.

use std::{sync::Arc, time::Duration};
use temporalio_common_wasm::RetryPolicy;
use temporalio_macros::{activities, workflow, workflow_methods};
use temporalio_sdk::{
    activities::{ActivityContext, ActivityError},
    ActivityOptions, ApplicationFailure, WorkerOptions, WorkflowContext, WorkflowResult,
};

pub const EXECUTION_TASK_QUEUE: &str = "gongbu-executions";

pub trait DurableExecutionRunner: Send + Sync + 'static {
    fn run_execution(&self, execution_id: &str) -> Result<String, String>;
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
        let options = ActivityOptions::with_start_to_close_timeout(Duration::from_secs(300))
            .retry_policy(RetryPolicy::builder().maximum_attempts(1).build())
            .build();
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
        self.runner.run_execution(&execution_id).map_err(|message| {
            ActivityError::application(ApplicationFailure::non_retryable(message))
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
