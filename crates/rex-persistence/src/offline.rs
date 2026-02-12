use serde::{Deserialize, Serialize};

/// 离线消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OfflineMessage {
    pub id: u64,
    pub client_id: u128,  // 目标客户端ID
    pub title: String,    // 消息主题
    pub payload: Vec<u8>, // 消息内容
    pub timestamp: u64,   // 消息时间戳
    pub retries: u32,     // 重试次数
}

impl OfflineMessage {
    pub fn new(client_id: u128, title: String, payload: bytes::Bytes) -> Self {
        Self {
            id: gen_message_id(),
            client_id,
            title,
            payload: payload.to_vec(),
            timestamp: rex_core::utils::now_secs(),
            retries: 0,
        }
    }

    pub fn increment_retries(&mut self) {
        self.retries += 1;
    }

    pub fn is_expired(&self, ttl_secs: u64) -> bool {
        let now = rex_core::utils::now_secs();
        now.saturating_sub(self.timestamp) > ttl_secs
    }
}

/// 生成消息ID
fn gen_message_id() -> u64 {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let now = rex_core::utils::now_secs();
    let counter = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    (now << 16) | (counter & 0xFFFF)
}

/// 离线消息队列配置
#[derive(Debug, Clone)]
pub struct OfflineQueueConfig {
    pub max_queue_size: usize, // 每个客户端最大离线消息数
    pub message_ttl: u64,      // 消息过期时间（秒）
    pub cleanup_interval: u64, // 清理间隔（秒）
}

impl Default for OfflineQueueConfig {
    fn default() -> Self {
        Self {
            max_queue_size: 1000,
            message_ttl: 86400 * 7, // 7天
            cleanup_interval: 3600, // 1小时
        }
    }
}
