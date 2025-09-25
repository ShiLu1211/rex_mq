use std::sync::Arc;

use dashmap::{DashMap, DashSet};

use crate::RexClientInner;

pub struct System {
    server_id: String,
    clients: DashMap<u128, Arc<RexClientInner>>,
    title_to_client: DashMap<String, DashSet<u128>>,
}

impl System {
    pub fn new(server_id: &str) -> Arc<Self> {
        Arc::new(Self {
            server_id: server_id.to_string(),
            clients: DashMap::new(),
            title_to_client: DashMap::new(),
        })
    }

    pub fn add_client(&self, client: Arc<RexClientInner>) {
        self.clients.insert(client.id(), client.clone());

        // todo
    }
}
