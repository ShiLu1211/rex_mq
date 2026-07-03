# ADR-0002: Extract `Forwarder` port from `ClusterPort` (close the C4 split)

- **Status:** Accepted
- **Date:** 2026-07-03
- **Context:** `rex-server/src/cluster/server_cluster.rs:572 LOC` carrying both cluster membership state **and** cross-node I/O.
- **Related:** [ADR-0001: Split RexSystem into ports](./0001-split-rex-system-into-ports.md), [`CONTEXT.md`](../../CONTEXT.md), architecture review report `architecture-review-*.html`.

## Context

ADR-0001 deliberately left `ClusterPort` as one wide port — six methods — and named the eventual split (`ClusterRegistry` + `Router`) as the C4 candidate, deferring it "until a second consumer of either sub-port appears."

Since then three things changed:

1. **`Router` was extracted** as a third port (`system/router.rs`, 170 LOC), shrinking `ClusterPort` to 7 methods. The C4 split was half-done.
2. **`cluster/forward_relay.rs` (180 LOC)** appeared — the inbound peer-message delivery path. It bypasses the `ClusterPort` port entirely and reaches into `Services` to call `services.cluster.broadcast(...)` for ACK back-out. **This is the second consumer** that justifies the rest of the C4 split: forwarding now has two sites that don't talk through any port.
3. **`ServerClusterManager` reached 572 LOC.** It owns NodeManager lifecycle, the `mpsc` dispatch loop with five `ClusterMessage` arms, reconnect-and-retry logic (`try_reconnect_and_send`), the `forward_message` wire send, the `broadcast` fan-out, and a pure-delegation `ClusterPort` impl (`server_cluster.rs:449–484`, ~36 lines of `fn name(...) { ClusterPort::name(self, ...) }`). A god-struct that earns its existence by deleting-ergo-spreads-complexity (passes the deletion test).

A second bug class hid in the same neighborhood: `handler/title.rs::fallback-loop` (lines 51–69) walks every known peer on `if !success`. The `bool` return hides *why* the first attempt failed, so the loop tries nodes it already knows are down.

## Decision

Split `ClusterPort` into two narrower ports: a slimmed **ClusterPort** (membership-only, 5 sync methods) and a new **Forwarder** port (cluster I/O, 3 async methods). Add `Forwarder` to the `Services` bundle. Trim `ServerClusterManager` to membership + gossip + the dispatch loop.

### The ports

| Port | Module | Surface | Sync? |
|------|--------|---------|-------|
| `ClusterPort` *(slimmed)* | `system/cluster_port.rs` | `register_client` / `unregister_client` / `find_node_for_title` / `get_local_node_id` / `get_nodes` | sync |
| `Forwarder` *(new)* | `system/forwarder.rs` | `forward` / `deliver` / `broadcast` | **async** (uses `NodeManager` transport) |

### Return types

**`FwdResult`** — replaces `bool` from the old `forward_message`:

```rust
pub enum FwdResult {
    Delivered,
    NoPeerForTitle,
    PeerUnreachable(String),   // node name; lets handler skip the failing one
    PeerRejected(String),
}
```

The variants expose the failure modes the title handler always wanted but couldn't see. `PeerUnreachable(String)` carries the failing node name so the fallback loop at `handler/title.rs:51–69` can skip it without re-asking `ClusterPort::get_nodes`.

**`DeliveryOutcome`** — replaces the void return on inbound relay:

```rust
pub struct DeliveryOutcome {
    pub delivered_to: usize,
    pub failed: usize,
    pub acked_back: bool,
}
```

Replaces `deliver_forward_message`'s void-with-log path; makes inbound delivery testable without grepping logs.

### Fallback placement

`forward` walks all known peers internally on `PeerUnreachable`, excluding the failing node and the local node. The caller calls one method and gets one result. The handler stops holding the multi-node walk.

### Lifecycle: `NodeManager` slot

`NetworkForwarder` holds an `Arc<ArcSwap<Option<Arc<NodeManager>>>>` so the slot is empty at construction and populated when `ServerClusterManager::start()` runs. Calls before population return `FwdResult::PeerUnreachable("cluster-not-started")`. This matches the existing `RwLock<Option<NodeManager>>` pattern in `server_cluster.rs` and lets unit tests construct a `NetworkForwarder` with a mock `NodeManager` slot without spinning up a cluster.

### Service and adapter layout

- **`Services`** gains `pub forwarder: Arc<dyn Forwarder>` next to the existing `cluster` and `router` fields.
- **`NetworkForwarder`** is the production adapter (wraps `Arc<NodeManager>`, `Arc<GlobalRouteTable>`, `NodeId`, `Arc<dyn ClientRegistry>`).
- **`MockForwarder`** lives in `handler/test_util.rs` alongside `TestClusterPort` — the existing location for second adapters, no file reorganisation.

## Consequences

### Wins

