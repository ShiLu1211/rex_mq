# ClientState Restoration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Persist client subscriptions so a server restart replays them as ghost entries; live reconnection claims the ghost; expired ghosts are GC'd along with their queued offline messages.

**Architecture:** Split `OfflineBuffer` into two ports (`OfflineBuffer` keeps the message queue; `ClientStateStore` owns persistent client state). Add ghost methods to `ClientRegistry` so restored entries share the registry's lookup path with live ones. Best-effort persistence — every sled failure is logged at warn and surfaces via `last_error()`, never takes the server down.

**Tech Stack:** Rust, sled (existing), bincode (existing), async-trait (existing), `dashmap` (existing), `rex-observability` Prometheus macros (existing).

## Global Constraints

- Persistence is **best-effort**: every sled failure is logged at `warn!`, captured by `last_error()`, and the operation continues. Persistence is for restart, not for correctness.
- On-disk layout is **unchanged**: same three sled trees (`clients`, `offline_queue`, `offline_index`) at the same path.
- `RexSystemConfig::ghost_ttl_secs` defaults to **86400** (24h). All `state_store.save` calls persist `ghost_until = now_secs() + ghost_ttl_secs` so the TTL applies uniformly to live entries (which become ghosts on restart) and to existing ghosts. *(Plan refinement of spec line 152: spec said `u64::MAX` for live, but that would make saved live entries permanent ghosts on restart. This plan applies the TTL uniformly — `ghost_ttl_secs` is meaningful in every save path.)*
- Test sled paths use `tempfile::TempDir` so tests don't leak state. *(Project does not currently depend on `tempfile`; if `tempfile` is not in workspace deps, use the per-test unique subdir under `std::env::temp_dir()` pattern already used in `crates/rex-server/src/system/offline.rs::tests::SLED_COUNTER`.)*
- Every code change compiles and all existing tests pass before the next task begins. Tests are added in the same task as the implementation (TDD: write test → confirm fail → implement → confirm pass).

---

## File Structure

**New files:**
- `crates/rex-persistence/src/client_state_repo.rs` — wraps sled `clients` tree
- `crates/rex-server/src/system/client_state_store.rs` — port trait + adapters + `RestoredClient`

**Modified files (touch list):**
- `crates/rex-persistence/src/lib.rs` — export new types
- `crates/rex-persistence/src/store.rs` — drop now-unused client-state methods (final cleanup task)
- `crates/rex-server/src/system/mod.rs` — export new port + adapters
- `crates/rex-server/src/system/services.rs` — add `state_store` field, migrate `add_client` / `remove_client`
- `crates/rex-server/src/system/client_registry.rs` — add ghost methods to trait + impl
- `crates/rex-server/src/system/offline.rs` — drop `save_client` / `remove_client` from port (final cleanup task)
- `crates/rex-server/src/system/janitor.rs` — extend loop with ghost GC
- `crates/rex-server/src/system/config.rs` — add `ghost_ttl_secs`
- `crates/rex-server/src/lib.rs` — wire startup restore + observability probes
- `crates/rex-server/src/handler/test_util.rs` — update `make_services` for new field
- `crates/rex-observability/src/metrics.rs` — add three new metric helpers

---

## Task 1: Add `ghost_ttl_secs` to `RexSystemConfig`

**Files:**
- Modify: `crates/rex-server/src/system/config.rs:1-106`

**Interfaces:**
- Consumes: nothing
- Produces: `RexSystemConfig { ghost_ttl_secs: u64, ... }` (default 86400)

- [ ] **Step 1: Add the field, default function, and constructor args**

Edit `crates/rex-server/src/system/config.rs`:

After line 25 (`pub ack_retries: u32,`), add:
```rust
    /// TTL (seconds) for restored ghost entries on restart. A live client
    /// saved with `ghost_until = now + ghost_ttl_secs` becomes a ghost with
    /// the same TTL on the next restart. Default 86400s (24h).
    #[serde(default = "default_ghost_ttl_secs")]
    pub ghost_ttl_secs: u64,
```

After `fn default_ack_retries() -> u32 { 3 }` (after line 58), add:
```rust
fn default_ghost_ttl_secs() -> u64 {
    86400
}
```

In `RexSystemConfig::new(...)` (lines 64-89), add `ghost_ttl_secs: u64` as the last parameter and initialize the field. In `RexSystemConfig::from_id(...)` (lines 91-105), add `ghost_ttl_secs: 86400`.

- [ ] **Step 2: Find and update every existing `RexSystemConfig::new` and `from_id` caller**

Run:
```bash
grep -rn "RexSystemConfig::new\|RexSystemConfig::from_id" crates/ --include="*.rs"
```

For every call site, add the trailing `86400` argument. Expected sites: `crates/rex-server/src/system/services.rs` (test helper, line ~311) and any CLI/CLI-args code.

- [ ] **Step 3: Build**

Run:
```bash
cargo build -p rex-server
```
Expected: compiles clean.

- [ ] **Step 4: Commit**

```bash
git add crates/rex-server/src/system/config.rs <any caller files>
git commit -m "feat(config): add RexSystemConfig.ghost_ttl_secs (default 86400)"
```

---

## Task 2: Add `ClientStateRepo` to `rex-persistence` (impl-side)

**Files:**
- Create: `crates/rex-persistence/src/client_state_repo.rs`
- Modify: `crates/rex-persistence/src/lib.rs:1-10`
- Modify: `crates/rex-persistence/src/error.rs` — add `NotFound` variant if not present (verify first)

**Interfaces:**
- Consumes: nothing
- Produces:
  ```rust
  #[derive(Debug, Clone, Serialize, Deserialize)]
  pub struct PersistedClient {
      pub client_id: u128,
      pub titles: Vec<String>,
      pub created_at: u64,
      pub ghost_until: u64,
  }

  pub struct ClientStateRepo { db: sled::Db }   // shares Db with PersistenceStore
  impl ClientStateRepo {
      pub fn new(db: sled::Db) -> Self;
      pub fn save(&self, client: &PersistedClient) -> Result<()>;
      pub fn remove(&self, client_id: u128) -> Result<()>;
      pub fn load_all(&self) -> Result<Vec<PersistedClient>>;
      pub fn take_expired_ghosts(&self, now: u64) -> Result<Vec<u128>>;
  }
  ```

- [ ] **Step 1: Add `NotFound` variant to `PersistenceError` if absent**

