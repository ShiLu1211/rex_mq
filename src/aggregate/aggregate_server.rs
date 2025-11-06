#![allow(dead_code)]
use std::sync::Arc;

use anyhow::Result;

use crate::{Protocol, RexServer, RexServerConfig, RexSystem, RexSystemConfig, open_server};

pub struct AggregateServer {
    system: Arc<RexSystem>,
    server_list: Vec<Arc<dyn RexServer>>,
}

impl AggregateServer {
    pub fn new(system: Arc<RexSystem>) -> Self {
        Self {
            system,
            server_list: vec![],
        }
    }

    pub fn new_with_config(system_config: RexSystemConfig) -> Self {
        let system = RexSystem::new(system_config);
        Self::new(system)
    }

    pub async fn add_server(
        &mut self,
        server_config: RexServerConfig,
        protocol: Protocol,
    ) -> Result<()> {
        let server = open_server(self.system.clone(), server_config, protocol).await?;
        self.server_list.push(server);
        Ok(())
    }
}

#[async_trait::async_trait]
impl RexServer for AggregateServer {
    async fn close(&self) {
        for server in self.server_list.iter() {
            server.close().await;
        }
        self.system.close().await;
    }
}
