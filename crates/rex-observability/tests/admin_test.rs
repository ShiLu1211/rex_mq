//! Integration tests for the admin HTTP server.

use std::{net::SocketAddr, sync::Arc};

use rex_observability::{
    admin::{AdminConfig, AdminState, build_router_with_state},
    health::{HealthProbe, HealthRegistry, ProbeResult},
    http::serve,
};
use tokio::time::{Duration, sleep};

#[allow(clippy::unwrap_used)]
fn free_port() -> SocketAddr {
    "127.0.0.1:0".parse().unwrap()
}

#[tokio::test]
async fn metrics_endpoint_returns_prometheus_text() {
    let reg = Arc::new(HealthRegistry::new());
    let state = AdminState {
        health: reg,
        admin: AdminConfig::default(),
    };
    let router = build_router_with_state(state);
    let handle = serve(router, free_port()).await.expect("bind");
    let url = format!("http://{}/metrics", handle.addr);

    // Touch a metric so it appears in the response.
    rex_observability::metrics::set_clients_connected(7);

    let resp = reqwest::get(&url).await.expect("request");
    assert!(resp.status().is_success());
    let body = resp.text().await.expect("body");
    assert!(
        body.contains("rex_clients_connected"),
        "metrics body missing expected metric: {}",
        body
    );
    handle.shutdown();
    sleep(Duration::from_millis(100)).await;
}

#[tokio::test]
async fn healthz_returns_200() {
    let reg = Arc::new(HealthRegistry::new());
    let state = AdminState {
        health: reg,
        admin: AdminConfig::default(),
    };
    let router = build_router_with_state(state);
    let handle = serve(router, free_port()).await.expect("bind");
    let url = format!("http://{}/healthz", handle.addr);

    let resp = reqwest::get(&url).await.expect("request");
    assert_eq!(resp.status().as_u16(), 200);
    let body = resp.text().await.expect("body");
    assert_eq!(body, "ok");
    handle.shutdown();
    sleep(Duration::from_millis(100)).await;
}

struct Up;
impl HealthProbe for Up {
    fn name(&self) -> &'static str {
        "up"
    }
    fn check(&self) -> ProbeResult {
        ProbeResult::Healthy
    }
}

#[tokio::test]
async fn readyz_aggregates_probes() {
    let mut reg = HealthRegistry::new();
    reg.register(Arc::new(Up));
    let reg = Arc::new(reg);
    let state = AdminState {
        health: reg,
        admin: AdminConfig::default(),
    };
    let router = build_router_with_state(state);
    let handle = serve(router, free_port()).await.expect("bind");
    let url = format!("http://{}/readyz", handle.addr);

    let resp = reqwest::get(&url).await.expect("request");
    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["status"], "healthy");
    handle.shutdown();
    sleep(Duration::from_millis(100)).await;
}