Read `crates/rex-persistence/src/error.rs`. If the enum has no `NotFound` variant, add one:
```rust
#[derive(Debug, thiserror::Error)]
pub enum PersistenceError {
    // ... existing variants ...
    #[error("not found")]
    NotFound,
}
```

- [ ] **Step 2: Write the failing test file**

Create `crates/rex-persistence/src/client_state_repo.rs` with the test module only:
```rust
use std::sync::atomic::{AtomicU64, Ordering};

const T_CLIENTS: &str = "clients";

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn fresh_db_path() -> std::path::PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    std::env::temp_dir().join(format!("rex-cs-repo-test-{pid}-{n}"))
}

#[test]
fn save_then_load_round_trips() {
    let path = fresh_db_path();
    std::fs::create_dir_all(&path).unwrap();
    let db = sled::open(&path).unwrap();
    let _ = db.open_tree(T_CLIENTS).unwrap();
    let repo = ClientStateRepo::new(db.clone());

    let client = PersistedClient {
        client_id: 0xABCD,
        titles: vec!["news".to_string(), "weather".to_string()],
        created_at: 100,
        ghost_until: 1_000_000,
    };
    repo.save(&client).unwrap();

    let loaded = repo.load_all().unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].client_id, 0xABCD);
    assert_eq!(loaded[0].titles, vec!["news", "weather"]);
    assert_eq!(loaded[0].created_at, 100);
    assert_eq!(loaded[0].ghost_until, 1_000_000);

    std::fs::remove_dir_all(&path).ok();
}

#[test]
fn remove_deletes_entry() {
    let path = fresh_db_path();
    std::fs::create_dir_all(&path).unwrap();
    let db = sled::open(&path).unwrap();
    let _ = db.open_tree(T_CLIENTS).unwrap();
    let repo = ClientStateRepo::new(db.clone());

    let client = PersistedClient {
        client_id: 0xBEEF,
        titles: vec![],
        created_at: 0,
        ghost_until: 0,
    };
    repo.save(&client).unwrap();
    repo.remove(0xBEEF).unwrap();

    let loaded = repo.load_all().unwrap();
    assert!(loaded.is_empty());

    std::fs::remove_dir_all(&path).ok();
}

#[test]
fn take_expired_ghosts_returns_only_expired_ids() {
    let path = fresh_db_path();
    std::fs::create_dir_all(&path).unwrap();
    let db = sled::open(&path).unwrap();
    let _ = db.open_tree(T_CLIENTS).unwrap();
    let repo = ClientStateRepo::new(db.clone());

    repo.save(&PersistedClient { client_id: 1, titles: vec![], created_at: 0, ghost_until: 50 }).unwrap();
    repo.save(&PersistedClient { client_id: 2, titles: vec![], created_at: 0, ghost_until: 150 }).unwrap();
    repo.save(&PersistedClient { client_id: 3, titles: vec![], created_at: 0, ghost_until: 100 }).unwrap();

    let expired = repo.take_expired_ghosts(100).unwrap();
    assert_eq!(expired, vec![1]);

    std::fs::remove_dir_all(&path).ok();
}

#[test]
fn take_expired_ghosts_removes_returned_entries() {
    let path = fresh_db_path();
    std::fs::create_dir_all(&path).unwrap();
    let db = sled::open(&path).unwrap();
    let _ = db.open_tree(T_CLIENTS).unwrap();
    let repo = ClientStateRepo::new(db.clone());

    repo.save(&PersistedClient { client_id: 1, titles: vec![], created_at: 0, ghost_until: 50 }).unwrap();

    let _ = repo.take_expired_ghosts(100).unwrap();
    let loaded = repo.load_all().unwrap();
    assert!(loaded.is_empty(), "expired ghosts should be removed after take");

    std::fs::remove_dir_all(&path).ok();
}
```

Append to `lib.rs` (line 9 area):
```rust
mod client_state_repo;
pub use client_state_repo::{ClientStateRepo, PersistedClient};
```

- [ ] **Step 3: Run tests to verify they fail to compile**

Run:
```bash
cargo test -p rex-persistence
```
Expected: compile errors (types `ClientStateRepo` / `PersistedClient` not defined). This is the "failing" state.

- [ ] **Step 4: Implement `PersistedClient` and `ClientStateRepo`**

In the same file `crates/rex-persistence/src/client_state_repo.rs`, add above the test module:
```rust
use serde::{Deserialize, Serialize};

use crate::error::{PersistenceError, Result};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersistedClient {
    pub client_id: u128,
    pub titles: Vec<String>,
    pub created_at: u64,
    pub ghost_until: u64,
}

pub struct ClientStateRepo {
    db: sled::Db,
}

impl ClientStateRepo {
    pub fn new(db: sled::Db) -> Self {
        // Touch the tree so it exists on disk before any save.
        let _ = db.open_tree(T_CLIENTS).expect("open clients tree");
        Self { db }
    }

    pub fn save(&self, client: &PersistedClient) -> Result<()> {
        let tree = self.db.open_tree(T_CLIENTS).map_err(|e| PersistenceError::Db(e.to_string()))?;
        let key = client.client_id.to_le_bytes();
        let value = bincode::serialize(client).map_err(PersistenceError::Serialization)?;
        tree.insert(key, value).map_err(|e| PersistenceError::Db(e.to_string()))?;
        Ok(())
    }

    pub fn remove(&self, client_id: u128) -> Result<()> {
        let tree = self.db.open_tree(T_CLIENTS).map_err(|e| PersistenceError::Db(e.to_string()))?;
        let key = client_id.to_le_bytes();
        tree.remove(key).map_err(|e| PersistenceError::Db(e.to_string()))?;
        Ok(())
    }

    pub fn load_all(&self) -> Result<Vec<PersistedClient>> {
        let tree = self.db.open_tree(T_CLIENTS).map_err(|e| PersistenceError::Db(e.to_string()))?;
        let mut out = Vec::new();
        for entry in tree.iter() {
            let (_, value) = entry.map_err(|e| PersistenceError::Db(e.to_string()))?;
            let client: PersistedClient =
                bincode::deserialize(&value).map_err(PersistenceError::Serialization)?;
            out.push(client);
        }
        Ok(out)
    }

    pub fn take_expired_ghosts(&self, now: u64) -> Result<Vec<u128>> {
        let tree = self.db.open_tree(T_CLIENTS).map_err(|e| PersistenceError::Db(e.to_string()))?;
        let mut expired = Vec::new();
        let mut to_remove = Vec::new();
        for entry in tree.iter() {
            let (key, value) = entry.map_err(|e| PersistenceError::Db(e.to_string()))?;
            let client: PersistedClient =
                bincode::deserialize(&value).map_err(PersistenceError::Serialization)?;
            if client.ghost_until < now {
                if key.as_ref().len() == 16 {
                    let mut buf = [0u8; 16];
                    buf.copy_from_slice(key.as_ref());
                    expired.push(u128::from_le_bytes(buf));
                    to_remove.push(key.to_vec());
                }
            }
        }
        for k in to_remove {
            tree.remove(k).map_err(|e| PersistenceError::Db(e.to_string()))?;
        }
        Ok(expired)
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run:
```bash
cargo test -p rex-persistence
```
Expected: 4 tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/rex-persistence/src/
git commit -m "feat(persistence): add ClientStateRepo over the clients sled tree"
```

