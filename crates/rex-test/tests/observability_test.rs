//! E2E tests for the observability framework.
//!
//! These tests stand up a full `rex-server` via `rex_test::factory::TestEnv`,
//! publish a message, and scrape the admin `/metrics` endpoint to confirm
//! the publish-path instrumentation (`rex_messages_published_total`,
//! `rex_messages_delivered_total`) actually fires.

#![allow(clippy::unwrap_used)]

#[cfg(test)]
mod tests {

    use std::time::Duration;

    use anyhow::Result;
    use rex_core::{Protocol, RexCommand};
    use rex_test::factory::TestEnv;
    use strum::IntoEnumIterator;
    use tokio::time::sleep;

    /// The headline test from the Task 12 brief: publish once, scrape
    /// `/metrics`, assert `rex_messages_published_total` appears in the
    /// response body. Runs across every transport the server supports
    /// so the metric path is exercised on TCP / QUIC / WebSocket.
    #[tokio::test]
    async fn publish_increments_counter_in_scrape() -> Result<()> {
        for protocol in Protocol::iter() {
            publish_increments_counter_inner(protocol).await?;
        }
        Ok(())
    }

    async fn publish_increments_counter_inner(protocol: Protocol) -> Result<()> {
        let mut ss = TestEnv::new().await;
        let _server = ss.start_server(protocol).await?;

        // Subscriber (so the publish is routed locally and the
        // delivered counter has a chance to fire as well).
        let mut subscriber = ss.create_client(protocol, "obs_chan").await?;
        subscriber.wait_connected().await;
        // Give the server a beat to register the subscription.
        sleep(Duration::from_millis(100)).await;

        // Publisher on a fresh title (so we don't share counters with
        // parallel tests; the helper uses random ports but the global
        // metric registry is shared).
        let publisher = ss.create_client(protocol, "").await?;
        publisher.wait_connected().await;
        publisher
            .send(RexCommand::Title, "obs_chan", b"hello")
            .await?;

        // Subscriber should see the message — proves the publish path
        // actually traversed the handler before we scrape metrics.
        let recv = tokio::time::timeout(Duration::from_secs(2), subscriber.recv())
            .await
            .expect("timeout waiting for subscriber recv")
            .expect("subscriber rx closed");
        assert_eq!(recv.data(), b"hello");

        // Resolve the admin addr bound by open_server. `open_server`
        // stores it on `Services::admin_addr` after binding the port,
        // so even ephemeral `0`-port setups are reachable from here.
        let admin_addr = ss
            .admin_addr()
            .expect("admin addr should be set after open_server");
        let url = format!("http://{}/metrics", admin_addr);

        let body = reqwest::get(&url)
            .await
            .expect("scrape /metrics")
            .text()
            .await
            .expect("scrape body");

        assert!(
            body.contains("rex_messages_published_total"),
            "metrics body missing `rex_messages_published_total`: {}",
            body
        );
        assert!(
            body.contains("rex_messages_delivered_total"),
            "metrics body missing `rex_messages_delivered_total`: {}",
            body
        );
        assert!(
            body.contains("rex_command_duration_seconds"),
            "metrics body missing `rex_command_duration_seconds`: {}",
            body
        );

        subscriber.close().await;
        publisher.close().await;
        ss.shutdown().await;
        sleep(Duration::from_millis(200)).await;
        Ok(())
    }

    /// A publish that has no matching subscriber must still increment
    /// the published counter — the publish path was *entered* and
    /// accepted by the server, even though no delivery happened.
    #[tokio::test]
    async fn publish_with_no_subscriber_still_counts() -> Result<()> {
        let mut ss = TestEnv::new().await;
        let _server = ss.start_server(Protocol::Tcp).await?;

        let publisher = ss.create_client(Protocol::Tcp, "").await?;
        publisher.wait_connected().await;
        publisher
            .send(RexCommand::Title, "no_subs", b"ping")
            .await?;
        sleep(Duration::from_millis(100)).await;

        let admin_addr = ss.admin_addr().expect("admin addr");
        let body = reqwest::get(format!("http://{}/metrics", admin_addr))
            .await
            .expect("scrape")
            .text()
            .await
            .expect("body");

        assert!(
            body.contains("rex_messages_published_total"),
            "no-target publish should still bump the published counter: {}",
            body
        );

        publisher.close().await;
        ss.shutdown().await;
        sleep(Duration::from_millis(200)).await;
        Ok(())
    }

    /// The `rex_cluster_peers` gauge must show up in `/metrics` even
    /// when cluster is disabled. `build_services` initialises the
    /// gauge to 0 so the metric is registered at scrape time; the
    /// `ServerClusterManager` then updates it as nodes join/leave.
    #[tokio::test]
    async fn cluster_peers_gauge_appears_in_scrape() -> Result<()> {
        let mut ss = TestEnv::new().await;
        let _server = ss.start_server(Protocol::Tcp).await?;

        // Give the admin server a moment to bind and the gauge to be
        // initialised.
        sleep(Duration::from_millis(200)).await;

        let admin_addr = ss.admin_addr().expect("admin addr");
        let body = reqwest::get(format!("http://{}/metrics", admin_addr))
            .await
            .expect("scrape /metrics")
            .text()
            .await
            .expect("scrape body");

        assert!(
            body.contains("rex_cluster_peers"),
            "metrics body missing `rex_cluster_peers`: {}",
            body
        );

        ss.shutdown().await;
        sleep(Duration::from_millis(200)).await;
        Ok(())
    }
}
