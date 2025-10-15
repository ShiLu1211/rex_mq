use std::{sync::Arc, time::Duration};

use dashmap::{DashMap, DashSet};
use rand::seq::IteratorRandom;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use crate::{RexClientInner, RexSystemConfig, utils::now_secs};

pub struct RexSystem {
    pub config: RexSystemConfig,
    id2client: DashMap<u128, Arc<RexClientInner>>,
    title2ids: DashMap<String, DashSet<u128>>,
    shutdown_tx: Arc<broadcast::Sender<()>>,
}

impl RexSystem {
    pub fn new(config: RexSystemConfig) -> Arc<Self> {
        let (shutdown_tx, mut shutdown_rx) = broadcast::channel(1);
        let system = Arc::new(Self {
            config,
            id2client: DashMap::new(),
            title2ids: DashMap::new(),
            shutdown_tx: Arc::new(shutdown_tx),
        });

        tokio::spawn({
            let system_clone = system.clone();
            async move {
                let check_interval = Duration::from_secs(system_clone.config.check_interval); // 检查频率
                let client_timeout = system_clone.config.client_timeout; // 客户端超时时间（秒）

                loop {
                    tokio::select! {
                        _ = tokio::time::sleep(check_interval) => {
                            system_clone.cleanup_inactive_clients(client_timeout);
                        }
                        _ = shutdown_rx.recv() => {
                            info!("Cleanup task received shutdown signal, stopping.");
                            break;
                        }
                    }
                }
            }
        });

        system
    }

    pub fn add_client(&self, client: Arc<RexClientInner>) {
        self.id2client.insert(client.id(), client.clone());

        for title in client.title_list() {
            self.title2ids
                .entry(title.to_string())
                .or_default()
                .insert(client.id());
        }
    }

    pub async fn remove_client(&self, client_id: u128) {
        if let Some((_id, client)) = self.id2client.remove(&client_id) {
            for title in client.title_list() {
                if let Some(clients) = self.title2ids.get_mut(&title) {
                    clients.remove(&client_id);
                }
            }
            if let Err(e) = client.close().await {
                warn!("close client [{:032X}] error: {}", client_id, e);
            } else {
                debug!("client [{:032X}] removed", client_id);
            }
        }
    }

    pub fn register_title(&self, client_id: u128, title: &str) {
        if let Some(client) = self.id2client.get(&client_id) {
            // 更新 client 自身的 title 列表
            client.insert_title(title.to_string());

            // 更新系统的映射
            self.title2ids
                .entry(title.to_string())
                .or_default()
                .insert(client_id);
        }
    }

    pub fn unregister_title(&self, client_id: u128, title: &str) {
        if let Some(client) = self.id2client.get(&client_id) {
            // 更新 client 自身的 title 列表
            client.remove_title(title);

            // 更新系统的映射
            if let Some(clients) = self.title2ids.get_mut(title) {
                clients.remove(&client_id);
                if clients.is_empty() {
                    // 没有 client 了，就把这个 title 清理掉
                    self.title2ids.remove(title);
                }
            }
        }
    }

    pub fn find_all(&self) -> Vec<Arc<RexClientInner>> {
        self.id2client
            .iter()
            .map(|client| client.value().clone())
            .collect()
    }

    pub fn find_all_by_title(
        &self,
        title: &str,
        exclude: Option<u128>,
    ) -> Vec<Arc<RexClientInner>> {
        if let Some(clients) = self.title2ids.get(title) {
            clients
                .iter()
                .filter(|id| exclude.is_none_or(|ex| **id != ex)) // 如果 exclude=None 就不过滤
                .filter_map(|id| self.id2client.get(&id).as_deref().cloned())
                .collect()
        } else {
            vec![]
        }
    }

    pub fn find_one_by_title(
        &self,
        title: &str,
        exclude: Option<u128>,
    ) -> Option<Arc<RexClientInner>> {
        if let Some(clients) = self.title2ids.get(title) {
            let mut rng = rand::rng();
            if let Some(id) = clients
                .iter()
                .filter(|id| exclude.is_none_or(|ex| **id != ex))
                .choose(&mut rng)
            {
                return self.id2client.get(&id).as_deref().cloned();
            }
        }

        None
    }

    pub fn find_some_by_id(&self, id: u128) -> Option<Arc<RexClientInner>> {
        self.id2client.get(&id).as_deref().cloned()
    }

    pub async fn close(&self) {
        let _ = self.shutdown_tx.send(());

        for client in self.id2client.iter() {
            if let Err(e) = client.close().await {
                warn!("close client error: {}", e);
            }
        }

        self.id2client.clear();
        self.title2ids.clear();
    }
}

impl RexSystem {
    fn cleanup_inactive_clients(&self, timeout_secs: u64) {
        let mut clients = self.find_all();
        let now = now_secs();
        let initial_count = clients.len();

        clients.retain(|client| {
            let last_active = client.last_recv();
            if now - last_active > timeout_secs {
                warn!(
                    "Client [{:032X}] (addr: {}) timed out, removing...",
                    client.id(),
                    client.local_addr()
                );
                false
            } else {
                true
            }
        });

        let removed_count = initial_count - clients.len();
        if removed_count > 0 {
            info!("Cleaned up {} inactive clients", removed_count);
        }
    }
}
