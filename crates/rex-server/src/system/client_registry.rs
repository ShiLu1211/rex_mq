//! Client registry port.
//!
//! Tracks the live set of clients and their title subscriptions. The trait is
//! intentionally **synchronous** — only in-memory map operations live here.
//! Cross-cutting concerns (cluster handshake, persistence, connection close)
//! belong to other ports and are invoked by callers, not by the registry.
//!
//! In this commit the trait and a real DashMap-backed `ClientRegistryImpl`
//! exist as a parallel construct next to `RexSystem`. `RexSystem` continues
//! to own its own maps; no caller uses `ClientRegistry` yet. Commit 8 of the
//! refactor plan deletes `RexSystem` and makes `ClientRegistryImpl` the
//! sole owner.

use std::sync::Arc;

use ahash::RandomState;
use dashmap::DashMap;
use rand::seq::IteratorRandom;
use rex_core::RexClientInner;

/// Read-only snapshot of a connected client for admin / metrics use.
///
/// Returned by [`ClientRegistry::list_snapshots`] and
/// [`ClientRegistry::get_snapshot`]. Plain data — no `Arc` to the live
/// client, so the caller can serialise / inspect after the client has
/// disconnected without lifetime concerns.
#[derive(Clone, Debug)]
pub struct ClientSnapshot {
    pub id: u128,
    pub transport: String,
    pub titles: Vec<String>,
    pub connected_secs: u64,
}

/// Tracks which clients are connected, and which titles each is subscribed to.
///
/// `remove_client` returns the removed client so the caller can close its
/// connection and persist its removal — those side-effects are not the
/// registry's job.
#[allow(dead_code)] // Port added in commit 2; consumed in commit 7+.
pub trait ClientRegistry: Send + Sync {
    fn add_client(&self, client: Arc<RexClientInner>);

    /// Remove a client from both the id map and every title map. Returns the
    /// removed client (or `None` if no such client existed) so the caller can
    /// close the connection.
    fn remove_client(&self, client_id: u128) -> Option<Arc<RexClientInner>>;

    /// Subscribe a connected client to a title. No-op if the client is not
    /// currently registered.
    fn register_title(&self, client_id: u128, title: &str);

    /// Unsubscribe a connected client from a title. No-op if the client is
    /// not currently registered or the title is unknown.
    fn unregister_title(&self, client_id: u128, title: &str);

    /// All currently connected clients.
    fn find_all(&self) -> Vec<Arc<RexClientInner>>;

    /// All clients subscribed to `title`, optionally excluding one client id
    /// (typically the sender).
    fn find_all_by_title(&self, title: &str, exclude: Option<u128>) -> Vec<Arc<RexClientInner>>;

    /// One random client subscribed to `title`, optionally excluding one
    /// client id (typically the sender).
    fn find_one_by_title(&self, title: &str, exclude: Option<u128>) -> Option<Arc<RexClientInner>>;

    /// Look up a client by its id.
    fn find_some_by_id(&self, id: u128) -> Option<Arc<RexClientInner>>;

    /// Return ids of clients whose last-received timestamp is older than
    /// `timeout_secs`. Does **not** remove them — the caller is expected to
    /// follow up with `remove_client` for each id returned. Pure state query,
    /// no side-effects.
    fn take_inactive(&self, timeout_secs: u64) -> Vec<u128>;

    /// Snapshot of every currently connected client. Order is unspecified;
    /// callers that need a stable view should sort the result by `id`.
    fn list_snapshots(&self) -> Vec<ClientSnapshot>;

    /// Snapshot of a single client, or `None` if no client with that id is
    /// currently connected.
    fn get_snapshot(&self, id: u128) -> Option<ClientSnapshot>;

    /// Number of currently connected clients. O(1). Used by the
    /// observability layer to publish the `rex_clients_connected`
    /// gauge after add/remove.
    fn client_count(&self) -> usize;

