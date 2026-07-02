use std::sync::Arc;

use anyhow::Result;
use bytes::BytesMut;
use rex_core::{RexClientInner, RexData};
use tokio::sync::{Semaphore, broadcast};
use tracing::{debug, warn};

use crate::{RexServerConfig, Services, handler::handle};

/// ServerBase — shared per-server state. Each transport holds one.
pub struct ServerBase {
    pub services: Arc<Services>,
    pub config: RexServerConfig,
    pub semaphore: Arc<Semaphore>,
}

impl ServerBase {
    pub fn new(
        services: Arc<Services>,
        config: RexServerConfig,
    ) -> (Self, broadcast::Receiver<()>) {
        let semaphore = Arc::new(Semaphore::new(config.max_concurrent_handlers));
        let shutdown_rx = services.shutdown.subscribe();

        let base = Self {
            services,
            config,
            semaphore,
        };

        (base, shutdown_rx)
    }

    pub fn send_shutdown_signal(&self) {
        self.services.shutdown.signal();
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

    if buffer.len() > 8192 {
        warn!(
            "Buffer too large for connection {} ({}KB), clearing",
            peer_addr,
            buffer.len() / 1024
        );
        buffer.clear();
    }

    Ok(())
}
