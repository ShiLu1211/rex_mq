#![allow(dead_code)]
use std::sync::Arc;

use anyhow::Result;

use crate::{
    Protocol, QuicServer, RexServer, RexServerConfig, RexSystem, RexSystemConfig, TcpServer,
    WebSocketServer,
};

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
        let server = match protocol {
            Protocol::Tcp => TcpServer::open(self.system.clone(), server_config).await?,
            Protocol::Quic => QuicServer::open(self.system.clone(), server_config).await?,
            Protocol::WebSocket => {
                WebSocketServer::open(self.system.clone(), server_config).await?
            }
        };
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
