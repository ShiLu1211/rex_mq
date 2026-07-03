use std::sync::Arc;

use anyhow::Result;
use bytes::BytesMut;
use rex_core::{RexClientInner, RexData};
use tokio::sync::{Semaphore, broadcast, watch};
use tracing::{debug, warn};

use crate::{RexServerConfig, Services, handler::handle};

/// ServerBase — shared per-server state. Each transport holds one.
pub struct ServerBase {
    pub services: Arc<Services>,
    pub config: RexServerConfig,
    pub semaphore: Arc<Semaphore>,
    /// Readiness signal: 0 = not ready, 1 = listener bound and accepting.
    /// Transports publish via `mark_ready`; consumers wait via `ready_rx`.
    ready_tx: watch::Sender<u8>,
    pub ready_rx: watch::Receiver<u8>,
}

impl ServerBase {
    pub fn new(
        services: Arc<Services>,
        config: RexServerConfig,
    ) -> (Self, broadcast::Receiver<()>) {
        let semaphore = Arc::new(Semaphore::new(config.max_concurrent_handlers));
        let shutdown_rx = services.shutdown.subscribe();
        let (ready_tx, ready_rx) = watch::channel(0u8);

        let base = Self {
            services,
            config,
            semaphore,
            ready_tx,
            ready_rx,
        };

        (base, shutdown_rx)
    }

    pub fn send_shutdown_signal(&self) {
        self.services.shutdown.signal();
    }

    /// Signal that the listener is bound and accepting connections.
    /// Idempotent — calling twice is harmless.
    pub fn mark_ready(&self) {
        let _ = self.ready_tx.send(1);
    }

    /// Wait until `mark_ready` has been called (or the watch channel
    /// itself changes). Resolves immediately if already ready.
    pub async fn wait_ready(&self) {
        let mut rx = self.ready_rx.clone();
        if *rx.borrow_and_update() == 1 {
            return;
        }
        // Wait for the next change to 1, or fall through on error.
        while rx.changed().await.is_ok() {
            if *rx.borrow_and_update() == 1 {
                return;
            }
        }
    }

    pub async fn acquire_connection_permit(&self) -> Result<tokio::sync::OwnedSemaphorePermit> {
        self.semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to acquire connection permit: {}", e))
    }
}

/// Parse and dispatch the buffer's contents. Loop until the buffer
/// either has nothing more to consume, exceeds `max_buffer_size`, or hits
/// a parse error. Free function so the `ConnectionDriver` (T1) and the
/// legacy `ServerBase` callers can both reach it.
pub async fn parse_and_handle_buffer(
    services: &Arc<Services>,
    peer: &Arc<RexClientInner>,
    buffer: &mut BytesMut,
    max_buffer_size: usize,
) -> Result<()> {
    let peer_addr = peer.local_addr();

    loop {
        match RexData::try_deserialize(buffer) {
            Ok(Some(mut rex_data)) => {
                debug!(
                    "Received data from {}: command={:?}",
                    peer_addr,
                    rex_data.command(),
                );

                if let Err(e) = handle(services, peer, &mut rex_data).await {
                    warn!("Error handling data from {}: {}", peer_addr, e);
                }

                peer.update_last_recv();
            }
            Ok(None) => {
                break;
            }
            Err(e) => {
                warn!(
                    "Error parsing data from {}: {}, clearing buffer",
                    peer_addr, e
                );
                buffer.clear();
                break;
            }
        }
    }

    if buffer.len() > max_buffer_size {
        warn!(
            "Buffer too large for connection {} ({}KB), clearing",
            peer_addr,
            buffer.len() / 1024
        );
        buffer.clear();
    }

    Ok(())
}
