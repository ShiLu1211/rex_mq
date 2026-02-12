use std::path::Path;

use tracing::{info, warn};

use crate::{
    client_state::ClientState,
    error::{PersistenceError, Result},
    offline::{OfflineMessage, OfflineQueueConfig},
};

/// 持久化存储主模块
pub struct PersistenceStore {
    db: sled::Db,
    config: StoreConfig,
    offline_config: OfflineQueueConfig,
}

/// 存储配置
#[derive(Debug, Clone)]
pub struct StoreConfig {
    pub path: String,
    pub enable_offline_queue: bool,
    pub enable_client_persistence: bool,
    pub sync_interval: u64, // 同步间隔（毫秒）
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            path: "./.rex_sled".to_string(),
            enable_offline_queue: true,
            enable_client_persistence: true,
            sync_interval: 1000,
        }
    }
}

// 存储树（Table）名称
const T_CLIENTS: &str = "clients";
const T_OFFLINE_QUEUE: &str = "offline_queue";
const T_OFFLINE_INDEX: &str = "offline_index"; // client_id -> [message_ids]

impl PersistenceStore {
    /// 打开或创建数据库
    pub async fn open(config: StoreConfig) -> Result<Self> {
        let path = Path::new(&config.path);
        std::fs::create_dir_all(path).map_err(PersistenceError::Io)?;

        let db = sled::open(path).map_err(|e| PersistenceError::Db(e.to_string()))?;

        // 初始化存储树
        let _ = db
            .open_tree(T_CLIENTS)
            .map_err(|e| PersistenceError::Db(e.to_string()))?;
        let _ = db
            .open_tree(T_OFFLINE_QUEUE)
            .map_err(|e| PersistenceError::Db(e.to_string()))?;
        let _ = db
            .open_tree(T_OFFLINE_INDEX)
            .map_err(|e| PersistenceError::Db(e.to_string()))?;

        info!("Persistence store opened at: {}", config.path);

        Ok(Self {
            db,
            config,
            offline_config: OfflineQueueConfig::default(),
        })
    }

    /// 同步到磁盘
    pub async fn flush(&self) -> Result<()> {
        self.db
            .flush()
            .map_err(|e| PersistenceError::Db(e.to_string()))?;
        Ok(())
    }

    /// 关闭存储
    pub async fn close(&self) -> Result<()> {
        self.flush().await?;
        info!("Persistence store closed");
        Ok(())
    }
}

/* ==================== 客户端状态持久化 ==================== */

impl PersistenceStore {
    /// 保存客户端状态
    pub async fn save_client(&self, state: &ClientState) -> Result<()> {
        if !self.config.enable_client_persistence {
            return Ok(());
        }

        let key = state.client_id.to_le_bytes();
        let value = bincode::serialize(state).map_err(PersistenceError::Serialization)?;

        self.db
            .open_tree(T_CLIENTS)
            .map_err(|e| PersistenceError::Db(e.to_string()))?
            .insert(key, value)
            .map_err(|e| PersistenceError::Db(e.to_string()))?;

        Ok(())
    }

    /// 加载所有客户端状态
    pub async fn load_all_clients(&self) -> Result<Vec<ClientState>> {
        let tree = self
            .db
            .open_tree(T_CLIENTS)
            .map_err(|e| PersistenceError::Db(e.to_string()))?;

        let mut states = Vec::new();
        for entry in tree.iter() {
            let (_, value) = entry.map_err(|e| PersistenceError::Db(e.to_string()))?;
            let state: ClientState =
                bincode::deserialize(&value).map_err(PersistenceError::Serialization)?;
            states.push(state);
        }

        Ok(states)
    }

    /// 删除客户端状态
    pub async fn remove_client(&self, client_id: u128) -> Result<()> {
        let key = client_id.to_le_bytes();

        self.db
            .open_tree(T_CLIENTS)
            .map_err(|e| PersistenceError::Db(e.to_string()))?
            .remove(key)
            .map_err(|e| PersistenceError::Db(e.to_string()))?;

        Ok(())
    }

