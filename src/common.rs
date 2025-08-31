use std::{net::SocketAddr, sync::Arc};

use anyhow::Result;
use async_trait::async_trait;
use bytes::BytesMut;
use dashmap::DashSet;
use uuid::Uuid;

#[async_trait]
pub trait Sender: Sync + Send {
    async fn send_buf(&self, buf: &BytesMut) -> Result<()>;
    async fn close(&self) -> Result<()>;
}

pub struct Client {
    id: usize,
    local_addr: SocketAddr,
    titles: DashSet<String>,
    sender: Arc<dyn Sender>,
}

impl Client {
    pub fn new(id: usize, local_addr: SocketAddr, title: String, sender: Arc<dyn Sender>) -> Self {
        Client {
            id,
            local_addr,
            titles: title.split(';').map(|s| s.to_string()).collect(),
            sender,
        }
    }

    pub fn new_with_title(title: String, sender: Arc<dyn Sender>) -> Self {
        Client {
            id: Uuid::new_v4().as_u128() as usize,
            local_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
            titles: title.split(';').map(|s| s.to_string()).collect(),
            sender,
        }
    }

    pub async fn send_buf(&self, buf: &BytesMut) -> Result<()> {
        self.sender.send_buf(buf).await
    }

    pub async fn close(&self) -> Result<()> {
        self.sender.close().await
    }
}
