//! Thin `RexSenderTrait` wrapper that records each `send_buf` call into
//! the `rex_bytes_out_total` Prometheus counter labelled by transport.
//!
//! The rex-sender crate cannot import `rex-observability` (it would pull
//! in the full Prometheus + axum stack), so the per-transport
//! instrumentation lives here and wraps the bare sender at construction
//! time. The wrapper is `Arc`-friendly and lock-free on the hot path —
//! `inc_bytes_out` is a single atomic add.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use rex_core::RexSenderTrait;
use rex_observability::metrics::inc_bytes_out;

/// Wraps an inner `RexSenderTrait` and records `send_buf` byte counts.
pub struct MetricsSender {
    inner: Arc<dyn RexSenderTrait>,
    transport_label: &'static str,
}

impl MetricsSender {
    pub fn new(inner: Arc<dyn RexSenderTrait>, transport_label: &'static str) -> Arc<Self> {
        Arc::new(Self {
            inner,
            transport_label,
        })
    }
}

#[async_trait]
impl RexSenderTrait for MetricsSender {
    async fn send_buf(&self, buf: &[u8]) -> Result<()> {
        inc_bytes_out(self.transport_label, buf.len() as u64);
        self.inner.send_buf(buf).await
    }

    async fn close(&self) -> Result<()> {
        self.inner.close().await
    }
}
