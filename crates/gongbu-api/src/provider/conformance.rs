use super::contract::{AdapterOutcome, ContractError};
use reqwest::Url;
use std::io::{Cursor, Read, Write};

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
    pub result: Result<AdapterOutcome, ContractError>,
    pub submissions: u32,
}

/// Reusable behavioral contract run by every URL-producing image adapter.
pub(super) fn assert_adapter_conformance(mut run: impl FnMut(Case) -> Observation) {
    for (case, expected_code, expected_submissions, evidence_required) in [
        (Case::Rejection, "provider_rejected", 1, false),
        (Case::AmbiguousPostSend, "timeout_unknown_outcome", 1, false),
        (Case::EvidenceRetention, "artifact_policy_failure", 1, true),
        (Case::HostPolicy, "artifact_policy_failure", 1, true),
        (Case::ArtifactBound, "artifact_policy_failure", 1, true),
        (Case::UnsafeRetry, "unsafe_retry", 0, false),
        (Case::InvalidRequest, "invalid_request", 0, false),
    ] {
        let observation = run(case);
        assert_eq!(observation.submissions, expected_submissions, "{case:?}");
        let error = observation.result.expect_err("case must be rejected");
        if matches!(case, Case::UnsafeRetry) {
            assert!(
                matches!(&error, ContractError::UnsafeRetry)
                    || matches!(
                        &error,
                        ContractError::Provider { code }
                            if matches!(code.as_str(), "config_invalid" | "retry_not_supported" | "idempotency_policy_mismatch")
                    ),
                "unexpected unsafe retry error: {error}"
            );
            assert!(!error.to_string().contains("secret-canary"));
            continue;
        }
        let (code, request_id, operation_id) = match &error {
            ContractError::Provider { code } => (code.as_str(), None, None),
            ContractError::ProviderWithEvidence {
                code,
                request_id,
                operation_id,
            } => (
                code.as_str(),
                request_id.as_deref(),
                operation_id.as_deref(),
            ),
            other => panic!("unexpected {case:?} error: {other}"),
        };
        assert_eq!(code, expected_code, "{case:?}");
        if evidence_required {
            assert!(request_id.is_some() || operation_id.is_some(), "{case:?}");
        }
        assert!(!error.to_string().contains("secret-canary"), "{case:?}");
    }
}

pub(super) fn assert_body_and_artifact_bounds(
    mut provider_body_rejected: impl FnMut(&mut Cursor<Vec<u8>>, usize) -> bool,
    mut artifact_body_rejected: impl FnMut(&mut Cursor<Vec<u8>>, usize) -> bool,
) {
    assert!(!provider_body_rejected(&mut Cursor::new(vec![0; 8]), 8));
    assert!(provider_body_rejected(&mut Cursor::new(vec![0; 9]), 8));
    assert!(!artifact_body_rejected(&mut Cursor::new(vec![0; 8]), 8));
    assert!(artifact_body_rejected(&mut Cursor::new(vec![0; 9]), 8));
}

pub(super) fn assert_redirect_blocked(fetch_rejected: impl FnOnce(&Url) -> bool) {
    use std::net::TcpListener;

    let destination = TcpListener::bind("127.0.0.1:0").unwrap();
    destination.set_nonblocking(true).unwrap();
    let destination_url = format!("http://{}/secret", destination.local_addr().unwrap());
    let redirector = TcpListener::bind("127.0.0.1:0").unwrap();
    let redirect_url = Url::parse(&format!(
        "http://{}/artifact",
        redirector.local_addr().unwrap()
    ))
    .unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = redirector.accept().unwrap();
        let mut request = [0; 1024];
        let _ = stream.read(&mut request).unwrap();
        write!(
            stream,
            "HTTP/1.1 302 Found\r\nLocation: {destination_url}\r\nContent-Length: 0\r\n\r\n"
        )
        .unwrap();
    });
    assert!(fetch_rejected(&redirect_url));
    server.join().unwrap();
    assert!(
        matches!(destination.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
    );
}
