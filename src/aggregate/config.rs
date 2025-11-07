use crate::{RexServerConfig, RexSystemConfig};
use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateConfig {
    pub system: RexSystemConfig,
    pub servers: Vec<RexServerConfig>,
}

impl AggregateConfig {
    pub fn from_file(path: &str) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: AggregateConfig = toml::from_str(&content)?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        if !self.servers.iter().any(|s| s.enabled) {
            return Err(anyhow::anyhow!("No enabled servers found"));
        }
        Ok(())
    }
}