---

## Task 3: Add `ClientStateStore` port and adapters

**Files:**
- Create: `crates/rex-server/src/system/client_state_store.rs`
- Modify: `crates/rex-server/src/system/mod.rs` (add `pub use client_state_store::*;`)

**Interfaces:**
- Consumes: `rex_persistence::{ClientStateRepo, PersistedClient}` (Task 2)
- Produces:
  ```rust
  pub struct RestoredClient {
      pub client_id: u128,
      pub titles: Vec<String>,
      pub created_at: u64,
      pub ghost_until: u64,
  }

  #[async_trait]
  pub trait ClientStateStore: Send + Sync {
      async fn save(&self, client_id: u128, titles: &[String], created_at: u64, ghost_until: u64);
      async fn remove(&self, client_id: u128);
      async fn load_all(&self) -> Vec<RestoredClient>;
      async fn take_expired_ghosts(&self, now: u64) -> Vec<u128>;
      fn last_error(&self) -> Option<String>;
      async fn close(&self);
  }

  pub struct SledClientStateStore { repo: parking_lot::Mutex<Option<Arc<ClientStateRepo>>>, last_error: Arc<Mutex<Option<String>>> }
  impl SledClientStateStore { pub fn open(path: &str) -> anyhow::Result<Arc<Self>> }

  pub struct NoopClientStateStore;
  ```

- [ ] **Step 1: Write the failing test module**

Create `crates/rex-server/src/system/client_state_store.rs` with the test module only:
```rust
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;

use super::{ClientStateStore, NoopClientStateStore, RestoredClient, SledClientStateStore};

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
    s1.save(0xABCD, &["news".to_string(), "weather".to_string()], 100, 1_000_000).await;
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
    assert!(s.last_error().is_some(), "save after close should set last_error");
    let _ = std::fs::remove_dir_all(&path);
}
```

In `crates/rex-server/src/system/mod.rs`, add `pub use client_state_store::*;` and `pub mod client_state_store;` near the other `system::offline` re-exports.

- [ ] **Step 2: Run tests to verify they fail to compile**

Run:
```bash
cargo test -p rex-server --lib system::client_state_store
```
Expected: compile errors (types not defined).

- [ ] **Step 3: Implement the port + adapters**

In `crates/rex-server/src/system/client_state_store.rs`, add above the test module:
```rust
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
    async fn save(
        &self,
        client_id: u128,
        titles: &[String],
        created_at: u64,
        ghost_until: u64,
    );

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
    repo: parking_lot::Mutex<Option<Arc<ClientStateRepo>>>,
    last_error: Arc<Mutex<Option<String>>>,
}

impl SledClientStateStore {
    /// Open a sled-backed store at `path`. The path is shared with the
    /// offline buffer's `PersistenceStore`; both wrappers operate over
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
        match self.repo.save(&p) {
            Ok(()) => *self.last_error.lock() = None,
            Err(e) => {
                warn!("Failed to save client state: {}", e);
                *self.last_error.lock() = Some(e.to_string());
            }
        }
    }

    async fn remove(&self, client_id: u128) {
        match self.repo.remove(client_id) {
            Ok(()) => *self.last_error.lock() = None,
            Err(e) => {
                warn!("Failed to remove client state: {}", e);
                *self.last_error.lock() = Some(e.to_string());
            }
        }
    }

    async fn load_all(&self) -> Vec<RestoredClient> {
        match self.repo.load_all() {
            Ok(v) => {
                *self.last_error.lock() = None;
                v.into_iter().map(RestoredClient::from).collect()
            }
            Err(e) => {
                warn!("Failed to load client states: {}", e);
                *self.last_error.lock() = Some(e.to_string());
                Vec::new()
            }
        }
    }

    async fn take_expired_ghosts(&self, now: u64) -> Vec<u128> {
        match self.repo.take_expired_ghosts(now) {
            Ok(v) => {
                *self.last_error.lock() = None;
                v
            }
            Err(e) => {
                warn!("Failed to take expired ghosts: {}", e);
                *self.last_error.lock() = Some(e.to_string());
                Vec::new()
            }
        }
    }

    fn last_error(&self) -> Option<String> {
        self.last_error.lock().clone()
    }

    async fn close(&self) {
        // No explicit close needed; sled::Db drops on Arc drop.
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run:
```bash
cargo test -p rex-server --lib system::client_state_store
```
Expected: 4 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/rex-server/src/system/client_state_store.rs crates/rex-server/src/system/mod.rs
git commit -m "feat(system): add ClientStateStore port + sled/noop adapters"
```

---

## Task 4: Add ghost methods to `ClientRegistry`

**Files:**
- Modify: `crates/rex-server/src/system/client_registry.rs:41-94` (trait), `:96-117` (impl struct + ctor), `:119-257` (impl block)

**Interfaces:**
- Consumes: nothing
- Produces (additions to the trait):
  ```rust
  fn add_ghost(&self, client_id: u128, titles: Vec<String>, ghost_until: u64) -> Result<(), DuplicateClient>;
  fn claim_ghost(&self, client_id: u128) -> bool;
  fn remove_ghost(&self, client_id: u128) -> bool;
  fn ghost_count(&self) -> usize;
  fn ghost_titles(&self, client_id: u128) -> Option<Vec<String>>;
  ```

- [ ] **Step 1: Add the `DuplicateClient` error type**

At the top of `crates/rex-server/src/system/client_registry.rs`, after the imports, add:
```rust
/// Returned by [`ClientRegistry::add_ghost`] when a live client OR
/// ghost with the same id is already registered.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum DuplicateClient {
    /// A live client with this id is already registered.
    Live,
    /// A ghost with this id already exists.
    Ghost,
}
```

- [ ] **Step 2: Write the failing tests**

