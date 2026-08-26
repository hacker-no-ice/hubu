//! Durable adapter-owned submission and observation loop.

use std::{sync::mpsc, time::Duration};

use crate::{
    gongbu,
    operation_registry::{ClaimedDurableOperation, GongbuContinuation},
    OperationRegistryCapability, Server,
};

const DISPATCH_RETRY_LIMIT: u32 = 5;
const OBSERVATION_RETRY_LIMIT: u32 = 5;
const RECONCILIATION_RETRY_LIMIT: u32 = 6;

pub(super) fn run(server: Server, stop: &std::sync::atomic::AtomicBool, wake: &mpsc::Receiver<()>) {
    while !stop.load(std::sync::atomic::Ordering::Acquire) {
        let delay = match server.advance_durable_operation_once() {
            Ok(true) => Duration::ZERO,
            Ok(false) | Err(()) => server.operation_tick / 4,
        };
        if delay.is_zero() {
            continue;
        }
        match wake.recv_timeout(delay) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

impl Server {
    pub(super) fn advance_durable_operation_once(&self) -> Result<bool, ()> {
        let OperationRegistryCapability::Available(registry) = self.operation_registry.as_ref()
        else {
            return Ok(false);
        };
        let operation = {
            let mut registry = registry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            registry.promote_accepted_operations().map_err(|_| ())?;
            registry.claim_due_operation().map_err(|_| ())?
        };
        let Some(operation) = operation else {
            return Ok(false);
        };
        if operation.deadline_expired {
            self.fail_operation(&operation, "operation_deadline_exhausted")?;
            return Ok(true);
        }
        let Some(client) = self.backends.gongbu.as_ref() else {
            self.fail_operation(&operation, "gongbu_unconfigured")?;
            return Ok(true);
        };
        let expected = GongbuContinuation {
            operation_key: operation.operation_key.clone(),
            operation_handle: operation.operation_handle.clone(),
            execution_id: operation.execution_id.clone(),
        };
        let result = if let Some(execution_id) = operation.execution_id.as_deref() {
            gongbu::observe_durable_execution(client, execution_id, &expected)
        } else {
            let Some(request) = operation.request.clone() else {
                self.fail_operation(&operation, "execution_request_unavailable")?;
                return Ok(true);
            };
            gongbu::create_durable_execution(client, request, &expected)
        };
        match result {
            Ok(lifecycle) if lifecycle.status == "reconciliation_required" => {
                let attempts = operation.reconciliation_attempts.saturating_add(1);
                if attempts >= RECONCILIATION_RETRY_LIMIT {
                    self.with_registry(|registry| {
                        registry.fail_durable_lifecycle(
                            &operation,
                            &lifecycle,
                            "reconciliation_exhausted",
                        )
                    })?;
                } else {
                    self.with_registry(|registry| {
                        registry.record_durable_lifecycle(
                            &operation,
                            &lifecycle,
                            exponential_delay(self.operation_tick.saturating_mul(30), attempts - 1),
                            true,
                        )
                    })?;
                }
            }
            Ok(lifecycle) => {
                self.with_registry(|registry| {
                    registry.record_durable_lifecycle(
                        &operation,
                        &lifecycle,
                        self.operation_tick,
                        false,
                    )
                })?;
            }
            Err(error) => self.record_call_failure(&operation, error)?,
        }
        Ok(true)
    }

    fn record_call_failure(
        &self,
        operation: &ClaimedDurableOperation,
        error: gongbu::DurableCallError,
    ) -> Result<(), ()> {
        if !error.retryable {
            let diagnostic = if operation.execution_id.is_none() {
                error.admission_diagnostic
            } else {
                None
            };
            return self.fail_operation(operation, durable_failure_code(error.code, diagnostic));
        }
        let (attempts, limit, base, pending_code, exhausted_code) =
            if operation.execution_id.is_none() {
                (
                    operation.dispatch_attempts.saturating_add(1),
                    DISPATCH_RETRY_LIMIT,
                    self.operation_tick,
                    "dispatch_retry_pending",
                    "dispatch_retry_exhausted",
                )
            } else {
                (
                    operation.observation_failures.saturating_add(1),
                    OBSERVATION_RETRY_LIMIT,
                    self.operation_tick,
                    "observation_retry_pending",
                    "observation_retry_exhausted",
                )
            };
        if attempts >= limit {
            self.fail_operation(operation, exhausted_code)
        } else {
            self.with_registry(|registry| {
                registry.retry_durable_operation(
                    operation,
                    exponential_delay(base, attempts - 1),
                    pending_code,
                )
            })
        }
    }

    fn fail_operation(
        &self,
        operation: &ClaimedDurableOperation,
        result_code: &str,
    ) -> Result<(), ()> {
        self.with_registry(|registry| registry.fail_durable_operation(operation, result_code))
    }

    fn with_registry(
        &self,
        operation: impl FnOnce(&mut crate::operation_registry::OperationRegistry) -> anyhow::Result<()>,
    ) -> Result<(), ()> {
        let OperationRegistryCapability::Available(registry) = self.operation_registry.as_ref()
        else {
            return Err(());
        };
        operation(
            &mut registry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
        .map_err(|_| ())
    }
}

fn durable_failure_code(
    code: &str,
    diagnostic: Option<gongbu::AdmissionDiagnostic>,
) -> &'static str {
    match (code, diagnostic) {
        ("invalid_request", Some(diagnostic)) => diagnostic.durable_result_code(),
        ("invalid_request", None) => "execution_request_invalid",
        ("unauthorized", _) => "gongbu_authentication_failed",
        ("forbidden", _) => "gongbu_access_forbidden",
        ("not_found", _) => "execution_not_found",
        ("immutable_scope_conflict", _) => "execution_intent_conflict",
        _ => "execution_dispatch_failed",
    }
}

fn exponential_delay(base: Duration, exponent: u32) -> Duration {
    base.saturating_mul(1_u32 << exponent.min(31))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_delay_is_exponential_and_saturating() {
        assert_eq!(
            exponential_delay(Duration::from_secs(1), 0),
            Duration::from_secs(1)
        );
        assert_eq!(
            exponential_delay(Duration::from_secs(1), 4),
            Duration::from_secs(16)
        );
        assert_eq!(exponential_delay(Duration::MAX, 31), Duration::MAX);
    }

    #[test]
    fn permanent_backend_errors_map_to_safe_codes() {
        assert_eq!(
            durable_failure_code("unauthorized", None),
            "gongbu_authentication_failed"
        );
        assert_eq!(
            durable_failure_code("unexpected-secret", None),
            "execution_dispatch_failed"
        );
    }

    #[test]
    fn admission_diagnostics_map_only_invalid_dispatch_requests() {
        for diagnostic in [
            gongbu::AdmissionDiagnostic::TargetNotSelectable,
            gongbu::AdmissionDiagnostic::PricingSelectorNotMatched,
        ] {
            assert_eq!(
                durable_failure_code("invalid_request", Some(diagnostic)),
                diagnostic.durable_result_code()
            );
            assert_eq!(
                durable_failure_code("unauthorized", Some(diagnostic)),
                "gongbu_authentication_failed"
            );
        }
        assert_eq!(
            durable_failure_code("invalid_request", None),
            "execution_request_invalid"
        );
    }
}
