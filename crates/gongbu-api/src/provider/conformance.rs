use super::contract::{AdapterOutcome, ProviderFailure, ProviderPhase, SpendDisposition};

#[derive(Clone, Copy, Debug)]
pub(super) enum Case {
    Rejection,
    AmbiguousPostSend,
    EvidenceRetention,
    HostPolicy,
    ArtifactBound,
    UnsafeRetry,
    InvalidRequest,
}

pub(super) struct Observation {
    pub result: Result<AdapterOutcome, ProviderFailure>,
    pub submissions: u32,
}

/// Reusable behavioral contract run by every URL-producing image adapter.
pub(super) fn assert_adapter_conformance(mut run: impl FnMut(Case) -> Observation) {
    for (case, expected_code, expected_submissions, disposition, evidence_required) in [
        (
            Case::Rejection,
            "provider_rejected",
            1,
            SpendDisposition::Release,
            false,
        ),
        (
            Case::AmbiguousPostSend,
            "timeout_unknown_outcome",
            1,
            SpendDisposition::Reconcile,
            false,
        ),
        (
            Case::EvidenceRetention,
            "artifact_policy_failure",
            1,
            SpendDisposition::Reconcile,
            true,
        ),
        (
            Case::HostPolicy,
            "artifact_policy_failure",
            1,
            SpendDisposition::Reconcile,
            true,
        ),
        (
            Case::ArtifactBound,
            "artifact_policy_failure",
            1,
            SpendDisposition::Reconcile,
            true,
        ),
        (
            Case::UnsafeRetry,
            "unsafe_retry",
            0,
            SpendDisposition::Release,
            false,
        ),
        (
            Case::InvalidRequest,
            "invalid_request",
            0,
            SpendDisposition::Release,
            false,
        ),
    ] {
        let observation = run(case);
        assert_eq!(observation.submissions, expected_submissions, "{case:?}");
        let error = observation.result.expect_err("case must be rejected");
        if matches!(case, Case::UnsafeRetry) {
            assert!(
                matches!(
                    error.code.as_str(),
                    "retry_not_supported" | "idempotency_policy_mismatch"
                ),
                "{case:?}: {}",
                error.code
            );
        } else {
            assert_eq!(error.code, expected_code, "{case:?}");
        }
        assert_eq!(error.spend_disposition, disposition, "{case:?}");
        assert!(
            matches!(
                error.phase,
                ProviderPhase::PreSend
                    | ProviderPhase::Submission
                    | ProviderPhase::Processing
                    | ProviderPhase::Artifact
            ),
            "{case:?}"
        );
        if evidence_required {
            assert!(
                error.evidence.request_id.is_some() || error.evidence.operation_id.is_some(),
                "{case:?}"
            );
        }
        assert!(!error.to_string().contains("secret-canary"), "{case:?}");
    }
}
