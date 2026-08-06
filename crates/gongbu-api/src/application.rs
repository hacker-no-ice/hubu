//! Runnable HTTP and Temporal process composition.
//!
//! Provider-specific activity implementations are supplied by the executable
//! that owns them. This module owns the process-level invariant that the HTTP
//! API is never served without a live Temporal worker and that both share the
//! same repository and scheduler.

use crate::{
    artifact::ArtifactService,
    execution::Repository,
    http::{Api, AuthenticatedAccount, HttpResponse},
    provider::{contract::PricingCatalog, targets::ProviderTargetConfig},
    temporal::{start_worker, DurableExecutionRunner, StartedTemporalWorker},
};
use axum::{
    body::{to_bytes, Body},
    extract::State,
    http::{header, HeaderMap, Request, StatusCode},
    response::{IntoResponse, Response},
    Router,
};
use futures::future::{select, Either};
use std::{future::Future, net::SocketAddr, sync::Arc};
use temporalio_client::Client;
use temporalio_sdk::Runtime;
use thiserror::Error;

const MAX_REQUEST_BYTES: usize = 1024 * 1024;

pub trait Authenticator: Send + Sync + 'static {
    /// Validate transport credentials and return the trusted account principal.
    fn authenticate(
        &self,
        headers: &HeaderMap,
    ) -> Result<AuthenticatedAccount, AuthenticationError>;
}

#[derive(Clone, Copy, Debug, Error)]
#[error("request authentication failed")]
pub struct AuthenticationError;

pub struct ApplicationDependencies {
    pub repository: Repository,
    pub artifacts: ArtifactService,
    pub targets: ProviderTargetConfig,
    pub pricing: PricingCatalog,
    pub temporal_runtime: Arc<Runtime>,
    pub temporal_client: Client,
    pub execution_runner: Arc<dyn DurableExecutionRunner>,
    pub authenticator: Arc<dyn Authenticator>,
    pub now: Arc<dyn Fn() -> String + Send + Sync>,
}

#[derive(Clone)]
struct ApplicationState {
    api: Api,
    authenticator: Arc<dyn Authenticator>,
}

/// Bind the v1 HTTP surface and poll the Temporal execution queue until shutdown.
///
/// Startup is ordered deliberately: the worker must successfully initialize
/// before the listener begins accepting execution requests.
pub async fn serve<F>(
    listener: tokio::net::TcpListener,
    dependencies: ApplicationDependencies,
    shutdown: F,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    F: Future<Output = ()> + Send + 'static,
{
    let mut worker = start_worker(
        dependencies.temporal_runtime,
        dependencies.temporal_client,
        dependencies.execution_runner,
    )?;
    let api = Api::new(
        dependencies.repository,
        dependencies.artifacts,
        dependencies.targets,
        dependencies.pricing,
        worker.scheduler.clone(),
        move || (dependencies.now)(),
    );
    let state = ApplicationState {
        api,
        authenticator: dependencies.authenticator,
    };
    let completion = worker.take_completion();
    let supervised_shutdown = wait_for_shutdown(shutdown, completion);
    let result = axum::serve(listener, Router::new().fallback(dispatch).with_state(state))
        .with_graceful_shutdown(supervised_shutdown)
        .await;
    stop_worker(worker)?;
    result.map_err(Into::into)
}

async fn wait_for_shutdown<F>(shutdown: F, completion: futures::channel::oneshot::Receiver<()>)
where
    F: Future<Output = ()>,
{
    match select(Box::pin(shutdown), Box::pin(completion)).await {
        Either::Left(_) | Either::Right(_) => {}
    }
}

fn stop_worker(
    worker: StartedTemporalWorker,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    worker.shutdown();
    worker.join()
}

async fn dispatch(State(state): State<ApplicationState>, request: Request<Body>) -> Response {
    let account = state.authenticator.authenticate(request.headers()).ok();
    let method = request.method().as_str().to_owned();
    let path = request.uri().path().to_owned();
    let body = match to_bytes(request.into_body(), MAX_REQUEST_BYTES).await {
        Ok(body) => body,
        Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    };
    into_axum(state.api.handle(&method, &path, account.as_ref(), &body))
}

fn into_axum(response: HttpResponse) -> Response {
    let status = StatusCode::from_u16(response.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (
        status,
        [(header::CONTENT_TYPE, response.content_type)],
        response.body,
    )
        .into_response()
}

pub fn listener_address(listener: &tokio::net::TcpListener) -> std::io::Result<SocketAddr> {
    listener.local_addr()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_domain_http_response_at_transport_boundary() {
        let response = into_axum(HttpResponse {
            status: 409,
            content_type: "application/json".into(),
            body: br#"{"error":"conflict"}"#.to_vec(),
        });
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/json"
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let body = runtime
            .block_on(to_bytes(response.into_body(), MAX_REQUEST_BYTES))
            .unwrap();
        assert_eq!(body, br#"{"error":"conflict"}"#.as_slice());
    }

    #[test]
    fn worker_completion_triggers_http_shutdown() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (completion_tx, completion_rx) = futures::channel::oneshot::channel();
        completion_tx.send(()).unwrap();
        runtime.block_on(wait_for_shutdown(futures::future::pending(), completion_rx));
    }
}
