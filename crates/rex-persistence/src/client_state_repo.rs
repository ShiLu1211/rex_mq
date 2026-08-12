use serde::{Deserialize, Serialize};

use crate::error::{PersistenceError, Result};

pub(crate) const T_CLIENTS: &str = "clients";

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
        Self { db }
    }

    pub fn save(&self, client: &PersistedClient) -> Result<()> {
        let tree = self
            .db
            .open_tree(T_CLIENTS)
            .map_err(|e| PersistenceError::Db(e.to_string()))?;
        let key = client.client_id.to_le_bytes();
        let value = bincode::serialize(client).map_err(PersistenceError::Serialization)?;
        tree.insert(key, value)
            .map_err(|e| PersistenceError::Db(e.to_string()))?;
        Ok(())
    }

    /// Force any queued writes to disk. Blocks until flush completes.
    /// sled's default async-flush is too lazy for test harnesses that
    /// simulate a process restart in <1s; production code does not
    /// need to call this.
    pub fn flush(&self) {
        // Flush the T_CLIENTS tree specifically so the per-row
        // bincode writes hit disk. sled::Db::flush only flushes the
        // default tree, which leaves ours buffered.
        if let Ok(tree) = self.db.open_tree(T_CLIENTS)
            && let Err(e) = tree.flush()
        {
            tracing::warn!("client_state_repo: tree.flush failed: {e}");
        }
        if let Err(e) = self.db.flush() {
            tracing::warn!("client_state_repo: db.flush failed: {e}");
        }
    }

    pub fn remove(&self, client_id: u128) -> Result<()> {
        let tree = self
            .db
            .open_tree(T_CLIENTS)
            .map_err(|e| PersistenceError::Db(e.to_string()))?;
        let key = client_id.to_le_bytes();
        tree.remove(key)
            .map_err(|e| PersistenceError::Db(e.to_string()))?;
        Ok(())
    }

    pub fn load_all(&self) -> Result<Vec<PersistedClient>> {
        let tree = self
            .db
            .open_tree(T_CLIENTS)
            .map_err(|e| PersistenceError::Db(e.to_string()))?;
        let mut out = Vec::new();
        for entry in tree.iter() {
            let (_, value) = entry.map_err(|e| PersistenceError::Db(e.to_string()))?;
            let client: PersistedClient =
                bincode::deserialize(&value).map_err(PersistenceError::Serialization)?;
            out.push(client);
        }
        Ok(out)
    }

    /// Sweep all ghost entries whose `ghost_until` is strictly less than `now`
    /// and return their client ids. The matching entries are also removed
    /// from disk so they aren't returned again on the next sweep.
    ///
    /// Best-effort by design:
    ///
    /// - **Boundary**: uses strict `<` against `ghost_until`, meaning a row
    ///   whose `ghost_until == now` is *not* reaped this call. This matches
    ///   the Janitor contract (`take_expired(now)` should drop strictly-past
    ///   entries; the next sweep handles the boundary case). Documented
    ///   behaviour; do not change without updating the contract.
    /// - **Atomicity**: the iter + remove phases are not atomic. If the
    ///   process dies between collecting `to_remove` and applying the
    ///   removes, the next call to `take_expired_ghosts` will re-collect the
    ///   same ids (the rows are still present) and re-remove them. Idempotent.
    /// - **Corrupted rows**: a row whose value fails to deserialize is logged
    ///   and skipped rather than aborting the whole sweep. Stale rows still
    ///   take space on disk but don't block the rest of the cleanup; an
    ///   operator can inspect them via the sled admin tools. This is the
    ///   safer choice for a periodic janitor task.
    pub fn take_expired_ghosts(&self, now: u64) -> Result<Vec<u128>> {
        let tree = self
            .db
            .open_tree(T_CLIENTS)
            .map_err(|e| PersistenceError::Db(e.to_string()))?;
        let mut expired = Vec::new();
        let mut to_remove = Vec::new();
        for entry in tree.iter() {
            let (key, value) = match entry {
                Ok(kv) => kv,
                Err(e) => {
                    tracing::warn!("take_expired_ghosts: skipping corrupted kv pair: {e}");
                    continue;
                }
            };
            let client: PersistedClient = match bincode::deserialize(&value) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("take_expired_ghosts: skipping row with undecodable value: {e}");
                    continue;
                }
            };
            if client.ghost_until < now && key.as_ref().len() == 16 {
                let mut buf = [0u8; 16];
                buf.copy_from_slice(key.as_ref());
                expired.push(u128::from_le_bytes(buf));
                to_remove.push(key.to_vec());
            }
        }
        for k in to_remove {
            tree.remove(k)
                .map_err(|e| PersistenceError::Db(e.to_string()))?;
        }
        Ok(expired)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

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

        repo.save(&PersistedClient {
            client_id: 1,
            titles: vec![],
            created_at: 0,
            ghost_until: 50,
        })
        .unwrap();
        repo.save(&PersistedClient {
            client_id: 2,
            titles: vec![],
            created_at: 0,
            ghost_until: 150,
        })
        .unwrap();
        repo.save(&PersistedClient {
            client_id: 3,
            titles: vec![],
            created_at: 0,
            ghost_until: 100,
        })
        .unwrap();

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

        repo.save(&PersistedClient {
            client_id: 1,
            titles: vec![],
            created_at: 0,
            ghost_until: 50,
        })
        .unwrap();

        let _ = repo.take_expired_ghosts(100).unwrap();
        let loaded = repo.load_all().unwrap();
        assert!(
            loaded.is_empty(),
            "expired ghosts should be removed after take"
        );

        std::fs::remove_dir_all(&path).ok();
    }

    #[test]
    fn take_expired_ghosts_empty_tree_returns_empty_vec() {
        let path = fresh_db_path();
        std::fs::create_dir_all(&path).unwrap();
        let db = sled::open(&path).unwrap();
        let _ = db.open_tree(T_CLIENTS).unwrap();
        let repo = ClientStateRepo::new(db.clone());

        let expired = repo.take_expired_ghosts(100).unwrap();
        assert!(expired.is_empty(), "empty tree should yield no ids");

        std::fs::remove_dir_all(&path).ok();
    }

    #[test]
    fn take_expired_ghosts_all_expired_returns_all_and_clears() {
        let path = fresh_db_path();
        std::fs::create_dir_all(&path).unwrap();
        let db = sled::open(&path).unwrap();
        let _ = db.open_tree(T_CLIENTS).unwrap();
        let repo = ClientStateRepo::new(db.clone());

        // All three entries expired at or before the sweep time.
        repo.save(&PersistedClient {
            client_id: 1,
            titles: vec![],
            created_at: 0,
            ghost_until: 50,
        })
        .unwrap();
        repo.save(&PersistedClient {
            client_id: 2,
            titles: vec![],
            created_at: 0,
            ghost_until: 50,
        })
        .unwrap();
        repo.save(&PersistedClient {
            client_id: 3,
            titles: vec![],
            created_at: 0,
            ghost_until: 50,
        })
        .unwrap();

        let mut expired = repo.take_expired_ghosts(100).unwrap();
        expired.sort_unstable();
        assert_eq!(expired, vec![1, 2, 3]);

        // After a sweep with all reaped, the tree is empty.
        let loaded = repo.load_all().unwrap();
        assert!(
            loaded.is_empty(),
            "all-expired sweep should clear every row"
        );

        std::fs::remove_dir_all(&path).ok();
    }

    #[test]
    fn take_expired_ghosts_skips_corrupted_rows() {
        // Insert a real entry plus a row whose value is not valid
        // bincode for PersistedClient. The corrupted row should be
        // logged + skipped, the real entry reaped if expired.
        let path = fresh_db_path();
        std::fs::create_dir_all(&path).unwrap();
        let db = sled::open(&path).unwrap();
        let tree = db.open_tree(T_CLIENTS).unwrap();

        let real = PersistedClient {
            client_id: 0xABCD,
            titles: vec![],
            created_at: 0,
            ghost_until: 50,
        };
        let real_bytes = bincode::serialize(&real).unwrap();
        tree.insert(0xABCDu128.to_le_bytes(), real_bytes).unwrap();

        // 0xDEAD: 16-byte key but garbage value.
        tree.insert(
            0xDEADu128.to_le_bytes(),
            b"this is not a valid bincode payload".to_vec(),
        )
        .unwrap();

        drop(tree);
        let repo = ClientStateRepo::new(db.clone());

        let expired = repo.take_expired_ghosts(100).unwrap();
        assert_eq!(
            expired,
            vec![0xABCD],
            "corrupted row should not abort the sweep"
        );

        std::fs::remove_dir_all(&path).ok();
    }
}
