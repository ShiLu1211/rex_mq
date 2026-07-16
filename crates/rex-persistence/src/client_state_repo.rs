use serde::{Deserialize, Serialize};

use crate::error::{PersistenceError, Result};

const T_CLIENTS: &str = "clients";

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

    pub fn take_expired_ghosts(&self, now: u64) -> Result<Vec<u128>> {
        let tree = self
            .db
            .open_tree(T_CLIENTS)
            .map_err(|e| PersistenceError::Db(e.to_string()))?;
        let mut expired = Vec::new();
        let mut to_remove = Vec::new();
        for entry in tree.iter() {
            let (key, value) = entry.map_err(|e| PersistenceError::Db(e.to_string()))?;
            let client: PersistedClient =
                bincode::deserialize(&value).map_err(PersistenceError::Serialization)?;
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
}
