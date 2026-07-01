# ADR-0001: Split RexSystem into ports (ClientRegistry, AckTracker, OfflineBuffer, ClusterPort)

- **Status:** Accepted
- **Date:** 2026-07-01
- **Context:** `rex-server/src/system/registry.rs` (491 LOC)
- **Related:** [`CONTEXT.md`](../../CONTEXT.md) (will reference once present), architecture review report `architecture-review-*.html`

## Context

`RexSystem` had grown into a god-struct carrying seven orthogonal concerns under one `Arc`:

1. `id2client` + `title2clients` — in-memory client registry
2. `pending_acks` — ACK timeout tracking
3. `persistence: Option<Arc<PersistenceStore>>` — sled-backed offline queue and client-state persistence
4. `cluster_manager: RwLock<Option<Arc<ServerClusterManager>>>` — the cluster handshake and route lookup
5. `shutdown_tx: Arc<broadcast::Sender<()>>` — broadcast signal
6. `config: RexSystemConfig` — system configuration
7. The background cleanup task (`cleanup_inactive_clients`, `cleanup_expired_acks`)

Every handler in `handler/*` reached into 3–4 of these fields per call. The persistence path was write-only in production — `save_client_state` ran on every `add_client`, but nothing read it back. `get_offline_messages` was only exercised in `persistence_test.rs`.

A second problem hid in the same neighborhood: there were **two parallel shutdown signals** — `RexSystem.shutdown_tx` (for the cleanup task) and `ServerBase.shutdown_tx` (for the transport loops). They never coordinated.

## Decision

Delete `RexSystem`. Replace it with four narrow port traits plus one cross-cutting shutdown port, wired by a `Services` bundle.

### The ports

| Port | Module | Sync? | Surface |
|------|--------|-------|---------|
| `ClientRegistry` | `system/registry.rs` (renamed) | sync | `add_client` / `remove_client` / `register_title` / `unregister_title` / `find_*` |
| `AckTracker` | `system/ack.rs` | sync | `register_pending_ack` / `take_pending_ack` / `take_expired(now)` |
| `OfflineBuffer` | `system/offline.rs` | **async** | `save_client` / `remove_client` / `queue_offline_message` / `get_offline_messages` / `clear_offline_messages` |
| `ClusterPort` | `system/cluster_port.rs` | sync | `register_client` / `unregister_client` / `find_node_for_title` / `get_local_node_id` / `get_nodes` / `forward_message` |
| `Shutdown` | `system/shutdown.rs` | sync | `subscribe()` / `signal()` |

### Cross-cutting modules

- **`Services`** (`system/services.rs`) — bundle struct: `pub struct Services { pub registry: Arc<dyn ClientRegistry>, pub acks: Arc<dyn AckTracker>, pub offline: Arc<dyn OfflineBuffer>, pub cluster: Arc<dyn ClusterPort>, pub shutdown: Arc<Shutdown> }`.
- **`Janitor`** (`system/janitor.rs`) — owns `Services` + a `Shutdown` receiver. Runs the periodic cleanup loop. Calls `acks.take_expired()` and `registry.take_inactive()`, then does the async sends via `ClientRegistry::find_some_by_id` and `RexClientInner::send_buf`.

### Dependency rules

- `ClientRegistry::new` takes `Arc<dyn ClusterPort>` and calls `cluster.register_client` / `cluster.unregister_client` inside `add_client` / `remove_client`. The cluster handshake is part of client lifecycle.
- `AckTracker` is **pure state** — it does not call into `ClientRegistry`. `take_expired(now)` returns `Vec<(msg_id, source_id)>` and the Janitor does the lookup + send.
- `OfflineBuffer` is **async only on this port** (sled is async). The other three ports stay sync — `async_trait` lives only on `OfflineBuffer`.
- `Shutdown` is held by `Janitor`, by every transport (TCP / QUIC / WebSocket), and by `AggregateServer::close`. **There is exactly one broadcast signal** for the whole server.

### Login wires into OfflineBuffer

`handler/login.rs` gains a real consumer for `OfflineBuffer`. On a brand-new client ID, after `registry.add_client(...)`, the handler calls `services.offline.get_offline_messages(client_id)`, sends each as a `Title` message, then calls `clear_offline_messages(client_id)`. This gives `OfflineBuffer` the second adapter the seam requires and completes a half-built feature.

### Tests

Each handler gets `#[cfg(test)] mod tests` with inline mocks — a `MockClientRegistry`, `MockAckTracker`, etc., ~30 LOC each. Port impls (the real DashMap-backed `ClientRegistry`, the real `AckTracker`) get their own impl-side tests in `system/registry.rs::tests` and `system/ack.rs::tests`. `rex-test/tests/` continues to cover E2E.

### Construction

`lib.rs::open_server` becomes the single wiring site:

