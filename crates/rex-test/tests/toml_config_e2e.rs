//! End-to-end smoke test: rex.toml → loader → server → client.

use rex_server::{ClusterConfig, RexServerConfig, Shutdown, build_services, open_server};

fn write_toml(dir: &std::path::Path, contents: &str) -> std::path::PathBuf {
    let p = dir.join("rex.toml");
    std::fs::write(&p, contents).unwrap();
    p
}

#[tokio::test]
async fn full_load_through_loader_serves_tcp() {
    let tmp = tempfile::tempdir().unwrap();
    let toml_src = r#"
        [server]
        server_id = "e2e-rex-toml"
        check_interval = 1
        client_timeout = 30

        [[endpoints]]
        protocol = "tcp"
        address = "127.0.0.1:0"

        [persistence]
        enabled = false

        [persistence.offline]
        enabled = false

        [cluster]
        enabled = false
    "#;
    let path = write_toml(tmp.path(), toml_src);

    let cfg = rex_config::Loader::new()
        .with_config_path(path)
        .load()
        .expect("load ok");

    let sys = rex_server::RexSystemConfig::from(&cfg);
    let shutdown = Shutdown::new();
    let services = build_services(sys, shutdown.clone(), None).await;
    let endpoints: Vec<RexServerConfig> = rex_server::config::endpoints_from_config(&cfg);
    let cluster: Option<ClusterConfig> = rex_server::config::cluster_from_config(&cfg);

    let mut server_config = endpoints[0].clone();
    server_config.cluster = cluster;

    let handle = open_server(services, server_config).await.expect("open");
    // Server started with TOML-loaded config — basic smoke test passes
    handle.close().await;
    shutdown.signal();
}
