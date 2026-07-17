//! End-to-end restart restoration test for the ClientStateStore feature.
//!
//! Verifies that:
//! 1. Subscribing a client persists its state via ClientStateStore.
//! 2. After server restart (same persistence path), the client's id has
//!    a ghost entry in the registry.
//! 3. After client reconnect, claim_ghost drops the ghost and the live
//!    client is present.
//!
//! Uses direct `build_services` + `open_server` so the test controls
//! server lifecycle explicitly (TestEnv owns the server and doesn't
//! cleanly model a "restart" cycle).

use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use rex_core::{Protocol, RexClientInner, RexSenderTrait};
use rex_server::{
    RexServerConfig, RexSystemConfig, Shutdown, build_services, open_server,
};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn fresh_persistence_path() -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    std::env::temp_dir().join(format!("rex-e2e-restart-{pid}-{n}"))
}

struct NoopSender;
#[async_trait]
impl RexSenderTrait for NoopSender {
    async fn send_buf(&self, _buf: &[u8]) -> anyhow::Result<()> {
        Ok(())
    }
    async fn close(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

fn dummy_client_with_id(id: u128) -> Arc<RexClientInner> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
    Arc::new(RexClientInner::new(
        id,
        addr,
        "",
        Arc::new(NoopSender) as Arc<dyn RexSenderTrait>,
    ))
}

#[tokio::test]
async fn restart_restores_ghost_after_live_save() {
    let persistence_path = fresh_persistence_path();
    let bind_addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();

    // ---- First boot: save a live client with a known id ----
    let config = {
        let mut c = RexSystemConfig::from_id("e2e-restart");
        c.persistence_enabled = true;
        c.persistence_path = persistence_path.to_string_lossy().to_string();
        c.check_interval = 1;
        c.ghost_ttl_secs = 60;
        c
    };
    let server_config = RexServerConfig::new(Protocol::Tcp, bind_addr);
    let shutdown1 = Shutdown::new();
    let services1 = build_services(config.clone(), shutdown1.clone(), None).await;
    let server1 = open_server(services1.clone(), server_config.clone())
        .await
        .expect("open server 1");
    server1.ready().await;

    // Build a dummy live client directly via Services.add_client (which
    // now persists state via the ClientStateStore).
    let client_id = 0xCAFE_BEEFu128;
    let live_client = dummy_client_with_id(client_id);
    services1.add_client(live_client.clone()).await;
    services1.cluster.register_client(client_id);

    // Stop server 1. Drop services so the sled Db releases its file
    // lock on the persistence path; wait briefly for the TCP listener
    // to release the bound port.
    server1.close().await;
    shutdown1.signal();
    drop(services1);
    drop(server1);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // ---- Second boot: verify ghost appears from persisted state ----
    let shutdown2 = Shutdown::new();
    let services2 = build_services(config.clone(), shutdown2.clone(), None).await;
    let server2 = open_server(services2.clone(), server_config)
        .await
        .expect("open server 2");
    server2.ready().await;

    // After the restore loop ran in open_server, the registry should
    // contain a ghost for client_id.
    assert!(
        services2.registry.ghost_titles(client_id).is_some(),
        "expected ghost entry for client_id {client_id:032X} after restart"
    );
    assert_eq!(
        services2.registry.ghost_count(),
        1,
        "expected exactly one ghost after restart"
    );

    // ---- Third: simulate reconnect by add_client with the same id ----
    // claim_ghost should have already happened inside add_client.
    let reconnect = dummy_client_with_id(client_id);
    services2.add_client(reconnect).await;

    assert_eq!(
        services2.registry.ghost_count(),
        0,
        "ghost should be claimed on reconnect"
    );
    assert!(
        services2.registry.find_some_by_id(client_id).is_some(),
        "live client should be present after reconnect"
    );

    server2.close().await;
    shutdown2.signal();
    drop(services2);
    drop(server2);
    let _ = std::fs::remove_dir_all(&persistence_path);
}