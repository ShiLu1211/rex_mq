//! Admin HTTP routes: /metrics, /healthz, /readyz, /admin/*.

use std::sync::Arc;

use axum::{Json, Router, extract::State, http::StatusCode, response::IntoResponse, routing::get};
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
/// constructor; later tasks add `/admin/*` routes here.
pub fn build_router_with_state(state: AdminState) -> Router {
    Router::new()
        .route("/metrics", get(metrics_handler))
        .route("/healthz", get(healthz_handler))
        .route("/readyz", get(readyz_handler))
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

#[derive(Serialize)]
struct ReadyResponse {
    status: &'static str,
    probes: Vec<ProbeEntry>,
}

#[derive(Serialize)]
struct ProbeEntry {
    name: &'static str,
    status: &'static str,
    reason: Option<String>,
}

async fn readyz_handler(State(state): State<AdminState>) -> impl IntoResponse {
    let agg: AggregatedHealth = state.health.check_all();
    let body = ReadyResponse {
        status: match agg.status {
            AggregateStatus::Healthy => "healthy",
            AggregateStatus::Degraded => "degraded",
            AggregateStatus::Unhealthy => "unhealthy",
        },
        probes: agg
            .results
            .into_iter()
            .map(|(name, r)| ProbeEntry {
                name,
                status: match r {
                    crate::health::ProbeResult::Healthy => "healthy",
                    crate::health::ProbeResult::Degraded { .. } => "degraded",
                    crate::health::ProbeResult::Unhealthy { .. } => "unhealthy",
                },
                reason: match r {
                    crate::health::ProbeResult::Healthy => None,
                    crate::health::ProbeResult::Degraded { reason }
                    | crate::health::ProbeResult::Unhealthy { reason } => Some(reason),
                },
            })
            .collect(),
    };
    let status = match agg.status {
        AggregateStatus::Healthy | AggregateStatus::Degraded => StatusCode::OK,
        AggregateStatus::Unhealthy => StatusCode::SERVICE_UNAVAILABLE,
    };
    (status, Json(body))
}
