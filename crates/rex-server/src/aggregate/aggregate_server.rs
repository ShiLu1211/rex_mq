use anyhow::Result;
use std::net::SocketAddr;
use std::sync::Arc;
use tracing::info;

use crate::{
    AggregateConfig, RexServerConfig, RexServerTrait, RexSystemConfig, Services, Shutdown,
    build_services, open_server,
};

pub struct AggregateServer {
    services: Arc<Services>,
    server_list: Vec<Arc<dyn RexServerTrait>>,
}

impl AggregateServer {
    pub async fn from_config(config: AggregateConfig) -> Result<Self> {
        let shutdown = Shutdown::new();
        let services = build_services(config.system, shutdown, None).await;
        let mut server = Self {
            services,
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

    pub fn new(services: Arc<Services>) -> Self {
        Self {
            services,
            server_list: vec![],
        }
    }

    pub async fn new_with_config(system_config: RexSystemConfig) -> Self {
        let shutdown = Shutdown::new();
        let services = build_services(system_config, shutdown, None).await;
        Self::new(services)
    }

    pub async fn add_server(&mut self, server_config: RexServerConfig) -> Result<()> {
        let server = open_server(self.services.clone(), server_config).await?;
        self.server_list.push(server);
        Ok(())
    }

    pub fn services(&self) -> &Arc<Services> {
        &self.services
    }

    pub fn server_count(&self) -> usize {
        self.server_list.len()
    }
}

#[async_trait::async_trait]
impl RexServerTrait for AggregateServer {
    async fn close(&self) {
        info!(
            "Shutting down aggregate server with {} endpoints",
            self.server_list.len()
        );
        for server in self.server_list.iter() {
            server.close().await;
        }
        self.services.shutdown.signal();
    }

    fn addr(&self) -> SocketAddr {
        self.server_list
            .first()
            .map(|s| s.addr())
            .unwrap_or_else(|| SocketAddr::from(([127, 0, 0, 1], 0)))
    }
}
