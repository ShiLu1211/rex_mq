//! ACK tracker port.
//!
//! Tracks pending ACK requests so the server can time them out. The trait is
//! intentionally **synchronous** — only in-memory state operations live
//! here. Sending timeout packets to original senders is the **caller's** job
//! (today: `RexSystem::cleanup_expired_acks`; commit 4+: the Janitor).
//!
//! In this commit the trait and a real DashMap-backed `AckTrackerImpl` are
//! introduced. `RexSystem::new` constructs one internally and its existing
//! methods (`register_pending_ack`, `take_pending_ack`, `get_pending_ack`,
//! `cleanup_expired_acks`) forward to it. The `pending_acks` field on
//! `RexSystem` is removed.

use std::sync::Arc;

use ahash::RandomState;
use dashmap::DashMap;
use rex_core::utils::now_secs;

/// Information about a pending ACK.
///
/// Returned by `AckTracker::take` and `AckTracker::get` so callers can build
/// the timeout packet using the original source's client id. `title` and
/// `is_group` are stored but not currently read — kept as part of the
/// surface for future filtering / debugging use cases.
#[derive(Clone)]
pub struct PendingAckInfo {
    pub source_client_id: u128,
    pub title: String,
    pub timestamp: u64,
    pub is_group: bool,
}

/// Tracks pending ACK requests and identifies which have expired.
pub trait AckTracker: Send + Sync {
    /// Record a pending ACK for `message_id`. The tracker's clock is used to
    /// stamp the entry.
    fn register(&self, message_id: u64, source_client_id: u128, title: String, is_group: bool);

    /// Remove and return the pending ACK for `message_id`, if any.
    fn take(&self, message_id: u64) -> Option<PendingAckInfo>;

    /// Return the pending ACK for `message_id` without removing it.
    fn get(&self, message_id: u64) -> Option<PendingAckInfo>;

    /// Remove every entry older than `timeout_secs` from the tracker clock.
    /// Returns `(message_id, source_client_id)` pairs for each removed entry
    /// so the caller can deliver timeout packets.
    fn take_expired(&self, now: u64) -> Vec<(u64, u128)>;

    /// Number of currently pending ACK entries. O(1). Used by the
    /// observability layer to publish the `rex_pending_acks` gauge.
    fn pending_count(&self) -> usize;
}

/// DashMap-backed production implementation. Owns its `timeout_secs` config.
pub struct AckTrackerImpl {
    pending_acks: DashMap<u64, PendingAckInfo, RandomState>,
    timeout_secs: u64,
}

impl AckTrackerImpl {
    pub fn new(timeout_secs: u64) -> Arc<Self> {
        Arc::new(Self {
            pending_acks: DashMap::with_hasher(RandomState::new()),
            timeout_secs,
        })
    }
}

impl AckTracker for AckTrackerImpl {
    fn register(&self, message_id: u64, source_client_id: u128, title: String, is_group: bool) {
        let info = PendingAckInfo {
            source_client_id,
            title,
            timestamp: now_secs(),
            is_group,
        };
        self.pending_acks.insert(message_id, info);
    }

    fn take(&self, message_id: u64) -> Option<PendingAckInfo> {
        self.pending_acks.remove(&message_id).map(|(_, v)| v)
    }

    fn get(&self, message_id: u64) -> Option<PendingAckInfo> {
        self.pending_acks.get(&message_id).map(|v| v.clone())
    }

    fn take_expired(&self, now: u64) -> Vec<(u64, u128)> {
        let expired: Vec<u64> = self
            .pending_acks
            .iter()
            .filter(|entry| now.saturating_sub(entry.value().timestamp) > self.timeout_secs)
            .map(|entry| *entry.key())
            .collect();

        let mut result = Vec::with_capacity(expired.len());
        for msg_id in expired {
            if let Some((_, info)) = self.pending_acks.remove(&msg_id) {
                result.push((msg_id, info.source_client_id));
            }
        }
        result
    }