```rust
let shutdown = Arc::new(Shutdown::new());
let cluster = Arc::new(ServerClusterManager::new(node_id, true));
let registry = Arc::new(ClientRegistryImpl::new(cluster.clone())) as Arc<dyn ClientRegistry>;
let acks = Arc::new(AckTrackerImpl::new(ack_timeout)) as Arc<dyn AckTracker>;
let offline: Arc<dyn OfflineBuffer> = match config.offline_path {
    Some(p) => Arc::new(SledOfflineBuffer::open(p).await?),
    None => Arc::new(NoopOfflineBuffer),
};
let services = Arc::new(Services { registry, acks, offline, cluster: cluster.clone(), shutdown: shutdown.clone() });
tokio::spawn(Janitor::run(services.clone(), shutdown.subscribe()));
let server = TcpServer::open(services.clone(), server_config).await?;
```

`ServerBase` shrinks to `{ services: Arc<Services>, config, semaphore }`.

## Consequences

### Wins

- **Locality.** Bug in ACK timeouts → AckTracker + Janitor. Bug in title routing → ClientRegistry. Bug in cluster handshake → ClientRegistry + ClusterPort mock. No more "find the god-struct."
- **Testability.** Handlers test against mocks; impl-side tests verify the DashMap logic. The two adapters rule is honored for every port.
- **Leverage.** One mock `ClientRegistry` is reused by all eight handlers. One mock `AckTracker` is reused by cast, group, title. The cost of adding a ninth handler drops sharply.
- **Real OfflineBuffer.** Offline replay becomes a working feature. The persistence crate earns its place.
- **One shutdown signal.** The two-parallel-signals bug is fixed by construction — there is no second `shutdown_tx` to keep in sync.

### Costs

- **Five traits to maintain.** Each port has a trait definition that the impl must satisfy. Rust doesn't have struct inheritance, so the trait list is the cost of the split. Mitigated by `#[automock]`-style derives if we adopt them later, or by hand-rolling tiny mocks for tests.
- **One more module per port.** `system/` grows from `{ config.rs, registry.rs }` to `{ config.rs, services.rs, registry.rs, ack.rs, offline.rs, cluster_port.rs, shutdown.rs, janitor.rs }`. Mitigated: each is <150 LOC.
- **`async_trait` on `OfflineBuffer`.** The crate is already in `Cargo.toml` (used by `RexServerTrait`). No new dependency.
- **`ServerBase` construction order changes.** Tests that build `ServerBase` directly need updating. Currently: only `lib.rs::open_server` and `aggregate/aggregate_server.rs` build it.

### Non-decisions

We deliberately did **not** split `ClusterPort` into `ClusterRegistry` (handshake) + `Router` (lookup) — that's the C4 candidate, separate deepening. Revisit when a second consumer of either sub-port appears.

We deliberately did **not** make `RexSystem` a thin coordinator that forwards to ports — that's the literal antithesis of the deepening. The whole point is the god-struct dies.

## Alternatives considered

- **Coordinator struct (`RexSystem` stays as forwarder).** Rejected: keeps the god-struct, gets none of the test gain.
- **Generic handlers (`<R: ClientRegistry>`).** Rejected: handler signatures explode; tests require building the full generic instantiation of every handler.
- **`async` on every port that touches I/O.** Rejected: `async_trait` is a footgun and `AckTracker` doesn't need it.
- **Delete persistence entirely (B for Q3).** Rejected: persistence already has a real shape; completing it is cheaper than re-deriving it.
- **Shutdown as a field on `Janitor`.** Rejected: transports depend on shutdown too; conflating cleanup with shutdown creates a circular dep.
- **Construction inside `ServerBase`.** Rejected: `lib.rs::open_server` is already the wiring site; moving construction into ServerBase hides it.

## Migration plan

1. Add `Shutdown` port + impl. Replace the two parallel signals. Wire transports. Compile + tests green.
2. Add `ClientRegistry` trait + `ClientRegistryImpl` (DashMap-backed). Re-export as `Arc<dyn ClientRegistry>`. Migrate handler callsites one at a time behind a feature. Drop `RexSystem.id2client` and `title2clients`.
3. Add `AckTracker` trait + impl. Move `pending_acks` out of `RexSystem`. Drop `take_expired` async-ness.
4. Add `Janitor` module. Port `cleanup_inactive_clients` and `cleanup_expired_acks` into `registry.take_inactive` / `acks.take_expired` + Janitor effects.
5. Add `OfflineBuffer` trait + impl. Wire `login` to drain.
6. Add `ClusterPort` trait. Migrate the existing `ServerClusterManager` to implement it.
7. Add `Services` bundle. Migrate `ServerBase.system` → `ServerBase.services`. Migrate `handler::handle` signature.
8. Delete `RexSystem`. Update `lib.rs::open_server` wiring.
9. Add per-handler `#[cfg(test)] mod tests` with inline mocks.

Each step compiles and tests pass before the next.
