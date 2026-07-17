//! CLI flag overrides — populated by `rex-cli`.

#[derive(Debug, Clone, Default)]
pub struct CliOverrides {
    pub config_path: Option<std::path::PathBuf>,
    pub server_id: Option<String>,
    pub persist: Option<bool>,
    pub cluster_enabled: bool,
    pub cluster_addr: Option<String>,
    pub seeds: Option<String>,
    pub admin_write: bool,
}
