//! Admin HTTP routes: /metrics, /healthz, /readyz, /admin/*.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::IntoResponse,
    routing::get,
};
use prometheus::{Encoder, TextEncoder};
use serde::Serialize;

use crate::health::{AggregateStatus, AggregatedHealth, HealthRegistry};
use crate::probe::traits::ClientSummary;

#[derive(Clone, Default)]
pub struct AdminConfig {
    /// When `None`, all write endpoints reject with 401 (fail-closed).
    pub token: Option<String>,
}

#[derive(Clone)]
pub struct AdminState {
    pub health: Arc<HealthRegistry>,
    pub admin: AdminConfig,
    /// Optional registry snapshot for the /admin/clients endpoint.
    /// `None` returns an empty list (used in tests).
    pub registry: Option<Arc<dyn crate::probe::traits::RegistrySnapshot>>,
    /// Optional client-cancel hook for /admin/clients/:id/disconnect.
    /// `None` causes the endpoint to return 503.
    pub client_cancel: Option<Arc<dyn crate::probe::traits::ClientCancel>>,
}

/// Canonical builder. Every later route addition (Task 10 disconnect) extends
/// this function in place.
pub fn build_router_with_state(state: AdminState) -> Router {
    let public = Router::new()
        .route("/metrics", get(metrics_handler))
        .route("/healthz", get(healthz_handler))
        .route("/readyz", get(readyz_handler));

    let protected = Router::new()
        .route("/admin/clients", get(list_clients_handler))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_admin_token,
        ));

    public.merge(protected).with_state(state)
}

/// Convenience wrapper used by tests that don't have a registry / cancel hook.
pub fn build_router(health: Arc<HealthRegistry>, admin: AdminConfig) -> Router {
    build_router_with_state(AdminState {
        health,
        admin,
        registry: None,
        client_cancel: None,
    })
}

async fn require_admin_token(
    State(state): State<AdminState>,
    headers: HeaderMap,
    req: axum::extract::Request,
    next: Next,
) -> Result<impl IntoResponse, (StatusCode, &'static str)> {
    let expected = match state.admin.token.as_deref() {
        Some(t) => t,
        None => return Err((StatusCode::UNAUTHORIZED, "unauthorized")),
    };
    let provided = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "));
    match provided {
        Some(t) if t == expected => Ok(next.run(req).await),
        _ => Err((StatusCode::UNAUTHORIZED, "unauthorized")),
    }
}

async fn metrics_handler() -> impl IntoResponse {
    let encoder = TextEncoder::new();
    let metric_families = crate::metrics::global_registry().gather();
    let mut buf = Vec::new();
    if encoder.encode(&metric_families, &mut buf).is_err() {
        return (StatusCode::INTERNAL_SERVER_ERROR, "encode failed").into_response();
    }
    (
        StatusCode::OK,
        [("content-type", "text/plain; version=0.0.4")],
        buf,
    )
        .into_response()
}

async fn healthz_handler() -> impl IntoResponse {
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

#[derive(Serialize)]
struct ClientSummaryView {
    id: String,
    transport: String,
    titles: Vec<String>,
    connected_secs: u64,
}

// Placeholder implementation; replaced in Task 8 once ClientRegistry
// exposes the real snapshot. For now returns empty list.
async fn list_clients_handler(State(state): State<AdminState>) -> impl IntoResponse {
    let snaps = match state.registry.as_ref() {
        Some(r) => r.list_clients(),
        None => vec![],
    };
    let body: Vec<ClientSummaryView> = snaps
        .into_iter()
        .map(|c: ClientSummary| ClientSummaryView {
            id: format!("{:032X}", c.id),
            transport: c.transport,
            titles: c.titles,
            connected_secs: c.connected_secs,
        })
        .collect();
    (StatusCode::OK, Json(body))
}