Append to the `mod tests` block at the bottom of `client_registry.rs`:
```rust
    #[test]
    fn add_ghost_then_ghost_count_returns_one() {
        let reg = ClientRegistryImpl::new();
        reg.add_ghost(0xCAFE, vec!["news".to_string()], u64::MAX).unwrap();
        assert_eq!(reg.ghost_count(), 1);
        assert_eq!(reg.ghost_titles(0xCAFE), Some(vec!["news".to_string()]));
    }

    #[test]
    fn add_ghost_with_existing_live_id_returns_live_error() {
        let reg = ClientRegistryImpl::new();
        let c = dummy_client();
        let id = c.id();
        reg.add_client(c);
        let err = reg.add_ghost(id, vec![], u64::MAX).unwrap_err();
        assert_eq!(err, DuplicateClient::Live);
    }

    #[test]
    fn add_ghost_with_existing_ghost_id_returns_ghost_error() {
        let reg = ClientRegistryImpl::new();
        reg.add_ghost(0xCAFE, vec![], u64::MAX).unwrap();
        let err = reg.add_ghost(0xCAFE, vec![], u64::MAX).unwrap_err();
        assert_eq!(err, DuplicateClient::Ghost);
    }

    #[test]
    fn claim_ghost_returns_true_and_drops_entry() {
        let reg = ClientRegistryImpl::new();
        reg.add_ghost(0xCAFE, vec!["a".to_string()], u64::MAX).unwrap();
        assert!(reg.claim_ghost(0xCAFE));
        assert_eq!(reg.ghost_count(), 0);
        assert!(!reg.claim_ghost(0xCAFE));
    }

    #[test]
    fn remove_ghost_returns_true_when_present() {
        let reg = ClientRegistryImpl::new();
        reg.add_ghost(0xCAFE, vec![], u64::MAX).unwrap();
        assert!(reg.remove_ghost(0xCAFE));
        assert_eq!(reg.ghost_count(), 0);
        assert!(!reg.remove_ghost(0xCAFE));
    }

    #[test]
    fn add_ghost_does_not_affect_title_map_for_live_clients() {
        // Ghost titles are stored separately; title_count should not
        // be incremented by add_ghost.
        let reg = ClientRegistryImpl::new();
        reg.add_ghost(0xCAFE, vec!["news".to_string()], u64::MAX).unwrap();
        assert_eq!(reg.title_count(), 0);
        assert!(reg.find_one_by_title("news", None).is_none());
    }
```

- [ ] **Step 3: Run tests to verify they fail**

Run:
```bash
cargo test -p rex-server --lib system::client_registry
```
Expected: compile errors (methods not defined).

- [ ] **Step 4: Add the trait methods**

In the `ClientRegistry` trait (around line 93), add:
```rust
    /// Add a ghost entry. Fails with `DuplicateClient::Live` if a live
    /// client with the same id is registered, or `DuplicateClient::Ghost`
    /// if a ghost already exists.
    fn add_ghost(
        &self,
        client_id: u128,
        titles: Vec<String>,
        ghost_until: u64,
    ) -> Result<(), DuplicateClient>;

    /// Drop a ghost entry if present. Returns true if a ghost was dropped.
    /// Does NOT affect a live client with the same id.
    fn claim_ghost(&self, client_id: u128) -> bool;

    /// Drop a ghost entry. Returns true if a ghost was dropped.
    fn remove_ghost(&self, client_id: u128) -> bool;

    /// Number of currently registered ghost entries.
    fn ghost_count(&self) -> usize;

    /// Return the titles registered for a ghost entry, or `None` if no
    /// ghost with that id exists.
    fn ghost_titles(&self, client_id: u128) -> Option<Vec<String>>;
```

- [ ] **Step 5: Add the storage field + impl methods**

In `ClientRegistryImpl` (line 98), add a field:
```rust
pub struct ClientRegistryImpl {
    id2client: DashMap<u128, Arc<RexClientInner>, RandomState>,
    title2clients: DashMap<String, Vec<Arc<RexClientInner>>, RandomState>,
    ghosts: DashMap<u128, (Vec<String>, u64), RandomState>,
}
```

In `ClientRegistryImpl::new` (line 105), add the initialiser:
```rust
        Arc::new(Self {
            id2client: DashMap::with_hasher(RandomState::new()),
            title2clients: DashMap::with_hasher(RandomState::new()),
            ghosts: DashMap::with_hasher(RandomState::new()),
        })
```

In the `impl ClientRegistry for ClientRegistryImpl` block, append (before the closing brace):
```rust
    fn add_ghost(
        &self,
        client_id: u128,
        titles: Vec<String>,
        ghost_until: u64,
    ) -> Result<(), DuplicateClient> {
        if self.id2client.contains_key(&client_id) {
            return Err(DuplicateClient::Live);
        }
        if self.ghosts.contains_key(&client_id) {
            return Err(DuplicateClient::Ghost);
        }
        self.ghosts.insert(client_id, (titles, ghost_until));
        Ok(())
    }

    fn claim_ghost(&self, client_id: u128) -> bool {
        self.ghosts.remove(&client_id).is_some()
    }

    fn remove_ghost(&self, client_id: u128) -> bool {
        self.ghosts.remove(&client_id).is_some()
    }

    fn ghost_count(&self) -> usize {
        self.ghosts.len()
    }

    fn ghost_titles(&self, client_id: u128) -> Option<Vec<String>> {
        self.ghosts.get(&client_id).map(|e| e.value().0.clone())
    }
```

- [ ] **Step 6: Run tests to verify they pass**

Run:
```bash
cargo test -p rex-server --lib system::client_registry
```
Expected: all tests (existing + new) pass.

- [ ] **Step 7: Commit**

```bash
git add crates/rex-server/src/system/client_registry.rs
git commit -m "feat(registry): add ghost entry lifecycle methods to ClientRegistry"
```

---

## Task 5: Add `state_store` to `Services` and wire `build_services`

**Files:**
- Modify: `crates/rex-server/src/system/services.rs:37-114` (struct + ctor)
- Modify: `crates/rex-server/src/lib.rs:135-194` (`build_services`)
- Modify: `crates/rex-server/src/handler/test_util.rs:154-183` (`make_services`)

**Interfaces:**
- Consumes: `Arc<dyn ClientStateStore>` (Task 3)
- Produces: `Services { state_store: Arc<dyn ClientStateStore>, ... }`

- [ ] **Step 1: Update `Services` struct + ctor**

