//! ClientStateStore port.
//!
//! Persists per-client session state (id, titles, ghost TTL) across
//! restarts. Distinct from `OfflineBuffer` (which is per-client queued
//! messages) per the C5 deepening of ADR-0002.
//!
//! Two adapters:
//!
//! - `SledClientStateStore` wraps `rex_persistence::ClientStateRepo`.
//!   Opened from a path on disk; survives restarts.
//! - `NoopClientStateStore` does nothing. Used when persistence is disabled
//!   or as a test double.

use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use rex_persistence::{ClientStateRepo, PersistedClient};
use tracing::warn;

/// A restored client entry — what `load_all` returns at startup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoredClient {
    pub client_id: u128,
    pub titles: Vec<String>,
    pub created_at: u64,
    /// Unix timestamp (seconds) past which this entry is eligible for
    /// GC. Compared against `rex_core::utils::now_secs()`.
    pub ghost_until: u64,
}

impl From<PersistedClient> for RestoredClient {
    fn from(p: PersistedClient) -> Self {
        Self {
            client_id: p.client_id,
            titles: p.titles,
            created_at: p.created_at,
            ghost_until: p.ghost_until,
        }
    }
}

#[async_trait]
pub trait ClientStateStore: Send + Sync {
    /// Persist a client entry. `ghost_until` is the timestamp after which
    /// the entry becomes eligible for GC.
    async fn save(&self, client_id: u128, titles: &[String], created_at: u64, ghost_until: u64);

    /// Remove a client entry. Idempotent.
    async fn remove(&self, client_id: u128);

    /// All currently persisted client entries. Order unspecified.
    async fn load_all(&self) -> Vec<RestoredClient>;

    /// Atomically return and remove ids whose `ghost_until < now`.
    /// Used by the Janitor's GC loop.
    async fn take_expired_ghosts(&self, now: u64) -> Vec<u128>;

    /// Last error the store observed (e.g. a sled write failure). None
    /// when no error has happened since process start. Surfaced to
    /// `/readyz` via a probe.
    fn last_error(&self) -> Option<String>;

    /// Flush + close the underlying store. Idempotent.
    async fn close(&self);
}

/// Sled-backed production implementation. Owns a `ClientStateRepo`.
///
/// The repo is held in `Mutex<Option<...>>` so `close()` can release the
/// sled handle and a subsequent `open()` can re-acquire it on the same path.
pub struct SledClientStateStore {
    repo: Mutex<Option<Arc<ClientStateRepo>>>,
    last_error: Arc<Mutex<Option<String>>>,
}

impl SledClientStateStore {
    /// Open a sled-backed store at `path`. The path is shared with
    /// the offline buffer's `PersistenceStore`; both wrappers operate over
    /// distinct trees in the same `sled::Db`.
    pub async fn open(path: &str) -> anyhow::Result<Arc<Self>> {
        use anyhow::Context;

        std::fs::create_dir_all(path).context("create persistence dir")?;
        let db = sled::open(path).context("open sled")?;
        let repo = Arc::new(ClientStateRepo::new(db));
        Ok(Arc::new(Self {
            repo: Mutex::new(Some(repo)),
            last_error: Arc::new(Mutex::new(None)),
        }))
    }

    /// Construct from an already-opened sled `Db`. Used when the
    /// caller wants to share one `sled::Db` across multiple adapters
    /// (offline buffer + this `SledClientStateStore`) to avoid the
    /// sled file-lock contention that happens with multiple
    /// `sled::open` calls on the same path.
    pub fn with_db(db: Arc<sled::Db>) -> Arc<Self> {
        Arc::new(Self {
            repo: Mutex::new(Some(Arc::new(ClientStateRepo::new((*db).clone())))),
            last_error: Arc::new(Mutex::new(None)),
        })
    }
}

#[async_trait]
impl ClientStateStore for SledClientStateStore {
    async fn save(&self, client_id: u128, titles: &[String], created_at: u64, ghost_until: u64) {
        let p = PersistedClient {
            client_id,
            titles: titles.to_vec(),
            created_at,
            ghost_until,
        };
        let result = self.repo.lock().as_ref().map(|repo| repo.save(&p));
        match result {
            Some(Ok(())) => *self.last_error.lock() = None,
            Some(Err(e)) => {
                warn!("Failed to save client state: {}", e);
                *self.last_error.lock() = Some(e.to_string());
            }
            None => {
                warn!("ClientStateStore::save called after close");
                *self.last_error.lock() = Some("store closed".into());
            }
        }
    }

    async fn remove(&self, client_id: u128) {
        let result = self.repo.lock().as_ref().map(|repo| repo.remove(client_id));
        match result {
            Some(Ok(())) => *self.last_error.lock() = None,
            Some(Err(e)) => {
                warn!("Failed to remove client state: {}", e);
                *self.last_error.lock() = Some(e.to_string());
            }
            None => {
                warn!(
                    "ClientStateStore::remove called for client {:032X} after close",
                    client_id
                );
                *self.last_error.lock() = Some("store closed".into());
            }
        }
    }

