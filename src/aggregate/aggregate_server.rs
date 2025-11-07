use anyhow::Result;
use std::sync::Arc;
use tracing::info;

use crate::{AggregateConfig, RexServer, RexServerConfig, RexSystem, RexSystemConfig, open_server};

pub struct AggregateServer {
    system: Arc<RexSystem>,
    server_list: Vec<Arc<dyn RexServer>>,
}

impl AggregateServer {
    pub async fn from_config(config: AggregateConfig) -> Result<Self> {
        let system = RexSystem::new(config.system);
        let mut server = Self {
            system,
            server_list: vec![],
        };

        for server_config in config.servers {
            if !server_config.enabled {
                info!(
                    "Skipping disabled server: {:?} on {}",
                    server_config.protocol, server_config.bind_addr
                );
                continue;
            }

            info!(
                "Starting {:?} server on {}",
                server_config.protocol, server_config.bind_addr
            );
            server.add_server(server_config).await?;
        }

        Ok(server)
    }

    pub async fn from_config_file(path: &str) -> Result<Self> {
        let config = AggregateConfig::from_file(path)?;
        Self::from_config(config).await
    }

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

    pub async fn add_server(&mut self, server_config: RexServerConfig) -> Result<()> {
        let server = open_server(self.system.clone(), server_config).await?;
        self.server_list.push(server);
        Ok(())
    }

    pub fn system(&self) -> &Arc<RexSystem> {
        &self.system
    }

    pub fn server_count(&self) -> usize {
        self.server_list.len()
    }
}

#[async_trait::async_trait]
impl RexServer for AggregateServer {
    async fn close(&self) {
        info!(
            "Shutting down aggregate server with {} endpoints",
            self.server_list.len()
        );
        for server in self.server_list.iter() {
            server.close().await;
        }
        self.system.close().await;
    }
}