    /// 清空所有客户端状态
    pub async fn clear_clients(&self) -> Result<()> {
        self.db
            .open_tree(T_CLIENTS)
            .map_err(|e| PersistenceError::Db(e.to_string()))?
            .clear()
            .map_err(|e| PersistenceError::Db(e.to_string()))?;

        Ok(())
    }
}

/* ==================== 离线消息队列 ==================== */

impl PersistenceStore {
    /// 添加离线消息
    pub async fn add_offline_message(&self, msg: &OfflineMessage) -> Result<()> {
        let queue_key = Self::offline_queue_key(&msg.client_id);
        let tree = self
            .db
            .open_tree(T_OFFLINE_INDEX)
            .map_err(|e| PersistenceError::Db(e.to_string()))?;

        // 检查队列是否已满
        let current_size = tree
            .get(queue_key)
            .map_err(|e| PersistenceError::Db(e.to_string()))?
            .map(|v| {
                let ids: Vec<u64> = bincode::deserialize(&v).unwrap_or_default();
                ids.len()
            })
            .unwrap_or(0);

        if current_size >= self.offline_config.max_queue_size {
            warn!(
                "Offline queue full for client {}, dropping oldest message",
                msg.client_id
            );
            // 删除最旧的消息
            self.remove_oldest_offline(&msg.client_id).await?;
        }

        // 存储消息
        let msg_key = msg.id.to_le_bytes();
        let msg_value = bincode::serialize(msg).map_err(PersistenceError::Serialization)?;

        self.db
            .open_tree(T_OFFLINE_QUEUE)
            .map_err(|e| PersistenceError::Db(e.to_string()))?
            .insert(msg_key, msg_value)
            .map_err(|e| PersistenceError::Db(e.to_string()))?;

        // 更新索引
        let mut ids = self.get_offline_message_ids(msg.client_id).await?;
        ids.push(msg.id);
        let index_value = bincode::serialize(&ids).map_err(PersistenceError::Serialization)?;
        tree.insert(queue_key, index_value)
            .map_err(|e| PersistenceError::Db(e.to_string()))?;

        Ok(())
    }

    /// 获取客户端的离线消息
    pub async fn get_offline_messages(&self, client_id: u128) -> Result<Vec<OfflineMessage>> {
        let ids = self.get_offline_message_ids(client_id).await?;
        let tree = self
            .db
            .open_tree(T_OFFLINE_QUEUE)
            .map_err(|e| PersistenceError::Db(e.to_string()))?;

        let mut messages = Vec::new();
        for id in ids {
            let key = id.to_le_bytes();
            if let Some(value) = tree
                .get(key)
                .map_err(|e| PersistenceError::Db(e.to_string()))?
            {
                let msg: OfflineMessage =
                    bincode::deserialize(&value).map_err(PersistenceError::Serialization)?;
                // 跳过过期消息
                if !msg.is_expired(self.offline_config.message_ttl) {
                    messages.push(msg);
                }
            }
        }

        Ok(messages)
    }

    /// 获取离线消息数量
    pub async fn get_offline_count(&self, client_id: u128) -> Result<usize> {
        let ids = self.get_offline_message_ids(client_id).await?;
        Ok(ids.len())
    }

    /// 删除单条离线消息
    pub async fn remove_offline_message(&self, client_id: u128, message_id: u64) -> Result<()> {
        let queue_key = Self::offline_queue_key(&client_id);
        let tree = self
            .db
            .open_tree(T_OFFLINE_INDEX)
            .map_err(|e| PersistenceError::Db(e.to_string()))?;

        // 更新索引
        let mut ids = self.get_offline_message_ids(client_id).await?;
        ids.retain(|&id| id != message_id);
        let index_value = bincode::serialize(&ids).map_err(PersistenceError::Serialization)?;
        tree.insert(queue_key, index_value)
            .map_err(|e| PersistenceError::Db(e.to_string()))?;

        // 删除消息
        let msg_key = message_id.to_le_bytes();
        self.db
            .open_tree(T_OFFLINE_QUEUE)
            .map_err(|e| PersistenceError::Db(e.to_string()))?
            .remove(msg_key)
            .map_err(|e| PersistenceError::Db(e.to_string()))?;

        Ok(())
    }

