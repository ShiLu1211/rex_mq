//! Adapters that bridge the in-crate system ports to the narrow
//! observability-side traits in `rex_observability::probe::traits`.
//!
//! rex-observability must not depend on rex-server (it lives below the
//! dependency line — see the crate-level docs in `rex-observability`).
//! So the trait surfaces it consumes are local mirrors declared in
//! `probe::traits`, and rex-server provides the concrete impls.

use std::collections::HashSet;
use std::sync::Arc;

use rex_observability::probe::traits::{
    ClientCancel as ObsClientCancel, ClientSummary, RegistrySnapshot as ObsRegistrySnapshot,
};

use crate::system::client_registry::ClientRegistry;
use crate::system::services::Services;

/// Bridge from the rex-server [`ClientRegistry`] port to the
/// rex-observability [`ObsRegistrySnapshot`] trait.
///
/// Takes the trait object (`Arc<dyn ClientRegistry>`) so it can be
/// wired straight from `Services::registry` in `open_server`. Title
/// count is derived from `list_snapshots()` — slightly more
/// expensive than reading `ClientRegistryImpl::title_count()` but
/// keeps the adapter free of concrete-type plumbing.
pub struct RegistryObsAdapter(pub Arc<dyn ClientRegistry>);

impl ObsRegistrySnapshot for RegistryObsAdapter {
    fn client_count(&self) -> usize {
        self.0.list_snapshots().len()
    }

    fn title_count(&self) -> usize {
        let mut titles = HashSet::new();
        for snap in self.0.list_snapshots() {
            for t in snap.titles {
                titles.insert(t);
            }
        }
        titles.len()
    }

    fn max_clients(&self) -> usize {
        0
    }

    fn list_clients(&self) -> Vec<ClientSummary> {
        self.0
            .list_snapshots()
            .into_iter()
            .map(|s| ClientSummary {
                id: s.id,
                transport: s.transport,
                titles: s.titles,
                connected_secs: s.connected_secs,
            })
            .collect()
    }
}

/// Bridge from the observability `ClientCancel` trait to
/// `Services::cancel_client`. Wraps a shared `Services` so the admin
/// router can invoke per-client cancellation without depending on
/// rex-server directly.
pub struct ClientCancelAdapter(pub Arc<Services>);

impl ObsClientCancel for ClientCancelAdapter {
    fn cancel(&self, id: u128) -> bool {
        self.0.cancel_client(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handler::test_util::dummy_client_with_id;
    use crate::system::client_registry::ClientRegistryImpl;

    fn make_adapter() -> (Arc<ClientRegistryImpl>, RegistryObsAdapter) {
        let reg = ClientRegistryImpl::new();
        let adapter = RegistryObsAdapter(reg.clone());
        (reg, adapter)
    }

    #[test]
    fn adapter_mirrors_empty_registry() {
        let (_reg, adapter) = make_adapter();
        assert_eq!(adapter.client_count(), 0);
        assert_eq!(adapter.title_count(), 0);
        assert!(adapter.list_clients().is_empty());
    }

    #[test]
    fn adapter_reflects_added_clients_and_titles() {
        let (reg, adapter) = make_adapter();
        let a = dummy_client_with_id(0xAA);
        let b = dummy_client_with_id(0xBB);
        reg.add_client(a.clone());
        reg.add_client(b.clone());
        reg.register_title(0xAA, "news");
        reg.register_title(0xBB, "weather");

        assert_eq!(adapter.client_count(), 2);
        assert_eq!(adapter.title_count(), 2);
        let clients = adapter.list_clients();
        let ids: std::collections::HashSet<u128> = clients.iter().map(|c| c.id).collect();
        assert!(ids.contains(&0xAA));
        assert!(ids.contains(&0xBB));
    }
}