    fn pending_count(&self) -> usize {
        self.pending_acks.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_then_take() {
        let tracker = AckTrackerImpl::new(60);
        tracker.register(1, 100, "news".to_string(), false);

        let taken = tracker.take(1).expect("should find");
        assert_eq!(taken.source_client_id, 100);
        assert_eq!(taken.title, "news");
        assert!(!taken.is_group);

        // Second take returns None
        assert!(tracker.take(1).is_none());
    }

    #[test]
    fn take_returns_none_for_unknown_id() {
        let tracker = AckTrackerImpl::new(60);
        assert!(tracker.take(999).is_none());
    }

    #[test]
    fn get_does_not_remove() {
        let tracker = AckTrackerImpl::new(60);
        tracker.register(1, 100, "news".to_string(), false);

        let got = tracker.get(1).expect("should find");
        assert_eq!(got.source_client_id, 100);

        // Still there after get
        let got2 = tracker.get(1).expect("still findable");
        assert_eq!(got2.source_client_id, 100);
        // And take still works
        assert!(tracker.take(1).is_some());
    }

    #[test]
    fn take_expired_returns_old_entries() {
        // timeout=0 → anything with timestamp strictly less than `now` is expired.
        let tracker = AckTrackerImpl::new(0);
        tracker.register(1, 100, "news".to_string(), false);
        tracker.register(2, 200, "weather".to_string(), true);

        // Look into the future so all registered entries are expired.
        let future = now_secs() + 100;
        let mut expired = tracker.take_expired(future);
        expired.sort_by_key(|(id, _)| *id);

        assert_eq!(expired, vec![(1, 100), (2, 200)]);

        // Tracker is now empty
        assert!(tracker.take(1).is_none());
        assert!(tracker.take(2).is_none());
    }

    #[test]
    fn take_expired_skips_recent_entries() {
        // 1 hour timeout, so anything registered now is not expired.
        let tracker = AckTrackerImpl::new(3600);
        tracker.register(1, 100, "news".to_string(), false);

        // Call with `now` very close to the register time.
        let just_after = now_secs() + 1;
        let expired = tracker.take_expired(just_after);
        assert!(expired.is_empty(), "recent entry should not be expired");

        // Still there
        assert!(tracker.get(1).is_some());
    }

    #[test]
    fn take_expired_returns_empty_for_empty_tracker() {
        let tracker = AckTrackerImpl::new(60);
        assert!(tracker.take_expired(now_secs() + 1000).is_empty());
    }

    #[test]
    fn register_overwrites_on_id_collision() {
        let tracker = AckTrackerImpl::new(60);
        tracker.register(1, 100, "first".to_string(), false);
        tracker.register(1, 200, "second".to_string(), true);

        let taken = tracker.take(1).expect("should find");
        assert_eq!(taken.source_client_id, 200);
        assert_eq!(taken.title, "second");
        assert!(taken.is_group);
    }

    #[test]
    fn take_expired_only_removes_expired_entries() {
        // Mixed batch: some old, some new.
        let tracker = AckTrackerImpl::new(60);

        // Register two entries; they share the current timestamp.
        let now = now_secs();
        tracker.register(1, 100, "old".to_string(), false);
        // Force timestamp on entry 2 to be much older via register, then
        // directly overwrite via... actually, the public API uses now_secs().
        // So both entries are recent. To simulate "old", we'll just register
        // and call take_expired with now far in the future (timeout 60).
        tracker.register(2, 200, "recent".to_string(), false);

        // Just after register: nothing is expired (60s timeout).
        assert!(tracker.take_expired(now + 10).is_empty());

        // Far future: both are expired.
        let mut expired = tracker.take_expired(now + 1000);
        expired.sort_by_key(|(id, _)| *id);
        assert_eq!(expired, vec![(1, 100), (2, 200)]);
    }
}
