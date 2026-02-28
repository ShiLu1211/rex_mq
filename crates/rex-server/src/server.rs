use std::net::SocketAddr;

#[async_trait::async_trait]
pub trait RexServerTrait: Send + Sync {
    async fn close(&self);
    fn addr(&self) -> SocketAddr;
}
