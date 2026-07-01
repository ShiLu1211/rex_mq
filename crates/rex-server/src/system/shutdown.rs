//! Cross-cutting shutdown signal for the whole server.
//!
//! One `Shutdown` instance is held by every long-running task that needs to wake
//! on server stop: the periodic cleanup task, every transport's accept loop, every
//! per-connection transport task, and the `AggregateServer::close` coordinator.
//! Calling `signal()` wakes every subscriber.

use std::sync::Arc;

use tokio::sync::broadcast;

/// Capacity large enough for one signal per task in a single server process.
/// `broadcast::Sender::send` returns `Err` only when there are zero receivers —
/// silently dropping the error is intentional: an empty broadcast is a no-op.
const SHUTDOWN_CHANNEL_CAPACITY: usize = 1024;

pub struct Shutdown {
    tx: broadcast::Sender<()>,
}

impl Shutdown {
    /// Create a new shared shutdown signal. Returns `Arc<Self>` because the
    /// typical use is: build once, hand clones to every task that needs to
    /// subscribe or signal.
    pub fn new() -> Arc<Self> {
        let (tx, _) = broadcast::channel(SHUTDOWN_CHANNEL_CAPACITY);
        Arc::new(Self { tx })
    }

    /// Subscribe to the shutdown signal. Each subscriber receives the signal
    /// exactly once; receivers are independent (one slow subscriber does not
    /// block others).
    pub fn subscribe(&self) -> broadcast::Receiver<()> {
        self.tx.subscribe()
    }

    /// Broadcast shutdown to all current and future subscribers (until they
    /// `recv()` once and drop). Safe to call multiple times — every subscriber
    /// wakes once per call. A `send` to a channel with zero receivers is a no-op.
    pub fn signal(&self) {
        let _ = self.tx.send(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn signal_wakes_a_single_subscriber() {
        let shutdown = Shutdown::new();
        let mut rx = shutdown.subscribe();

        shutdown.signal();

        // recv() resolves because the signal was already sent before subscribe
        // ... wait, subscribe was before signal, so this should resolve.
        rx.recv().await.expect("subscriber should wake on signal");
    }

    #[tokio::test]
    async fn signal_wakes_every_subscriber() {
        let shutdown = Shutdown::new();
        let mut rx1 = shutdown.subscribe();
        let mut rx2 = shutdown.subscribe();
        let mut rx3 = shutdown.subscribe();

        shutdown.signal();

        rx1.recv().await.expect("rx1 wakes");
        rx2.recv().await.expect("rx2 wakes");
        rx3.recv().await.expect("rx3 wakes");
    }

    #[tokio::test]
    async fn signal_before_subscribe_is_missed() {
        // Documents the broadcast semantics: late subscribers do not see past
        // signals. This is intentional — shutdown is fire-and-forget, not a
        // durable event log.
        let shutdown = Shutdown::new();
        shutdown.signal();

        let mut rx = shutdown.subscribe();
        // recv would block forever; verify with a short timeout.
        let result = tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv()).await;
        assert!(result.is_err(), "late subscriber must not see prior signal");
    }

    #[tokio::test]
    async fn signal_with_no_subscribers_is_a_noop() {
        let shutdown = Shutdown::new();
        // Must not panic or return an error worth handling.
        shutdown.signal();
    }

    #[tokio::test]
    async fn multiple_signals_wake_subscriber_each_time() {
        let shutdown = Shutdown::new();
        let mut rx = shutdown.subscribe();

        shutdown.signal();
        rx.recv().await.expect("first signal");

        shutdown.signal();
        rx.recv().await.expect("second signal");
    }
}