    /// Number of distinct titles with at least one subscriber. O(1).
    /// Used by the observability layer to publish the `rex_titles_active`
    /// gauge.
    fn title_count(&self) -> usize;
}

/// DashMap-backed production implementation.
#[allow(dead_code)] // Port added in commit 2; consumed in commit 7+.
pub struct ClientRegistryImpl {
    id2client: DashMap<u128, Arc<RexClientInner>, RandomState>,
    title2clients: DashMap<String, Vec<Arc<RexClientInner>>, RandomState>,
}

impl ClientRegistryImpl {
    #[allow(dead_code)] // Used in tests; production caller lands in commit 7+.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            id2client: DashMap::with_hasher(RandomState::new()),
            title2clients: DashMap::with_hasher(RandomState::new()),
        })
    }

    /// Number of distinct titles currently in the title map. Thin accessor
    /// for the observability adapter.
    pub fn title_count(&self) -> usize {
        self.title2clients.len()
    }
}

impl ClientRegistry for ClientRegistryImpl {
    fn add_client(&self, client: Arc<RexClientInner>) {
        let id = client.id();
        self.id2client.insert(id, client.clone());

        for title in client.title_iter() {
            let mut clients = self.title2clients.entry(title).or_default();
            if !clients.iter().any(|c| c.id() == id) {
                clients.push(client.clone());
            }
        }
    }

    fn remove_client(&self, client_id: u128) -> Option<Arc<RexClientInner>> {
        let (_, client) = self.id2client.remove(&client_id)?;

        for title in client.title_iter() {
            if let Some(mut clients) = self.title2clients.get_mut(&title) {
                clients.retain(|c| c.id() != client_id);
                if clients.is_empty() {
                    drop(clients);
                    self.title2clients.remove(&title);
                }
            }
        }

        Some(client)
    }

    fn register_title(&self, client_id: u128, title: &str) {
        let Some(client) = self.id2client.get(&client_id) else {
            return;
        };

        client.insert_title(title);

        let mut clients = self.title2clients.entry(title.to_string()).or_default();
        if !clients.iter().any(|c| c.id() == client_id) {
            clients.push(client.clone());
        }
    }

    fn unregister_title(&self, client_id: u128, title: &str) {
        let Some(client) = self.id2client.get(&client_id) else {
            return;
        };

        client.remove_title(title);

        if let Some(mut clients) = self.title2clients.get_mut(title) {
            clients.retain(|c| c.id() != client_id);
            if clients.is_empty() {
                drop(clients);
                self.title2clients.remove(title);
            }
        }
    }

    fn find_all(&self) -> Vec<Arc<RexClientInner>> {
        self.id2client
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }

    fn find_all_by_title(&self, title: &str, exclude: Option<u128>) -> Vec<Arc<RexClientInner>> {
        let Some(clients) = self.title2clients.get(title) else {
            return Vec::new();
        };

        clients
            .iter()
            .filter(|c| exclude != Some(c.id()))
            .cloned()
            .collect()
    }

    fn find_one_by_title(&self, title: &str, exclude: Option<u128>) -> Option<Arc<RexClientInner>> {
        let clients = self.title2clients.get(title)?;
        let mut rng = rand::rng();

        clients
            .iter()
            .filter(|c| exclude != Some(c.id()))
            .choose(&mut rng)
            .cloned()
    }

    fn find_some_by_id(&self, id: u128) -> Option<Arc<RexClientInner>> {
        self.id2client.get(&id).as_deref().cloned()
    }

    fn take_inactive(&self, timeout_secs: u64) -> Vec<u128> {
        use rex_core::utils::now_secs;
        let now = now_secs();
        self.id2client
            .iter()
            .filter(|entry| now.saturating_sub(entry.value().last_recv()) > timeout_secs)
            .map(|entry| *entry.key())
            .collect()
    }

