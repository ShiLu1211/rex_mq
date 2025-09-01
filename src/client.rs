use std::{net::SocketAddr, sync::Arc};

use anyhow::Result;
use bytes::BytesMut;
use dashmap::DashSet;
use itertools::Itertools;

use crate::{common::new_uuid, sender::RexSender};

pub struct RexClient {
    id: usize,
    local_addr: SocketAddr,
    titles: DashSet<String>,
    sender: Arc<dyn RexSender>,
}

impl RexClient {
    pub fn new(id: usize, local_addr: SocketAddr, title: String, sender: Arc<dyn RexSender>) -> Self {
        RexClient {
            id,
            local_addr,
            titles: title.split(';').map(|s| s.to_string()).collect(),
            sender,
        }
    }

    pub fn new_with_title(title: String, sender: Arc<dyn RexSender>) -> Self {
        RexClient {
            id: new_uuid(),
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

    pub fn id(&self) -> usize {
        self.id
    }

    pub fn set_id(&mut self, id: usize) {
        self.id = id;
    }

    pub fn title_str(&self) -> String {
        self.titles.iter().map(|s| s.to_string()).join(";")
    }

    pub fn set_title(&mut self, title: String) {
        self.titles = title.split(';').map(|s| s.to_string()).collect();
    }

    pub fn has_title(&self, title: &str) -> bool {
        self.titles.contains(title)
    }
}
