use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RexSystemConfig {
    pub server_id: String,
    #[serde(default = "default_check_interval")]
    pub check_interval: u64,
    #[serde(default = "default_client_timeout")]
    pub client_timeout: u64,
}

fn default_check_interval() -> u64 {
    15
}
fn default_client_timeout() -> u64 {
    45
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
