//! Offline buffer port.
//!
//! Persists client state and queues messages destined for offline clients.
//! This is the **only** async port in the system — `sled`'s I/O is async.
//!
//! Two implementations:
//!
//! - `SledOfflineBuffer` wraps `rex_persistence::PersistenceStore`. Opened
//!   from a path on disk; messages and client state survive restarts.
//! - `NoopOfflineBuffer` does nothing. Used when persistence is disabled
//!   or as a test double. `get_offline_messages` always returns empty,
//!   so the login drain in `handler/login.rs` becomes a no-op too.
//!
//! `login` drains queued messages for a freshly-connected client ID, so
//! reconnecting clients receive any messages that arrived while they were
//! offline. See `handler/login.rs::handle` for the drain logic.

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use parking_lot::Mutex;
use rex_core::RexClientInner;
use rex_persistence::{OfflineMessage, PersistenceStore, StoreConfig};
use tracing::warn;

/// Persistence-backed offline buffer + client-state store.
///
/// All methods are infallible from the caller's perspective: errors are
/// logged at `warn` level and the operation is treated as a no-op. The
/// port's contract is "best effort" — losing a queued message is preferable
/// to taking the server down.
#[allow(dead_code)] // Port added in commit 5; consumed in commit 7+.
#[async_trait]
pub trait OfflineBuffer: Send + Sync {
    async fn save_client(&self, client: &Arc<RexClientInner>);
    async fn remove_client(&self, client_id: u128);

    async fn queue_offline_message(&self, target_client_id: u128, title: &str, payload: Bytes);

    /// Returns messages queued for `client_id`. Order is FIFO by
    /// `OfflineMessage::timestamp`. Caller is expected to follow up with
    /// `clear_offline_messages` once delivery is confirmed.
    async fn get_offline_messages(&self, client_id: u128) -> Vec<OfflineMessage>;

    async fn clear_offline_messages(&self, client_id: u128);
    async fn get_offline_count(&self, client_id: u128) -> usize;

    /// Flush + close the underlying store. Idempotent.
    async fn close(&self);

    /// Returns the last error the buffer observed (e.g. a `sled`
    /// write failure). `None` when no error has happened since process
    /// start. Surfaced to the observability `/readyz` probe.
    fn last_error(&self) -> Option<String>;
}

/// Sled-backed production implementation. Owns its `PersistenceStore`.
#[allow(dead_code)] // Port added in commit 5; consumed in commit 7+.
pub struct SledOfflineBuffer {
    store: Arc<PersistenceStore>,
    /// Last error observed by any write/read. `None` when no error has
    /// happened since process start. Read by the observability
    /// `/readyz` persistence probe.
    last_error: Arc<Mutex<Option<String>>>,
}

impl SledOfflineBuffer {
    pub async fn open(path: String) -> anyhow::Result<Arc<Self>> {
        let config = StoreConfig {
            path,
            enable_offline_queue: true,
            enable_client_persistence: true,
            sync_interval: 1000,
        };
        let store = PersistenceStore::open(config).await?;
        Ok(Arc::new(Self {
            store: Arc::new(store),
            last_error: Arc::new(Mutex::new(None)),
        }))
    }
}

#[async_trait]
impl OfflineBuffer for SledOfflineBuffer {
    async fn save_client(&self, client: &Arc<RexClientInner>) {
        let state = rex_persistence::ClientState::new(
            client.id(),
            client.title_iter(),
            client.local_addr().to_string(),
        );
        match self.store.save_client(&state).await {
            Ok(()) => *self.last_error.lock() = None,
            Err(e) => {
                warn!("Failed to save client state: {}", e);
                *self.last_error.lock() = Some(e.to_string());
            }
        }
    }

    async fn remove_client(&self, client_id: u128) {
        match self.store.remove_client(client_id).await {
            Ok(()) => *self.last_error.lock() = None,
            Err(e) => {
                warn!("Failed to remove client state: {}", e);
                *self.last_error.lock() = Some(e.to_string());
            }
        }
    }

    async fn queue_offline_message(&self, target_client_id: u128, title: &str, payload: Bytes) {
        let msg = OfflineMessage::new(target_client_id, title.to_string(), payload);
        match self.store.add_offline_message(&msg).await {
            Ok(()) => *self.last_error.lock() = None,
            Err(e) => {
                warn!("Failed to queue offline message: {}", e);
                *self.last_error.lock() = Some(e.to_string());
            }
        }
    }

    async fn get_offline_messages(&self, client_id: u128) -> Vec<OfflineMessage> {
        match self.store.get_offline_messages(client_id).await {
            Ok(v) => {
                *self.last_error.lock() = None;
                v
            }
            Err(e) => {
                warn!("Failed to read offline messages: {}", e);
                *self.last_error.lock() = Some(e.to_string());
                Vec::new()
            }
        }
    }

    async fn clear_offline_messages(&self, client_id: u128) {
        match self.store.clear_offline_messages(client_id).await {
            Ok(()) => *self.last_error.lock() = None,
            Err(e) => {
                warn!("Failed to clear offline messages: {}", e);
                *self.last_error.lock() = Some(e.to_string());
            }
        }
    }