    /// 删除客户端所有离线消息
    pub async fn clear_offline_messages(&self, client_id: u128) -> Result<()> {
        let ids = self.get_offline_message_ids(client_id).await?;
        let queue_tree = self
            .db
            .open_tree(T_OFFLINE_QUEUE)
            .map_err(|e| PersistenceError::Db(e.to_string()))?;
        let index_tree = self
            .db
            .open_tree(T_OFFLINE_INDEX)
            .map_err(|e| PersistenceError::Db(e.to_string()))?;

        // 删除所有消息
        for id in ids {
            let key = id.to_le_bytes();
            queue_tree
                .remove(key)
                .map_err(|e| PersistenceError::Db(e.to_string()))?;
        }

        // 删除索引
        index_tree
            .remove(Self::offline_queue_key(&client_id))
            .map_err(|e| PersistenceError::Db(e.to_string()))?;

        Ok(())
    }

    /// 清理过期消息
    pub async fn cleanup_expired(&self) -> Result<usize> {
        let mut cleaned = 0;
        let tree = self
            .db
            .open_tree(T_OFFLINE_INDEX)
            .map_err(|e| PersistenceError::Db(e.to_string()))?;
        let ttl = self.offline_config.message_ttl;

        for entry in tree.iter() {
            let (client_key, value) = entry.map_err(|e| PersistenceError::Db(e.to_string()))?;
            let _client_id = u128::from_le_bytes(
                client_key
                    .as_ref()
                    .try_into()
                    .map_err(|_| PersistenceError::InvalidFormat)?,
            );

            let mut ids: Vec<u64> =
                bincode::deserialize(&value).map_err(PersistenceError::Serialization)?;
            let before_len = ids.len();

            // 使用迭代器过滤，避免在闭包中使用 ?
            let queue_tree = self
                .db
                .open_tree(T_OFFLINE_QUEUE)
                .map_err(|e| PersistenceError::Db(e.to_string()))?;

            let mut to_remove = Vec::new();
            for &id in &ids {
                let msg_key = id.to_le_bytes();
                if let Ok(Some(value)) = queue_tree.get(msg_key)
                    && let Ok(msg) = bincode::deserialize::<OfflineMessage>(&value)
                    && msg.is_expired(ttl)
                {
                    to_remove.push(id);
                }
            }
            ids.retain(|&id| !to_remove.contains(&id));

            if ids.len() != before_len {
                let index_value =
                    bincode::serialize(&ids).map_err(PersistenceError::Serialization)?;
                tree.insert(client_key, index_value)
                    .map_err(|e| PersistenceError::Db(e.to_string()))?;
                cleaned += before_len - ids.len();

                // 删除过期消息
                for id in to_remove {
                    let msg_key = id.to_le_bytes();
                    let _ = queue_tree.remove(msg_key);
                }
            }
        }

        Ok(cleaned)
    }

    /* ==================== 内部方法 ==================== */

    fn offline_queue_key(client_id: &u128) -> [u8; 16] {
        client_id.to_le_bytes()
    }

    async fn get_offline_message_ids(&self, client_id: u128) -> Result<Vec<u64>> {
        let queue_key = Self::offline_queue_key(&client_id);
        let tree = self
            .db
            .open_tree(T_OFFLINE_INDEX)
            .map_err(|e| PersistenceError::Db(e.to_string()))?;

        if let Some(value) = tree
            .get(queue_key)
            .map_err(|e| PersistenceError::Db(e.to_string()))?
        {
            let ids: Vec<u64> =
                bincode::deserialize(&value).map_err(PersistenceError::Serialization)?;
            Ok(ids)
        } else {
            Ok(Vec::new())
        }
    }

    async fn remove_oldest_offline(&self, client_id: &u128) -> Result<()> {
        let mut ids = self.get_offline_message_ids(*client_id).await?;
        if let Some(&oldest_id) = ids.first() {
            ids.remove(0);
            self.remove_offline_message(*client_id, oldest_id).await?;
        }
        Ok(())
    }
}