In `crates/rex-server/src/system/services.rs`, add to the imports (around line 32):
```rust
use crate::system::client_state_store::ClientStateStore;
```

Add field to `Services` (after `forwarder`, before `shutdown`):
```rust
    /// Sled-backed (or no-op) persistent client-state store. Drives
    /// restart restoration and ghost GC. Added per the C5 deepening.
    pub state_store: Arc<dyn ClientStateStore>,
```

In `Services::new(...)`, add the `state_store: Arc<dyn ClientStateStore>` parameter before `shutdown`. Insert the field in the struct literal.

- [ ] **Step 2: Wire `state_store` into `build_services`**

In `crates/rex-server/src/lib.rs::build_services` (lines 135-194), after the offline buffer construction block (after line 153), add:
```rust
    let state_store: Arc<dyn ClientStateStore> = if config.persistence_enabled {
        match SledClientStateStore::open(&config.persistence_path).await {
            Ok(store) => store,
            Err(e) => {
                warn!(
                    "Failed to open client-state store at {}: {}, continuing without it",
                    config.persistence_path, e
                );
                Arc::new(NoopClientStateStore)
            }
        }
    } else {
        Arc::new(NoopClientStateStore)
    };
```

Update the imports in `lib.rs` (around line 13-16) to include `ClientStateStore`, `SledClientStateStore`, `NoopClientStateStore`:
```rust
pub use system::{
    AckTracker, AckTrackerImpl, ClientCancelAdapter, ClientRegistry, ClientRegistryImpl,
    ClientSnapshot, ClientStateStore, ClusterPort, ClusterRouter, DeliveryOutcome, Forwarder,
    FwdResult, Janitor, NetworkForwarder, NoopClientStateStore, NoopOfflineBuffer, OfflineBuffer,
    PendingAckInfo, RegistryObsAdapter, RexSystemConfig, RoutePlan, Router, Services, Shutdown,
    SledClientStateStore, SledOfflineBuffer,
};
```

In the `Services::new(...)` call inside `build_services` (line 175), insert `state_store` as the new parameter before `shutdown`.

- [ ] **Step 3: Update `handler/test_util.rs::make_services`**

In `crates/rex-server/src/handler/test_util.rs`, add `NoopClientStateStore` to the use statement (line 16-19):
```rust
use crate::{
    AckTracker, ClientStateStore, ClusterPort, ClusterRouter, ForwardRequest, NetworkForwarder,
    NoopClientStateStore, NoopOfflineBuffer, OfflineBuffer, PendingAckInfo, RexSystemConfig,
    Services, Shutdown,
};
```

In `make_services` (line 154), after the `offline` declaration, add:
```rust
    let state_store = Arc::new(NoopClientStateStore) as Arc<dyn ClientStateStore>;
```

Insert `state_store` into the `Services::new(...)` call (before `shutdown`).

- [ ] **Step 4: Build and run all tests**

Run:
```bash
cargo build --workspace
cargo test --workspace --no-run
```
Expected: compiles clean. Some tests that depend on Services may have other compilation errors — fix any callsite that constructs `Services::new(...)`.

- [ ] **Step 5: Run the existing test suite**

