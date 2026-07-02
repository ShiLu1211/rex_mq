//! Periodic cleanup task.
//!
//! Owns no state of its own — borrows `Arc<RexSystem>` to reach the
//! `ClientRegistry` and `AckTracker` ports, and a `Shutdown` receiver to
//! exit on server stop. The effects (close inactive clients, deliver ACK
//! timeouts) live here so the ports stay pure state.

use std::{sync::Arc, time::Duration};

use rex_core::{RetCode, RexCommand, utils::now_secs};
use tracing::{info, warn};

use crate::{RexSystem, Shutdown};

/// Background cleanup driver. Spawned by `lib.rs::open_server` (replacing
/// the previous `tokio::spawn` that lived inside `RexSystem::new`).
pub struct Janitor {
    system: Arc<RexSystem>,
}

impl Janitor {
    pub fn new(system: Arc<RexSystem>) -> Self {
        Self { system }
    }

    /// Periodic loop. Wakes every `check_interval` seconds and runs both
    /// cleanup arms. Exits when `Shutdown` signals.
    pub async fn run(self, shutdown: Arc<Shutdown>, check_interval: Duration, client_timeout: u64) {
        let mut shutdown_rx = shutdown.subscribe();

        loop {
            tokio::select! {
                _ = tokio::time::sleep(check_interval) => {
                    self.cleanup_inactive_clients(client_timeout).await;
                    self.cleanup_expired_acks().await;
                }
                _ = shutdown_rx.recv() => {
                    info!("Janitor received shutdown signal, stopping.");
                    break;
                }
            }
        }
    }

    /// Find clients whose last_recv is older than `client_timeout` and close
    /// them. Uses `registry.take_inactive` to identify candidates and
    /// `registry.remove_client` to take ownership of the `Arc<RexClientInner>`
    /// for closing.
    async fn cleanup_inactive_clients(&self, client_timeout: u64) {
        let stale_ids = self.system.registry().take_inactive(client_timeout);
        for client_id in stale_ids {
            let Some(client) = self.system.registry().remove_client(client_id) else {
                continue;
            };

            warn!(
                "Client [{:032X}] (addr: {}) timed out, removing...",
                client_id,
                client.local_addr()
            );

            if let Err(e) = client.close().await {
                warn!("close client [{:032X}] error: {}", client_id, e);
            } else {
                info!("client [{:032X}] removed", client_id);
            }
        }
    }

    /// For each ACK the tracker reports as expired, look up the original
    /// sender via the registry and deliver an `AckReturn` with
    /// `RetCode::AckTimeout`.
    async fn cleanup_expired_acks(&self) {
        if !self.system.is_ack_enabled() {
            return;
        }
        let now = now_secs();

        for (msg_id, source_client_id) in self.system.acks().take_expired(now) {
            let Some(sender) = self.system.registry().find_some_by_id(source_client_id) else {
                continue;
            };

            let ack_data = rex_core::AckData::new(msg_id);
            let rex_data = ack_data.to_rex_data(source_client_id, RexCommand::AckReturn);
            let mut rex_data = rex_data;
            rex_data.set_retcode(RetCode::AckTimeout);

            if let Err(e) = sender.send_buf(rex_data.pack_ref()).await {
                warn!("Failed to send ACK timeout to client: {}", e);
            }
        }
    }
}