    async fn load_all(&self) -> Vec<RestoredClient> {
        let result = self.repo.lock().as_ref().map(|repo| repo.load_all());
        match result {
            Some(Ok(v)) => {
                *self.last_error.lock() = None;
                v.into_iter().map(RestoredClient::from).collect()
            }
            Some(Err(e)) => {
                warn!("Failed to load client states: {}", e);
                *self.last_error.lock() = Some(e.to_string());
                Vec::new()
            }
            None => {
                warn!("ClientStateStore::load_all called after close");
                *self.last_error.lock() = Some("store closed".into());
                Vec::new()
            }
        }
    }

    async fn take_expired_ghosts(&self, now: u64) -> Vec<u128> {
        let result = self
            .repo
            .lock()
            .as_ref()
            .map(|repo| repo.take_expired_ghosts(now));
        match result {
            Some(Ok(v)) => {
                *self.last_error.lock() = None;
                v
            }
            Some(Err(e)) => {
                warn!("Failed to take expired ghosts: {}", e);
                *self.last_error.lock() = Some(e.to_string());
                Vec::new()
            }
            None => {
                warn!("ClientStateStore::take_expired_ghosts called after close");
                *self.last_error.lock() = Some("store closed".into());
                Vec::new()
            }
        }
    }

    fn last_error(&self) -> Option<String> {
        self.last_error.lock().clone()
    }

    async fn close(&self) {
        self.repo.lock().take();
    }
}

/// No-op implementation. All writes are silent. Used when persistence is
/// disabled or in tests.
pub struct NoopClientStateStore;

#[async_trait]
impl ClientStateStore for NoopClientStateStore {
    async fn save(&self, _: u128, _: &[String], _: u64, _: u64) {}
    async fn remove(&self, _: u128) {}
    async fn load_all(&self) -> Vec<RestoredClient> {
        Vec::new()
    }
    async fn take_expired_ghosts(&self, _: u64) -> Vec<u128> {
        Vec::new()
    }
    fn last_error(&self) -> Option<String> {
        None
    }
    async fn close(&self) {}
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::{ClientStateStore, NoopClientStateStore, SledClientStateStore};

    static SLED_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn fresh_sled_path() -> String {
        let n = SLED_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        std::env::temp_dir()
            .join(format!("rex-server-cs-test-{pid}-{n}"))
            .to_string_lossy()
            .to_string()
    }

    #[tokio::test]
    async fn noop_save_remove_close_are_silent() {
        let s = NoopClientStateStore;
        s.save(1, &["news".to_string()], 0, 100).await;
        s.remove(1).await;
        assert!(s.load_all().await.is_empty());
        assert!(s.take_expired_ghosts(1000).await.is_empty());
        assert!(s.last_error().is_none());
        s.close().await;
    }

    #[tokio::test]
    async fn sled_round_trip_persists_across_reopen() {
        let path = fresh_sled_path();
        let s1 = SledClientStateStore::open(&path).await.expect("open sled");
        s1.save(
            0xABCD,
            &["news".to_string(), "weather".to_string()],
            100,
            1_000_000,
        )
        .await;
        s1.close().await;

        let s2 = SledClientStateStore::open(&path).await.expect("reopen");
        let loaded = s2.load_all().await;
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].client_id, 0xABCD);
        assert_eq!(loaded[0].titles, vec!["news", "weather"]);
        assert_eq!(loaded[0].created_at, 100);
        assert_eq!(loaded[0].ghost_until, 1_000_000);
        s2.close().await;

        let _ = std::fs::remove_dir_all(&path);
    }

    #[tokio::test]
    async fn sled_take_expired_ghosts_returns_expired_ids_and_removes_them() {
        let path = fresh_sled_path();
        let s = SledClientStateStore::open(&path).await.expect("open sled");
        s.save(1, &[], 0, 50).await;
        s.save(2, &[], 0, 150).await;
        s.save(3, &[], 0, 100).await;

        let expired = s.take_expired_ghosts(100).await;
        assert_eq!(expired, vec![1]);

        let remaining = s.load_all().await;
        let ids: Vec<u128> = remaining.iter().map(|r| r.client_id).collect();
        assert_eq!(ids, vec![2, 3]);

        s.close().await;
        let _ = std::fs::remove_dir_all(&path);
    }

    #[tokio::test]
    async fn save_after_close_sets_last_error() {
        let path = fresh_sled_path();
        let s = SledClientStateStore::open(&path).await.expect("open");
        s.close().await;
        // After close, save should hit the None arm and set last_error.
        s.save(1, &["x".to_string()], 0, 100).await;
        assert!(
            s.last_error().is_some(),
            "save after close should set last_error"
        );
        let _ = std::fs::remove_dir_all(&path);
    }
}
