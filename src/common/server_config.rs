use std::net::SocketAddr;

pub struct RexServerConfig {
    pub bind_addr: SocketAddr,
    pub check_interval: u64,
    pub client_timeout: u64,
}

impl RexServerConfig {
    pub fn new(bind_addr: SocketAddr, check_interval: u64, client_timeout: u64) -> Self {
        Self {
            bind_addr,
            check_interval,
            client_timeout,
        }
    }

    pub fn from_addr(bind_addr: SocketAddr) -> Self {
        Self {
            bind_addr,
            check_interval: 15,
            client_timeout: 45,
        }
    }
}
