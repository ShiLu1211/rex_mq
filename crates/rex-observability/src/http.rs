//! Shared HTTP server lifecycle: bind, serve, graceful shutdown.

use std::net::SocketAddr;

use axum::Router;
use tokio::sync::watch;

#[derive(Clone)]
pub struct ServerHandle {
    pub addr: SocketAddr,
    shutdown_tx: watch::Sender<bool>,
}

impl ServerHandle {
    /// Trigger graceful shutdown. The HTTP server stops accepting new
    /// connections and waits up to 5s for in-flight requests to complete.
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }
}

pub async fn serve(router: Router, addr: SocketAddr) -> std::io::Result<ServerHandle> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let local_addr = listener.local_addr()?;
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let handle = ServerHandle {
        addr: local_addr,
        shutdown_tx,
    };

    tokio::spawn(async move {
        let server = axum::serve(listener, router).with_graceful_shutdown(async move {
            let mut rx = shutdown_rx;
            let _ = rx.changed().await;
        });
        if let Err(e) = server.await {
            tracing::error!("observability http server error: {}", e);
        }
    });

    Ok(handle)
}
