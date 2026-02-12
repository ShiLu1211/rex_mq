use serde::{Deserialize, Serialize};
use std::time::Duration;

/// 客户端状态（可持久化版本）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientState {
    pub client_id: u128,
    pub titles: Vec<String>,
    pub created_at: u64,
    pub last_seen: u64,
    pub local_addr: String,
}

impl ClientState {
    pub fn new(client_id: u128, titles: Vec<String>, local_addr: String) -> Self {
        let now = rex_core::utils::now_secs();
        Self {
            client_id,
            titles,
            created_at: now,
            last_seen: now,
            local_addr,
        }
    }

    pub fn update_last_seen(&mut self) {
        self.last_seen = rex_core::utils::now_secs();
    }

    pub fn idle_duration(&self) -> Duration {
        let now = rex_core::utils::now_secs();
        Duration::from_secs(now.saturating_sub(self.last_seen))
    }
}