    async fn get_offline_count(&self, client_id: u128) -> usize {
        match self.store.get_offline_count(client_id).await {
            Ok(n) => {
                *self.last_error.lock() = None;
                n
            }
            Err(e) => {
                warn!("Failed to read offline count: {}", e);
                *self.last_error.lock() = Some(e.to_string());
                0
            }
        }
    }

    async fn close(&self) {
        if let Err(e) = self.store.close().await {
            warn!("Error closing persistence store: {}", e);
            *self.last_error.lock() = Some(e.to_string());
        }
    }

    fn last_error(&self) -> Option<String> {
        self.last_error.lock().clone()
    }
}

/// No-op implementation. Every method returns immediately; `get_offline_messages`
/// returns an empty `Vec`. Use when persistence is disabled or in tests.
#[allow(dead_code)] // Port added in commit 5; consumed in commit 7+.
pub struct NoopOfflineBuffer;

#[async_trait]
impl OfflineBuffer for NoopOfflineBuffer {
    async fn save_client(&self, _client: &Arc<RexClientInner>) {}
    async fn remove_client(&self, _client_id: u128) {}

    async fn queue_offline_message(&self, _target_client_id: u128, _title: &str, _payload: Bytes) {}

    async fn get_offline_messages(&self, _client_id: u128) -> Vec<OfflineMessage> {
        Vec::new()
    }

    async fn clear_offline_messages(&self, _client_id: u128) {}
    async fn get_offline_count(&self, _client_id: u128) -> usize {
        0
    }
    async fn close(&self) {}
    fn last_error(&self) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rex_core::{RexSenderTrait, utils::new_uuid};
    use std::net::{Ipv4Addr, SocketAddr};
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Per-test unique sled path under the system temp dir. Avoids conflicts
    /// when tests run in parallel and avoids needing the `tempfile` crate.
    static SLED_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn fresh_sled_path() -> String {
        let n = SLED_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        std::env::temp_dir()
            .join(format!("rex-server-offline-test-{pid}-{n}"))
            .to_string_lossy()
            .to_string()
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

    fn dummy_client() -> Arc<RexClientInner> {
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
        Arc::new(RexClientInner::new(
            new_uuid(),
            addr,
            "",
            Arc::new(NoopSender) as Arc<dyn RexSenderTrait>,
        ))
    }

    #[tokio::test]
    async fn noop_save_client_is_silent() {
        let buf = NoopOfflineBuffer;
        let c = dummy_client();
        buf.save_client(&c).await;
    }

    #[tokio::test]
    async fn noop_get_returns_empty_vec() {
        let buf = NoopOfflineBuffer;
        assert!(buf.get_offline_messages(123).await.is_empty());
        assert_eq!(buf.get_offline_count(123).await, 0);
    }

    #[tokio::test]
    async fn noop_queue_is_silent_and_does_not_buffer() {
        let buf = NoopOfflineBuffer;
        buf.queue_offline_message(42, "news", Bytes::from_static(b"hello"))
            .await;
        assert!(buf.get_offline_messages(42).await.is_empty());
    }

    #[tokio::test]
    async fn noop_clear_is_silent() {
        let buf = NoopOfflineBuffer;
        buf.clear_offline_messages(42).await;
    }

    #[tokio::test]
    async fn noop_close_is_silent() {
        let buf = NoopOfflineBuffer;
        buf.close().await;
    }

    #[tokio::test]
    async fn sled_roundtrip_queues_and_drains() {
        let path = fresh_sled_path();
        let buf = SledOfflineBuffer::open(path).await.expect("open sled");

        let target = 0xABCDu128;

        buf.queue_offline_message(target, "news", Bytes::from_static(b"first"))
            .await;
        buf.queue_offline_message(target, "weather", Bytes::from_static(b"second"))
            .await;

        assert_eq!(buf.get_offline_count(target).await, 2);
        let msgs = buf.get_offline_messages(target).await;
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].title, "news");
        assert_eq!(msgs[1].title, "weather");

        buf.clear_offline_messages(target).await;
        assert_eq!(buf.get_offline_count(target).await, 0);
        assert!(buf.get_offline_messages(target).await.is_empty());

        buf.close().await;
    }

    #[tokio::test]
    async fn sled_save_and_remove_client() {
        let path = fresh_sled_path();
        let buf = SledOfflineBuffer::open(path).await.expect("open sled");

        let c = dummy_client();
        let id = c.id();
        buf.save_client(&c).await;
        buf.remove_client(id).await;

        buf.close().await;
    }

    #[tokio::test]
    async fn sled_isolates_per_client_id() {
        let path = fresh_sled_path();
        let buf = SledOfflineBuffer::open(path).await.expect("open sled");

        buf.queue_offline_message(1, "a", Bytes::from_static(b"x"))
            .await;
        buf.queue_offline_message(2, "b", Bytes::from_static(b"y"))
            .await;

        let m1 = buf.get_offline_messages(1).await;
        assert_eq!(m1.len(), 1);
        assert_eq!(m1[0].title, "a");

        let m2 = buf.get_offline_messages(2).await;
        assert_eq!(m2.len(), 1);
        assert_eq!(m2[0].title, "b");

        // Clearing one doesn't affect the other.
        buf.clear_offline_messages(1).await;
        assert!(buf.get_offline_messages(1).await.is_empty());
        assert_eq!(buf.get_offline_messages(2).await.len(), 1);

        buf.close().await;
    }
}