- **Locality.** Bug in forwarding → `forwarder.rs`. Bug in membership → `cluster_port.rs`. Bug in title fan-out → `router.rs`. No more "find the god-struct."
- **Testability.** `handler/title.rs::tests` gains the title-fallback path. Today it's untestable because `bool` and `NodeManager` together obscure intent; tomorrow `MockForwarder.next_forward_result` returns canned `FwdResult` values.
- **Real failure modes for the handler.** `PeerUnreachable(String)` lets the handler skip failing nodes by name. The current "try every other node" loop becomes a precise skip-then-continue.
- **Leverage.** One `MockForwarder` is reused by handler tests, janitor tests, and any future inter-node-sending code. One `FwdResult` enum replaces `bool` at every send site.
- **Closes an open ADR.** ADR-0001's C4 split was deferred "until a second consumer appears." That consumer is `forward_relay.rs`.

### Costs

- **One more trait.** `Forwarder` adds a third cross-cutting port that the `Services` bag gains. Three trait definitions total (ClusterPort, Router, Forwarder) instead of two. Mitigated by `NetworkForwarder` being a single ~150 LOC module.
- **Two trait methods removed from `ClusterPort`.** `forward_message` and `broadcast` migrate. Tests for `ClusterPort` (in `server_cluster.rs`) slimmer; trait impl drops 36 LOC of pure delegation.
- **`async_trait` already in the crate.** `[email protected]` is a workspace dep. No new external dependencies.
- **`ServerClusterManager` shrinks but doesn't disappear.** The NodeManager lifecycle, route table, and dispatch loop remain. Total cut: from 572 LOC to roughly 350 LOC. The trait-wrapper impl goes from 36 LOC of pass-through to nothing.

### Non-decisions

- We did **not** split `Forwarder` further into `OutboundForwarder` + `InboundRelay` — they share the `NodeManager` transport and the local registry; one trait keeps that cohesion.
- We did **not** introduce a generic `MessageHandler<M>` for `ClusterMessage` variants — that's E in the architecture review, a separate speculative deepening. Revisit when the dispatch loop grows past five arms.
- We did **not** replace `forward_relay.rs` as a free function — it moves *into* `NetworkForwarder::deliver` so the receiver of an inbound `ForwardMessage` is the same type that owns outbound delivery.

## Alternatives considered

- **`Forwarder` = outbound only.** `deliver` and `broadcast` would stay on `ClusterPort`. Rejected: leaves two concerns (inbound delivery + broadcast) entangled with membership state; `forward_relay.rs` would still reach across `Services` to call `services.cluster.broadcast`.
- **`Forwarder` = forward + deliver, broadcast stays on ClusterPort.** Rejected: `broadcast` is wire-level I/O and belongs with the other wire I/O methods; splitting it across two traits splits the I/O surface.
- **`Result<FwdSuccess, FwdError>`.** Standard Rust idiom. Rejected: most outcomes aren't errors (`NoPeerForTitle` is a normal "no one subscribed cluster-side" condition). Forces callers to `?`-bubble where they want to branch.
- **Keep `bool`, add `last_failure: Mutex<Option<FwdResult>>` on Forwarder.** Rejected: leaks impl detail; races under parallel test runs; rule-of-thumb is "return the info, don't stash it."
- **Big-bang single-PR migration.** Rejected: harder to review; no per-step verification. ADR-0001's nine-step incremental plan is the model we follow.
- **Restructure construction order so `NodeManager` exists before `Forwarder`.** Rejected: makes the `lib.rs::build_services` order more coupled; harder to construct a `NetworkForwarder` in tests; less symmetric with the existing `RwLock<Option<NodeManager>>` pattern.

## Migration plan

Each step compiles and tests pass before the next.

1. **Add `Forwarder` trait** + `FwdResult` + `DeliveryOutcome` in `system/forwarder.rs`. Add `NetworkForwarder` impl that delegates to `ServerClusterManager::forward_message` etc. (forward compat layer). Add `MockForwarder` to `handler/test_util.rs`.
2. **Add `forwarder: Arc<dyn Forwarder>` to `Services`.** Wire it in `lib.rs::build_services`. Construct `NetworkForwarder` before `ServerClusterManager::start()`, hand it an empty `ArcSwap` slot.
3. **Migrate `handler/title.rs`.** Replace `services.cluster.forward_message(...)` with `services.forwarder.forward(req)`; replace the manual fallback loop with a `match` on `FwdResult`.
4. **Migrate `cluster/forward_relay.rs`.** Replace `deliver_forward_message` callsite with `services.forwarder.deliver(msg)`; ACK back-out goes through `self.broadcast(...)` inside `NetworkForwarder`.
5. **Migrate `session_mutator.rs` + remaining `services.cluster.broadcast` sites** to `services.forwarder.broadcast`.
6. **Trim `ClusterPort`.** Remove `forward_message` and `broadcast`. Update `NoopClusterPort`. Delete `ServerClusterManager::forward_message`, `try_reconnect_and_send`, `broadcast` — now unused. Delete the 36-LOC pure-delegation trait impl.

After step 6, `ServerClusterManager` carries only: NodeManager lifecycle (`new`, `start`, `set_services`, `local_node_id`, `is_enabled`), the mpsc dispatch loop with four remaining arms (`Join`, `NodeList`, `TitleRegister`, `TitleUnregister`, `ForwardAck`), and the slim `ClusterPort` impl.