    fn list_snapshots(&self) -> Vec<ClientSnapshot> {
        use rex_core::utils::now_secs;
        let now = now_secs();
        self.id2client
            .iter()
            .map(|entry| {
                let c = entry.value();
                ClientSnapshot {
                    id: c.id(),
                    transport: c.transport_label(),
                    titles: c.subscribed_titles(),
                    connected_secs: now.saturating_sub(c.connected_at()),
                }
            })
            .collect()
    }

    fn client_count(&self) -> usize {
        self.id2client.len()
    }

    fn title_count(&self) -> usize {
        self.title2clients.len()
    }

    fn get_snapshot(&self, id: u128) -> Option<ClientSnapshot> {
        use rex_core::utils::now_secs;
        let c = self.id2client.get(&id)?;
        let now = now_secs();
        Some(ClientSnapshot {
            id: c.id(),
            transport: c.transport_label(),
            titles: c.subscribed_titles(),
            connected_secs: now.saturating_sub(c.connected_at()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rex_core::{RexSenderTrait, utils::new_uuid};
    use std::net::{Ipv4Addr, SocketAddr};

    /// Trivial no-op sender for unit tests. The registry tests only exercise
    /// id, title_iter, insert_title, remove_title — never send_buf or close.
    struct NoopSender;

    #[async_trait::async_trait]
    impl RexSenderTrait for NoopSender {
        async fn send_buf(&self, _buf: &[u8]) -> anyhow::Result<()> {
            Ok(())
        }
        async fn close(&self) -> anyhow::Result<()> {
            Ok(())
        }
    }

    fn dummy_client() -> Arc<RexClientInner> {
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
        Arc::new(RexClientInner::new(
            new_uuid(),
            addr,
            "",
            Arc::new(NoopSender) as Arc<dyn RexSenderTrait>,
        ))
    }

    #[test]
    fn add_then_find_by_id() {
        let reg = ClientRegistryImpl::new();
        let c = dummy_client();
        let id = c.id();
        reg.add_client(c.clone());

        let found = reg.find_some_by_id(id).expect("client should be findable");
        assert_eq!(found.id(), id);
    }

    #[test]
    fn add_then_remove_returns_client_and_clears_id_map() {
        let reg = ClientRegistryImpl::new();
        let c = dummy_client();
        let id = c.id();
        reg.add_client(c.clone());

        let removed = reg.remove_client(id).expect("remove returns the client");
        assert_eq!(removed.id(), id);
        assert!(reg.find_some_by_id(id).is_none());
    }

    #[test]
    fn remove_unknown_id_returns_none() {
        let reg = ClientRegistryImpl::new();
        assert!(reg.remove_client(0xDEAD).is_none());
    }

    #[test]
    fn register_title_then_find_by_title() {
        let reg = ClientRegistryImpl::new();
        let c = dummy_client();
        let id = c.id();
        reg.add_client(c.clone());

        reg.register_title(id, "news");

        let found = reg.find_one_by_title("news", None).expect("findable");
        assert_eq!(found.id(), id);
    }

    #[test]
    fn register_title_for_unknown_client_is_a_noop() {
        let reg = ClientRegistryImpl::new();
        reg.register_title(0xDEAD, "news");
        assert!(reg.find_one_by_title("news", None).is_none());
    }

    #[test]
    fn unregister_title_removes_from_title_map() {
        let reg = ClientRegistryImpl::new();
        let c = dummy_client();
        let id = c.id();
        reg.add_client(c.clone());
        reg.register_title(id, "news");

        reg.unregister_title(id, "news");
        assert!(reg.find_one_by_title("news", None).is_none());
        // client still in id map
        assert!(reg.find_some_by_id(id).is_some());
    }

    #[test]
    fn find_all_by_title_excludes_sender() {
        let reg = ClientRegistryImpl::new();
        let c1 = dummy_client();
        let c2 = dummy_client();
        let id1 = c1.id();
        let id2 = c2.id();
        reg.add_client(c1.clone());
        reg.add_client(c2.clone());
        reg.register_title(id1, "news");
        reg.register_title(id2, "news");

        let found = reg.find_all_by_title("news", Some(id1));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id(), id2);
    }

    #[test]
    fn find_one_by_title_returns_none_when_title_unknown() {
        let reg = ClientRegistryImpl::new();
        assert!(reg.find_one_by_title("missing", None).is_none());
    }

    #[test]
    fn find_all_returns_every_registered_client() {
        let reg = ClientRegistryImpl::new();
        let c1 = dummy_client();
        let c2 = dummy_client();
        reg.add_client(c1.clone());
        reg.add_client(c2.clone());

        let all = reg.find_all();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn add_client_with_existing_titles_populates_title_map() {
        // When add_client is called for a client that already has titles
        // (via prior register_title calls), the title map should be populated.
        let reg = ClientRegistryImpl::new();
        let c = dummy_client();
        let id = c.id();
        // Insert titles on the client before adding to the registry.
        c.insert_title("news");
        c.insert_title("weather");
        reg.add_client(c.clone());

        let news = reg.find_one_by_title("news", None).expect("news");
        assert_eq!(news.id(), id);
        let weather = reg.find_one_by_title("weather", None).expect("weather");
        assert_eq!(weather.id(), id);
    }

    #[test]
    fn take_inactive_returns_only_stale_clients() {
        let reg = ClientRegistryImpl::new();
        let c1 = dummy_client();
        let c2 = dummy_client();
        let id1 = c1.id();
        let id2 = c2.id();
        reg.add_client(c1.clone());
        reg.add_client(c2.clone());

        // Backdate c1 by 100 seconds so it is "stale" relative to a 10s timeout.
        // c2 keeps its current timestamp.
        use rex_core::utils::now_secs;
        c1.set_last_recv_for_test(now_secs().saturating_sub(100));
        c2.set_last_recv_for_test(now_secs());

        let stale = reg.take_inactive(10);
        assert_eq!(stale, vec![id1]);
        // c2 not in the list
        assert!(!stale.contains(&id2));
    }

    #[test]
    fn take_inactive_returns_empty_for_empty_registry() {
        let reg = ClientRegistryImpl::new();
        assert!(reg.take_inactive(0).is_empty());
    }

    #[test]
    fn take_inactive_does_not_remove() {
        let reg = ClientRegistryImpl::new();
        let c = dummy_client();
        let id = c.id();
        reg.add_client(c.clone());
        use rex_core::utils::now_secs;
        c.set_last_recv_for_test(now_secs().saturating_sub(100));

        let _ = reg.take_inactive(10);
        // Still in id map
        assert!(reg.find_some_by_id(id).is_some());
    }

    /// Build a client with a known id (mirrors the helper in
    /// `handler::test_util::dummy_client_with_id` but lives here so the
    /// registry tests don't depend on the handler test-util module).
    fn make_test_client(id: u128) -> Arc<RexClientInner> {
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
        Arc::new(RexClientInner::new(
            id,
            addr,
            "",
            Arc::new(NoopSender) as Arc<dyn RexSenderTrait>,
        ))
    }

    #[test]
    fn list_snapshots_returns_all_clients() {
        let reg = ClientRegistryImpl::new();
        let client_a = make_test_client(0xAA);
        let client_b = make_test_client(0xBB);
        reg.add_client(client_a.clone());
        reg.add_client(client_b.clone());

        let snaps = reg.list_snapshots();
        assert_eq!(snaps.len(), 2);
        let ids: std::collections::HashSet<u128> = snaps.iter().map(|s| s.id).collect();
        assert!(ids.contains(&0xAA));
        assert!(ids.contains(&0xBB));

        let single = reg.get_snapshot(0xAA).expect("found");
        assert_eq!(single.id, 0xAA);
        assert!(reg.get_snapshot(0xDEAD).is_none());
    }
}
