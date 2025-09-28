use std::sync::Arc;

use dashmap::{DashMap, DashSet};
use rand::seq::IteratorRandom;
use tracing::warn;

use crate::RexClientInner;

pub struct RexSystem {
    server_id: String,
    clients: DashMap<u128, Arc<RexClientInner>>,
    title_to_client: DashMap<String, DashSet<u128>>,
}

impl RexSystem {
    pub fn new(server_id: &str) -> Arc<Self> {
        Arc::new(Self {
            server_id: server_id.to_string(),
            clients: DashMap::new(),
            title_to_client: DashMap::new(),
        })
    }

    pub fn add_client(&self, client: Arc<RexClientInner>) {
        self.clients.insert(client.id(), client.clone());

        for title in client.title_list() {
            self.title_to_client
                .entry(title.to_string())
                .or_default()
                .insert(client.id());
        }
    }

    pub fn remove_client(&self, client_id: u128) {
        if let Some((_id, client)) = self.clients.remove(&client_id) {
            for title in client.title_list() {
                if let Some(clients) = self.title_to_client.get_mut(&title) {
                    clients.remove(&client_id);
                }
            }
        }
    }

    pub fn find_all(&self) -> Vec<Arc<RexClientInner>> {
        self.clients
            .iter()
            .map(|client| client.value().clone())
            .collect()
    }

    pub fn find_all_by_title(
        &self,
        title: &str,
        exclude: Option<u128>,
    ) -> Vec<Arc<RexClientInner>> {
        if let Some(clients) = self.title_to_client.get(title) {
            clients
                .iter()
                .filter(|id| exclude.is_none_or(|ex| **id != ex)) // 如果 exclude=None 就不过滤
                .filter_map(|id| self.clients.get(&id).as_deref().cloned())
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
        if let Some(clients) = self.title_to_client.get(title) {
            let mut rng = rand::rng();
            if let Some(id) = clients
                .iter()
                .filter(|id| exclude.is_none_or(|ex| **id != ex))
                .choose(&mut rng)
            {
                return self.clients.get(&id).as_deref().cloned();
            }
        }

        None
    }

    pub fn find_some_by_id(&self, id: u128) -> Option<Arc<RexClientInner>> {
        self.clients.get(&id).as_deref().cloned()
    }

    pub async fn close(&self) {
        for client in self.clients.iter() {
            if let Err(e) = client.close().await {
                warn!("close client error: {}", e);
            }
        }

        self.clients.clear();
        self.title_to_client.clear();
    }
}
