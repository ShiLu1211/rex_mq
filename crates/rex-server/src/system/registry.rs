use std::{sync::Arc, time::Duration};

use ahash::RandomState;
use dashmap::DashMap;
use rand::seq::IteratorRandom;
use rex_core::{RexClientInner, utils::now_secs};
use rex_persistence::{PersistenceStore, StoreConfig};
use tokio::sync::broadcast;
use tracing::{info, warn};

use crate::RexSystemConfig;

pub struct RexSystem {
    pub config: RexSystemConfig,
    id2client: DashMap<u128, Arc<RexClientInner>, RandomState>,
    title2clients: DashMap<String, Vec<Arc<RexClientInner>>, RandomState>,
    shutdown_tx: Arc<broadcast::Sender<()>>,
    // Persistence
    persistence: Option<Arc<PersistenceStore>>,
}

impl RexSystem {
    pub async fn new(config: RexSystemConfig) -> Arc<Self> {
        let (shutdown_tx, _shutdown_rx) = broadcast::channel(1);
        let shutdown_tx_arc = Arc::new(shutdown_tx);

        // Initialize persistence store
        let persistence = if config.persistence_enabled {
            let store_config = StoreConfig {
                path: config.persistence_path.clone(),
                enable_offline_queue: config.offline_enabled,
                enable_client_persistence: true,
                sync_interval: 1000,
            };
            match PersistenceStore::open(store_config).await {
                Ok(store) => Some(Arc::new(store)),
                Err(e) => {
                    warn!(
                        "Failed to open persistence store: {}, continuing without persistence",
                        e
                    );
                    None
                }
            }
        } else {
            None
        };

        let system = Arc::new(Self {
            config,
            id2client: DashMap::with_hasher(RandomState::new()),
            title2clients: DashMap::with_hasher(RandomState::new()),
            shutdown_tx: shutdown_tx_arc.clone(),
            persistence,
        });

        // Subscribe to shutdown signal for cleanup task
        let mut shutdown_rx = shutdown_tx_arc.subscribe();

        tokio::spawn({
            let system_clone = system.clone();
            async move {
                let check_interval = Duration::from_secs(system_clone.config.check_interval);
                let client_timeout = system_clone.config.client_timeout;

                loop {
                    tokio::select! {
                        _ = tokio::time::sleep(check_interval) => {
                            system_clone.cleanup_inactive_clients(client_timeout).await;
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

    /* ---------------- client lifecycle ---------------- */

    pub async fn add_client(&self, client: Arc<RexClientInner>) {
        let id = client.id();
        self.id2client.insert(id, client.clone());

        for title in client.title_iter() {
            let mut clients = self.title2clients.entry(title).or_default();
            // 避免重复添加
            if !clients.iter().any(|c| c.id() == id) {
                clients.push(client.clone());
            }
        }

        // Save client state to persistence
        self.save_client_state(&client).await;
    }

    pub async fn remove_client(&self, client_id: u128) {
        let client = match self.id2client.remove(&client_id) {
            Some((_id, client)) => client,
            None => return,
        };

        for title in client.title_iter() {
            if let Some(mut clients) = self.title2clients.get_mut(&title) {
                clients.retain(|c| c.id() != client_id);
                if clients.is_empty() {
                    drop(clients);
                    self.title2clients.remove(&title);
                }
            }
        }

        if let Err(e) = client.close().await {
            warn!("close client [{:032X}] error: {}", client_id, e);
        } else {
            info!("client [{:032X}] removed", client_id);
        }

        // Remove client state from persistence
        self.remove_client_state(client_id).await;
    }

    pub fn register_title(&self, client_id: u128, title: &str) {
        let Some(client) = self.id2client.get(&client_id) else {
            return;
        };

        client.insert_title(title);

        let mut clients = self.title2clients.entry(title.to_string()).or_default();
        // 避免重复添加
        if !clients.iter().any(|c| c.id() == client_id) {
            clients.push(client.clone());
        }
    }

    pub fn unregister_title(&self, client_id: u128, title: &str) {
        let Some(client) = self.id2client.get(&client_id) else {
            return;
        };

        client.remove_title(title);

        if let Some(mut clients) = self.title2clients.get_mut(title) {
            clients.retain(|c| c.id() != client_id);
            if clients.is_empty() {
                drop(clients);
                self.title2clients.remove(title);
            }
        }
    }

    /* ---------------- query ---------------- */

    pub fn find_all(&self) -> Vec<Arc<RexClientInner>> {
        self.id2client
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }

    pub fn find_all_by_title(
        &self,
        title: &str,
        exclude: Option<u128>,
    ) -> Vec<Arc<RexClientInner>> {
        let Some(clients) = self.title2clients.get(title) else {
            return Vec::new();
        };

        clients
            .iter()
            .filter(|c| exclude != Some(c.id()))
            .cloned()
            .collect()
    }

    pub fn find_one_by_title(
        &self,
        title: &str,
        exclude: Option<u128>,
    ) -> Option<Arc<RexClientInner>> {
        let clients = self.title2clients.get(title)?;
        let mut rng = rand::rng();

        clients
            .iter()
            .filter(|c| exclude != Some(c.id()))
            .choose(&mut rng)
            .cloned()
    }

    pub fn find_some_by_id(&self, id: u128) -> Option<Arc<RexClientInner>> {
        self.id2client.get(&id).as_deref().cloned()
    }

    /* ---------------- persistence ---------------- */

    /// Get persistence store reference
    pub fn persistence(&self) -> Option<&Arc<PersistenceStore>> {
        self.persistence.as_ref()
    }

    /// Check if persistence is enabled
    pub fn is_persistence_enabled(&self) -> bool {
        self.persistence.is_some()
    }

    /// Save client state to persistence
    pub async fn save_client_state(&self, client: &Arc<RexClientInner>) {
        if let Some(ref store) = self.persistence {
            let state = rex_persistence::ClientState::new(
                client.id(),
                client.title_iter(),
                client.local_addr().to_string(),
            );
            if let Err(e) = store.save_client(&state).await {
                warn!("Failed to save client state: {}", e);
            }
        }
    }

    /// Remove client state from persistence
    pub async fn remove_client_state(&self, client_id: u128) {
        if let Some(ref store) = self.persistence
            && let Err(e) = store.remove_client(client_id).await
        {
            warn!("Failed to remove client state: {}", e);
        }
    }

    /* ---------------- offline messages ---------------- */

    /// Queue message for offline client
    pub async fn queue_offline_message(
        &self,
        target_client_id: u128,
        title: &str,
        payload: bytes::Bytes,
    ) {
        if let Some(ref store) = self.persistence {
            let msg =
                rex_persistence::OfflineMessage::new(target_client_id, title.to_string(), payload);
            if let Err(e) = store.add_offline_message(&msg).await {
                warn!("Failed to queue offline message: {}", e);
            }
        }
    }

    /// Get offline messages for a client (and clear them)
    pub async fn get_offline_messages(
        &self,
        client_id: u128,
    ) -> Vec<rex_persistence::OfflineMessage> {
        if let Some(ref store) = self.persistence
            && let Ok(messages) = store.get_offline_messages(client_id).await
        {
            return messages;
        }
        Vec::new()
    }

    /// Clear offline messages for a client
    pub async fn clear_offline_messages(&self, client_id: u128) {
        if let Some(ref store) = self.persistence
            && let Err(e) = store.clear_offline_messages(client_id).await
        {
            warn!("Failed to clear offline messages: {}", e);
        }
    }

    /// Get offline message count for a client
    pub async fn get_offline_count(&self, client_id: u128) -> usize {
        if let Some(ref store) = self.persistence
            && let Ok(count) = store.get_offline_count(client_id).await
        {
            return count;
        }
        0
    }

    /* ---------------- shutdown ---------------- */

    pub async fn close(&self) {
        let _ = self.shutdown_tx.send(());

        for entry in self.id2client.iter() {
            if let Err(e) = entry.value().close().await {
                warn!("close client error: {}", e);
            }
        }

        self.id2client.clear();
        self.title2clients.clear();

        // Close persistence store
        if let Some(ref store) = self.persistence
            && let Err(e) = store.close().await
        {
            warn!("Error closing persistence store: {}", e);
        }
    }
}

/* ---------------- background cleanup ---------------- */

impl RexSystem {
    async fn cleanup_inactive_clients(&self, timeout_secs: u64) {
        let now = now_secs();
        let mut to_remove = Vec::new();

        for entry in self.id2client.iter() {
            let client = entry.value();
            if now - client.last_recv() > timeout_secs {
                to_remove.push(*entry.key());
            }
        }

        for client_id in to_remove {
            let client = match self.id2client.remove(&client_id) {
                Some((_id, client)) => client,
                None => continue,
            };

            warn!(
                "Client [{:032X}] (addr: {}) timed out, removing...",
                client_id,
                client.local_addr()
            );

            if let Err(e) = client.close().await {
                warn!("close client [{:032X}] error: {}", client_id, e);
            } else {
                info!("client [{:032X}] removed", client_id);
            }

            // 清理 title2clients
            for title in client.title_iter() {
                if let Some(mut clients) = self.title2clients.get_mut(&title) {
                    clients.retain(|c| c.id() != client_id);
                    if clients.is_empty() {
                        drop(clients);
                        self.title2clients.remove(&title);
                    }
                }
            }
        }
    }
}
