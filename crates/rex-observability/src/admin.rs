//! Admin HTTP routes: /metrics, /healthz, /readyz, /admin/*.

use std::sync::Arc;

use axum::{Router, http::StatusCode, response::IntoResponse, routing::get};
use prometheus::{Encoder, TextEncoder};
use serde::Serialize;

use crate::health::{AggregateStatus, AggregatedHealth, HealthRegistry};

#[derive(Clone, Default)]
pub struct AdminConfig {
    /// When `None`, all write endpoints reject with 401 (fail-closed).
    pub token: Option<String>,
}

#[derive(Clone)]
pub struct AdminState {
    pub health: Arc<HealthRegistry>,
    pub admin: AdminConfig,
}

/// Build a router from a fully-configured `AdminState`. This is the canonical
/// constructor; later tasks add `/readyz` and `/admin/*` routes here.
pub fn build_router_with_state(state: AdminState) -> Router {
    Router::new()
        .route("/metrics", get(metrics_handler))
        .route("/healthz", get(healthz_handler))
        .with_state(state)
}

async fn metrics_handler() -> impl IntoResponse {
    let encoder = TextEncoder::new();
    let metric_families = crate::metrics::global_registry().gather();
    let mut buf = Vec::new();
    match encoder.encode(&metric_families, &mut buf) {
        Ok(()) => (
            StatusCode::OK,
            [("content-type", "text/plain; version=0.0.4")],
            buf,
        )
            .into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "encode failed").into_response(),
    }
}

async fn healthz_handler() -> impl IntoResponse {
    // Liveness: process is alive and the HTTP server is accepting.
    (StatusCode::OK, "ok")
}

// Stub types so the rest of the file compiles; replaced in Task 6.
#[derive(Serialize)]
struct ReadyResponse {
    _dummy: (),
}
#[allow(dead_code)]
async fn _stub() -> impl IntoResponse {
    (StatusCode::OK, axum::Json(ReadyResponse { _dummy: () }))
}

// `AggregatedHealth` and `AggregateStatus` are imported for Task 6's
// `/readyz` route; silence the unused-import warning until then.
#[allow(dead_code)]
fn _unused_health_imports(_: &AggregatedHealth, _: AggregateStatus) {}
