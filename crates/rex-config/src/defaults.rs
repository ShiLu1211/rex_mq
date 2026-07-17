//! Default values for RexConfig. Every value matches spec §3.4 byte-level.

use std::net::SocketAddr;

use crate::root::{
    AckSection, ClusterSection, EndpointConfig, OfflineSection, PersistenceSection, RexConfig,
    ServerSection,
};
use rex_core::Protocol;
use rex_observability::ObservabilityConfig;

pub fn default_endpoint(protocol: Protocol, port: u16) -> EndpointConfig {
    EndpointConfig {
        protocol,
        address: SocketAddr::new(
            std::net::IpAddr::V4(std::net::Ipv4Addr::new(0, 0, 0, 0)),
            port,
        ),
        enabled: true,
        max_buffer_size: 8 * 1024 * 1024,
        max_concurrent_handlers: 1000,
    }
}

impl Default for RexConfig {
    fn default() -> Self {
        Self {
            server: ServerSection {
                server_id: "rex-server".to_string(),
                shutdown_grace: 10,
                check_interval: 15,
                client_timeout: 45,
            },
            endpoints: vec![
                default_endpoint(Protocol::Tcp, 8_881),
                default_endpoint(Protocol::Quic, 8_882),
                default_endpoint(Protocol::WebSocket, 8_883),
            ],
            cluster: ClusterSection {
                enabled: false,
                cluster_addr: Some("0.0.0.0:19882".parse().expect("static addr")),
                node_id: "auto".to_string(),
                seed_nodes: vec![],
            },
            persistence: PersistenceSection {
                enabled: true,
                path: "./.rex_sled".to_string(),
                offline: OfflineSection {
                    enabled: true,
                    ttl_secs: 7 * 86_400,
                    ghost_ttl_secs: 86_400,
                },
            },
            ack: AckSection {
                enabled: false,
                timeout_ms: 5_000,
                retries: 3,
            },
            observability: ObservabilityConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_matches_doc_table() {
        let d = RexConfig::default();

        // [server]
        assert_eq!(d.server.server_id, "rex-server");
        assert_eq!(d.server.shutdown_grace, 10);
        assert_eq!(d.server.check_interval, 15);
        assert_eq!(d.server.client_timeout, 45);

        // [[endpoints]] — exactly 3 default endpoints
        assert_eq!(d.endpoints.len(), 3, "expected 3 default endpoints");
        let tcp = "0.0.0.0:8881".parse::<SocketAddr>().unwrap();
        let quic = "0.0.0.0:8882".parse::<SocketAddr>().unwrap();
        let ws = "0.0.0.0:8883".parse::<SocketAddr>().unwrap();
        assert_eq!(d.endpoints[0].protocol, Protocol::Tcp);
        assert_eq!(d.endpoints[0].address, tcp);
        assert!(d.endpoints[0].enabled);
        assert_eq!(d.endpoints[1].protocol, Protocol::Quic);
        assert_eq!(d.endpoints[1].address, quic);
        assert_eq!(d.endpoints[2].protocol, Protocol::WebSocket);
        assert_eq!(d.endpoints[2].address, ws);

        // [cluster]
        assert!(!d.cluster.enabled);
        assert_eq!(d.cluster.node_id, "auto");
        assert!(d.cluster.seed_nodes.is_empty());

        // [persistence]
        assert!(d.persistence.enabled);
        assert_eq!(d.persistence.path, "./.rex_sled");
        assert!(d.persistence.offline.enabled);
        assert_eq!(d.persistence.offline.ttl_secs, 604_800);
        assert_eq!(d.persistence.offline.ghost_ttl_secs, 86_400);

        // [ack]
        assert!(!d.ack.enabled);
        assert_eq!(d.ack.timeout_ms, 5_000);
        assert_eq!(d.ack.retries, 3);

        // [observability]
        assert_eq!(d.observability.admin_addr.port(), 9_090);
        assert_eq!(d.observability.admin_token, None);
        assert!(d.observability.single_node_cluster_ok);
    }
}
