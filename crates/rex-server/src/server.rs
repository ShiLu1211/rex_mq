use std::net::SocketAddr;

#[async_trait::async_trait]
pub trait RexServerTrait: Send + Sync {
    async fn close(&self);
    fn addr(&self) -> SocketAddr;
    /// Wait until the server's listener is bound and accepting connections.
    /// Tests use this instead of `sleep()` to synchronize on startup.
    async fn ready(&self);
}
