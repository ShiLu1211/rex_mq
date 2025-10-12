use std::sync::Arc;

use tracing::info;

use crate::RexSystem;

pub struct ConnectionGuard {
    pub system: Arc<RexSystem>,
    pub peer_id: u128,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.system.remove_client(self.peer_id);
        info!("Connection {} cleaned up", self.peer_id);
    }
}