Run:
```bash
cargo test --workspace
```
Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/rex-server/src/system/services.rs crates/rex-server/src/lib.rs crates/rex-server/src/handler/test_util.rs
git commit -m "feat(services): wire ClientStateStore into Services bundle"
```

---

## Task 6: Migrate `Services::add_client` and `remove_client`

**Files:**
- Modify: `crates/rex-server/src/system/services.rs:147-174` (`add_client` and `remove_client`)
- Modify: `crates/rex-server/src/system/services.rs:281-353` (test helper `make_services`)

**Interfaces:**
- Consumes: `state_store: Arc<dyn ClientStateStore>` (Task 5), `ghost_ttl_secs` (Task 1)
- Produces: `add_client` claims any ghost before adding; `remove_client` removes from `state_store` instead of `offline`.

- [ ] **Step 1: Write the failing tests**

In `crates/rex-server/src/system/services.rs::tests` module, append:
```rust
    #[tokio::test]
    async fn add_client_claims_ghost_before_inserting_live() {
        use crate::system::client_state_store::ClientStateStore;
        use crate::system::client_registry::ClientRegistry;
        let s = make_services();
        let id = 0xCAFE_u128;
        // Pre-register a ghost
        s.registry.add_ghost(id, vec!["ghost_title".to_string()], u64::MAX).unwrap();
        assert_eq!(s.registry.ghost_count(), 1);

        // Add a live client with the same id; ghost should be claimed
        // before live is added.
        let client = crate::handler::test_util::dummy_client_with_id(id);
        s.add_client(client).await;

        assert_eq!(s.registry.ghost_count(), 0, "ghost should be claimed");
        assert!(s.registry.find_some_by_id(id).is_some(), "live should be present");
        assert_eq!(s.state_store.last_error(), None);
    }

    #[tokio::test]
    async fn add_client_persists_state_with_ttl() {
        use crate::system::client_state_store::ClientStateStore;
        let s = make_services();
        let client = crate::handler::test_util::dummy_client_with_id(0xBEEF);
        s.add_client(client).await;

        let loaded = s.state_store.load_all().await;
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].client_id, 0xBEEF);
        // ghost_until should be roughly now + ghost_ttl_secs
        let now = rex_core::utils::now_secs();
        let ttl = loaded[0].ghost_until;
        assert!(ttl >= now + s.config.ghost_ttl_secs - 5);
        assert!(ttl <= now + s.config.ghost_ttl_secs + 5);
    }

    #[tokio::test]
    async fn remove_client_drops_state_store_entry() {
        use crate::system::client_state_store::ClientStateStore;
        let s = make_services();
        let client = crate::handler::test_util::dummy_client_with_id(0xDEAD);
        s.add_client(client.clone()).await;
        assert_eq!(s.state_store.load_all().await.len(), 1);

        s.remove_client(client.id()).await;
        assert_eq!(s.state_store.load_all().await.len(), 0);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run:
```bash
cargo test -p rex-server --lib system::services
```
Expected: tests compile but fail (e.g., `state_store` is `Noop` so load_all returns empty; `ghost_until` is u64::MAX not derived from now).

- [ ] **Step 3: Implement `add_client` and `remove_client` migration**

Replace `Services::add_client` (line 147) with:
```rust
    pub async fn add_client(&self, client: Arc<RexClientInner>) {
        let id = client.id();
        // Drop any ghost entry for this id before inserting live — ghost
        // titles are NOT merged into the live client (see spec decision).
        self.registry.claim_ghost(id);
        self.registry.add_client(client.clone());
        self.cluster.register_client(id);

        let now = rex_core::utils::now_secs();
        let titles: Vec<String> = client.title_iter().collect();
        let ghost_until = now.saturating_add(self.config.ghost_ttl_secs);
        self.state_store.save(id, &titles, now, ghost_until).await;

        set_clients_connected(self.registry.client_count() as i64);
        set_titles_active(self.registry.title_count() as i64);
    }
```

Replace the persistence line in `Services::remove_client` (line 169, `self.offline.remove_client(client_id).await;`) with:
```rust
        self.state_store.remove(client_id).await;
```

Add `use rex_observability::metrics::set_clients_connected;` etc. — these are already imported in services.rs.

- [ ] **Step 4: Run tests to verify they pass**

Run:
```bash
cargo test -p rex-server --lib system::services
```
Expected: all services tests (existing + 3 new) pass.

- [ ] **Step 5: Run full test suite**

Run:
```bash
cargo test --workspace
```
Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/rex-server/src/system/services.rs
git commit -m "feat(services): migrate add/remove_client to ClientStateStore"
```

---

## Task 7: Wire startup restore in `open_server`

**Files:**
- Modify: `crates/rex-server/src/lib.rs:36-130` (`open_server`)

**Interfaces:**
- Consumes: `Services` with `state_store` (Task 5)
- Produces: on `open_server`, restored ghosts in `registry` and `cluster` before transports listen.

- [ ] **Step 1: Add the restore loop**

In `crates/rex-server/src/lib.rs::open_server`, after the cluster manager start block (after line 45), add:
```rust
    // Restore persisted client state as ghost entries. Best-effort:
    // a load_all failure is logged and the server still starts.
    if services.config.persistence_enabled {
        let restored = services.state_store.load_all().await;
        tracing::info!(
            "Restoring {} client entries from persistence",
            restored.len()
        );
        for entry in restored {
            if let Err(e) = services
                .registry
                .add_ghost(entry.client_id, entry.titles.clone(), entry.ghost_until)
            {
                tracing::warn!(
                    "Failed to restore ghost for client {:032X}: {:?}",
                    entry.client_id,
                    e
                );
                continue;
            }
            services.cluster.register_client(entry.client_id);
            tracing::debug!(
                "Restored ghost for client {:032X} ({} titles, ttl={})",
                entry.client_id,
                entry.titles.len(),
                entry.ghost_until
            );
        }
    }
```

- [ ] **Step 2: Build + test**

Run:
```bash
cargo build -p rex-server
cargo test --workspace
```
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add crates/rex-server/src/lib.rs
git commit -m "feat(server): restore persisted client state as ghosts on startup"
```

---

## Task 8: Extend Janitor with ghost GC loop

**Files:**
- Modify: `crates/rex-server/src/system/janitor.rs:1-93`

**Interfaces:**
- Consumes: `Services` with `state_store` (Task 5)
- Produces: Janitor's per-tick loop also calls `cleanup_expired_ghosts`.

- [ ] **Step 1: Write the failing test**

In `crates/rex-server/src/system/janitor.rs::tests` (add the module if absent; otherwise append):
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::client_registry::ClientRegistry;
    use crate::system::client_state_store::ClientStateStore;
    use std::sync::Arc;

    /// Build a Services bundle suitable for Janitor tests: real
    /// ClientRegistry, NoopClientStateStore, NoopOfflineBuffer, real
    /// TestClusterPort via make_services() in handler::test_util.
    fn make_services() -> Arc<Services> {
        crate::handler::test_util::make_services(false)
    }

    #[tokio::test]
    async fn cleanup_expired_ghosts_removes_expired_and_clears_messages() {
        let s = make_services();

        // Seed three ghosts: one expired, two fresh.
        s.registry.add_ghost(1, vec![], 0).unwrap();                // expired
        s.registry.add_ghost(2, vec![], u64::MAX).unwrap();         // live forever
        s.registry.add_ghost(3, vec![], rex_core::utils::now_secs() + 60).unwrap(); // fresh
        s.cluster.register_client(1);
        s.cluster.register_client(2);
        s.cluster.register_client(3);

        // Queue a message for the expired ghost so we can assert it gets cleared.
        s.offline.queue_offline_message(1, "x", bytes::Bytes::from_static(b"hello")).await;

        let janitor = Janitor::new(s.clone());
        janitor.cleanup_expired_ghosts(rex_core::utils::now_secs()).await;

        // Expired ghost gone; fresh ones still present.
        assert!(!s.registry.ghost_count() == 1 == false, "ghost 1 should be gone");
        assert_eq!(s.registry.ghost_count(), 2);
        assert!(s.registry.ghost_titles(1).is_none());
        // Messages for ghost 1 cleared
        assert_eq!(s.offline.get_offline_count(1).await, 0);
        // Cluster unregistered
        let unreg = s.cluster.unregister_calls_test();
        assert!(unreg.contains(&1));
    }
```

> The test relies on a `unregister_calls_test()` helper on the `TestClusterPort` cluster. If absent, add it in `crates/rex-server/src/handler/test_util.rs`:
> ```rust
> impl TestClusterPort {
>     pub fn unregister_calls_test(&self) -> Vec<u128> {
>         self.unregister_calls.lock().clone()
>     }
> }
> ```

- [ ] **Step 2: Implement `cleanup_expired_ghosts`**

In `crates/rex-server/src/system/janitor.rs`, append (before `impl` closes):
```rust
    /// For each ghost whose `ghost_until` has passed, drop the registry
    /// entry, unregister from the cluster, and clear its offline messages.
    /// Best-effort: a failure on any step is logged at warn; the loop
    /// continues to the next id.
    pub async fn cleanup_expired_ghosts(&self, now: u64) {
        let expired = self.services.state_store.take_expired_ghosts(now).await;
        for client_id in expired {
            self.services.registry.remove_ghost(client_id);
            self.services.cluster.unregister_client(client_id);
            self.services.offline.clear_offline_messages(client_id).await;
            tracing::info!(
                "Ghost for client {:032X} expired, removed",
                client_id
            );
        }
    }
```

Wire the call into the existing `run` loop. Replace the `tokio::select!` arm body (around line 30-34):
```rust
                _ = tokio::time::sleep(check_interval) => {
                    self.cleanup_inactive_clients(client_timeout).await;
                    self.cleanup_expired_acks().await;
                    self.cleanup_expired_ghosts(rex_core::utils::now_secs()).await;
                }
```

- [ ] **Step 3: Run the test**

Run:
```bash
cargo test -p rex-server --lib system::janitor
```
Expected: passes.

- [ ] **Step 4: Run full suite**

Run:
```bash
cargo test --workspace
```
Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add crates/rex-server/src/system/janitor.rs crates/rex-server/src/handler/test_util.rs
git commit -m "feat(janitor): add ghost GC sweep"
```

---

## Task 9: Add observability hooks (metrics + readyz probe)

**Files:**
- Modify: `crates/rex-observability/src/metrics.rs` (add three helpers)
- Modify: `crates/rex-server/src/lib.rs:36-130` (wire probe + counter increments)

**Interfaces:**
- Consumes: `Services` with `state_store` (Task 5), `registry.ghost_count()` (Task 4)
- Produces:
  - `rex_client_state_restore_total{result}` counter
  - `rex_client_state_ghosts_current` gauge
  - `rex_client_state_save_latency_seconds` histogram
  - `/readyz` probe aggregating `state_store.last_error()`

- [ ] **Step 1: Add metrics helpers to `rex-observability`**

In `crates/rex-observability/src/metrics.rs`, after the existing `set_cluster_peers` (around line 164), append:
```rust
pub fn inc_client_state_restore(result: &str) {
    lazy_counter_vec!(
        "rex_client_state_restore_total",
        "ClientStateStore restore operations at startup, labelled by outcome",
        &["result"],
    )
    .with_label_values(&[result])
    .inc();
}

pub fn set_client_state_ghosts_current(n: i64) {
    lazy_gauge!(
        "rex_client_state_ghosts_current",
        "Number of ghost entries currently in the registry",
    )
    .set(n);
}

pub fn observe_client_state_save_latency(secs: f64) {
    lazy_histogram_vec!(
        "rex_client_state_save_latency_seconds",
        "Latency of ClientStateStore::save calls",
        &["op"],
    )
    .with_label_values(&["save"])
        .observe(secs);
}
```

- [ ] **Step 2: Wire the counter into the restore loop**

In `crates/rex-server/src/lib.rs`, after the restore loop (Task 7), add:
```rust
    use rex_observability::metrics::{inc_client_state_restore, set_client_state_ghosts_current};
    set_client_state_ghosts_current(services.registry.ghost_count() as i64);
    for entry in services.state_store.load_all().await {
        inc_client_state_restore("ok");
    }
```

(If `load_all` already returned the list above, instead increment per-iteration inside that loop. Either approach works — pick the one that compiles with the existing structure.)

- [ ] **Step 3: Wire the latency observation into `state_store.save` adapter**

This is a refactor: introduce a wrapper in `SledClientStateStore::save` that times the inner call. Simpler: emit the latency at the call site (`Services::add_client`):
```rust
let _t = std::time::Instant::now();
self.state_store.save(id, &titles, now, ghost_until).await;
rex_observability::metrics::observe_client_state_save_latency(_t.elapsed().as_secs_f64());
```

Add the import at the top of `services.rs`:
```rust
use rex_observability::metrics::observe_client_state_save_latency;
```

- [ ] **Step 4: Add a `/readyz` probe**

In `crates/rex-server/src/lib.rs`, after the existing probe registration block (around line 113), add:
```rust
    struct ClientStateStoreAdapter(Arc<dyn crate::ClientStateStore>);
    impl rex_observability::probe::traits::PersistenceSnapshot for ClientStateStoreAdapter {
        fn last_error(&self) -> Option<String> {
            self.0.last_error()
        }
    }

    obs.health
        .register(Arc::new(rex_observability::probe::PersistenceHealthProbe::new(
            Arc::new(ClientStateStoreAdapter(services.state_store.clone())),
        )));
```

If `PersistenceSnapshot` is too narrow (e.g. it presumes a single source), define a sibling trait in `rex-observability::probe::traits`:
```rust
/// Read-only snapshot of a ClientStateStore for `/readyz`.
pub trait ClientStateStoreSnapshot: Send + Sync {
    fn last_error(&self) -> Option<String>;
}
```
…and add a matching probe in `rex-observability::probe::` (mirroring `PersistenceHealthProbe`). The simpler path is to reuse `PersistenceSnapshot`; pick whichever fits the existing trait shape.

- [ ] **Step 5: Build and test**

Run:
```bash
cargo build --workspace
cargo test --workspace
```
Expected: green.

- [ ] **Step 6: Commit**

```bash
git add crates/rex-observability/src/metrics.rs crates/rex-server/src/lib.rs crates/rex-server/src/system/services.rs
git commit -m "feat(observability): client-state metrics and /readyz probe"
```

---

## Task 10: Trim `OfflineBuffer` and `PersistenceStore`

**Files:**
- Modify: `crates/rex-server/src/system/offline.rs:34-56` (trait), `:172-196` (Noop impl), `:85-170` (Sled impl — drop save/remove)
- Modify: `crates/rex-persistence/src/store.rs:89-149` (drop unused methods on `PersistenceStore`)

**Interfaces:**
- Consumes: nothing (dead code removal)
- Produces: `OfflineBuffer` no longer carries `save_client` / `remove_client`; `PersistenceStore` no longer carries `save_client` / `remove_client` / `load_all_clients`.

- [ ] **Step 1: Remove `save_client` / `remove_client` from `OfflineBuffer` trait**

In `crates/rex-server/src/system/offline.rs`, delete from the trait (lines 36-37):
```rust
    async fn save_client(&self, client: &Arc<RexClientInner>);
    async fn remove_client(&self, client_id: u128);
```

- [ ] **Step 2: Remove implementations**

Delete from `SledOfflineBuffer` (lines 86-109) the `save_client` and `remove_client` methods.
Delete from `NoopOfflineBuffer` (lines 179-180) the same.

- [ ] **Step 3: Remove client-state methods from `PersistenceStore`**

In `crates/rex-persistence/src/store.rs`, delete:
- `pub async fn save_client(...)` (lines 91-106)
- `pub async fn load_all_clients(...)` (lines 109-124)
- `pub async fn remove_client(...)` (lines 127-137)
- `pub async fn clear_clients(...)` (lines 140-148)

Also remove `T_CLIENTS` (line 39) and the `db.open_tree(T_CLIENTS)` calls (lines 52-54).

- [ ] **Step 4: Remove the now-unused `ClientState` type**

In `crates/rex-persistence/src/client_state.rs`, the file is now empty of consumers. Delete the file and its `mod client_state;` / `pub use client_state::ClientState;` lines in `lib.rs`.

- [ ] **Step 5: Build + run full test suite**

Run:
```bash
cargo build --workspace
cargo test --workspace
```
Expected: green.

- [ ] **Step 6: Commit**

```bash
git add crates/rex-server/src/system/offline.rs crates/rex-persistence/src/
git commit -m "refactor(persistence): trim client-state methods from OfflineBuffer and PersistenceStore"
```

---

## Task 11: E2E restart test in `rex-test`

**Files:**
- Modify: `crates/rex-test/tests/persistence_test.rs` (extend) OR create `crates/rex-test/tests/restart_restore_test.rs`

**Interfaces:**
- Consumes: the full `open_server` machinery with persistence enabled
- Produces: an end-to-end test that subscribes, restarts the server, and verifies ghost restoration + offline-queue drain.

- [ ] **Step 1: Create the test file**

Create `crates/rex-test/tests/restart_restore_test.rs`:
```rust
//! End-to-end restart restoration test.
//!
//! 1. Start the server with persistence enabled.
//! 2. Connect a client, subscribe to a title.
//! 3. Disconnect the client.
//! 4. Stop the server.
//! 5. Restart the server (same persistence path).
//! 6. Assert: the client's id has a ghost in the registry.
//! 7. Connect a client with the same id; assert: it can re-subscribe
//!    and drain its offline messages.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn fresh_persistence_path() -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    std::env::temp_dir().join(format!("rex-e2e-restart-{pid}-{n}"))
}

#[tokio::test]
async fn restart_restores_ghost_and_drains_offline_messages() {
    use rex_client::Client;
    use rex_server::open_server;
    use rex_server::{RexServerConfig, RexSystemConfig};

    let persistence_path = fresh_persistence_path();
    let bind_addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();

    // ---- First boot ----
    let mut config = RexSystemConfig::from_id("e2e-restart");
    config.persistence_enabled = true;
    config.persistence_path = persistence_path.to_string_lossy().to_string();
    config.check_interval = 1;
    let server_config = RexServerConfig::new(rex_core::Protocol::Tcp, bind_addr);
    let shutdown1 = rex_server::Shutdown::new();
    let services1 = rex_server::build_services(config.clone(), shutdown1.clone(), None).await;
    let server1 = open_server(services1.clone(), server_config.clone()).await.unwrap();
    let bound = server1.local_addr();

    let client = Client::connect(bound).await.expect("connect");
    let client_id = client.id();
    client.subscribe("news").await.expect("subscribe");

    // Trigger server-side persistence: add_client is called by the login
    // handler. Sleep to let it land.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    drop(client);
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Shutdown
    shutdown1.signal();
    server1.shutdown().await;

    // ---- Second boot ----
    let config2 = config.clone();
    let shutdown2 = rex_server::Shutdown::new();
    let services2 = rex_server::build_services(config2, shutdown2.clone(), None).await;
    let server2 = open_server(services2.clone(), server_config).await.unwrap();
    let bound2 = server2.local_addr();

    // Ghost present in registry
    assert!(
        services2.registry.ghost_titles(client_id).is_some(),
        "expected ghost entry for client_id after restart"
    );

    // Reconnect with same id
    let reconnect = Client::connect_with_id(bound2, client_id).await.expect("reconnect");
    reconnect.subscribe("news").await.expect("re-subscribe");

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(
        services2.registry.find_some_by_id(client_id).is_some(),
        "live client should be present after reconnect"
    );
    assert_eq!(
        services2.registry.ghost_count(),
        0,
        "ghost should be claimed on reconnect"
    );

    shutdown2.signal();
    server2.shutdown().await;
    let _ = std::fs::remove_dir_all(&persistence_path);
}
```

> If `Client::connect_with_id`, `Client::connect`, `client.subscribe`, or `RexServerTrait::local_addr` / `shutdown` are not exposed, inspect `crates/rex-client/src/lib.rs` and `crates/rex-server/src/server/mod.rs` to align. The test serves as the contract; the supporting API may need small additions, scoped to this task.

- [ ] **Step 2: Run the test**

Run:
```bash
cargo test -p rex-test --test restart_restore_test -- --nocapture
```
Expected: passes.

- [ ] **Step 3: Commit**

```bash
git add crates/rex-test/tests/restart_restore_test.rs
git commit -m "test(e2e): restart restoration preserves subscriptions and drains offline"
```

---

## Self-Review

**1. Spec coverage:**

| Spec section | Covered by |
|---|---|
| Decisions → Purpose | Task 6, Task 7 |
| Decisions → Cluster interaction | Task 7 |
| Decisions → Ghost lifecycle (TTL) | Task 1, Task 6, Task 8 |
| Decisions → Queue tie-in | Task 8 |
| Decisions → Port split | Task 3, Task 10 |
| Decisions → Persistence layout | Task 2, Task 3, Task 10 |
| Data shapes → `RestoredClient` | Task 3 |
| Data shapes → `PersistedClient` | Task 2 |
| Data shapes → `ClientRegistry` ghost methods | Task 4 |
| Data flow → Startup restore | Task 7 |
| Data flow → Live save | Task 6 |
| Data flow → Live remove | Task 6 |
| Data flow → Ghost GC | Task 8 |
| TTL config | Task 1, Task 6 |
| Error handling | Task 3 (port adapter), Task 6 (best-effort call sites) |
| Observability hooks | Task 9 |
| Testing table | Task 2, Task 3, Task 4, Task 6, Task 8, Task 11 |
| Migration plan 12 steps | Tasks 1-11 (consolidated; cleanup step 10 maps to Task 10) |

**2. Placeholder scan:** No TBDs / TODO / "implement later". Every step shows exact code. One item flagged for the executor: Task 9 Step 4 picks the "reuse PersistenceSnapshot" path or adds a new trait — the executor chooses based on the existing trait shape and the chosen approach is documented inline. Task 11 Step 1 lists API assumptions that the executor verifies against the actual `rex-client` and `rex-server` API surface.

**3. Type consistency:** Method names match across tasks: `add_ghost` / `claim_ghost` / `remove_ghost` / `ghost_count` / `ghost_titles` (Task 4 → Task 7 → Task 8). `state_store.save(id, titles, created_at, ghost_until)` (Task 3 signature) → Task 6 caller uses the same signature. `take_expired_ghosts(now)` returns `Vec<u128>` consistently (Task 2, Task 3, Task 8). `PersistedClient` ↔ `RestoredClient` conversion defined in Task 3.
