pub struct RexSystemConfig {
    pub server_id: String,
    pub check_interval: u64,
    pub client_timeout: u64,
}

impl RexSystemConfig {
    pub fn new(server_id: String, check_interval: u64, client_timeout: u64) -> Self {
        Self {
            server_id,
            check_interval,
            client_timeout,
        }
    }

    pub fn from_id(server_id: &str) -> Self {
        Self {
            server_id: server_id.to_string(),
            check_interval: 15,
            client_timeout: 45,
        }
    }
}
