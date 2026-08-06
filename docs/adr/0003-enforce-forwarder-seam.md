# ADR-0003: Enforce the Forwarder seam — close the last cross-node wire-I/O bypasses

- **Status:** Accepted
- **Date:** 2026-07-17 (implementation closed 2026-08)
- **Context:** Five sites that touched cross-node wire I/O directly despite `Forwarder` already owning it.
- **Related:** [ADR-0002: Extract `Forwarder` port from `ClusterPort`](./0002-extract-forwarder-from-cluster-port.md), [`CONTEXT.md`](../../CONTEXT.md) — `Forwarder` / `ClusterPort` glossary.

## Context

ADR-0002 created `Forwarder` and migrated the cross-node send (`forward`) to it. Two follow-on commits tightened the trait: `PeerRejected` was removed as unreachable (`refactor(forwarder): remove unreachable FwdResult::PeerRejected variant`), and the test for direct-send was strengthened to wire a second peer so the fallback walk had a real candidate (`test(forwarder): direct-send test wires second peer to assert fallback is bypassed`).

Despite the trait existing and being wired into `Services`, **five sites bypassed it** and reached for wire-level I/O directly:

| File | Bypass | Why it's wrong |
|------|--------|----------------|
| `crates/rex-server/src/handler/title.rs:51-101` | `cluster.forward_message(&node, req)` + hand-rolled fallback loop over `cluster.get_nodes()` | Duplicates `Forwarder::forward`'s built-in direct-send + fallback walk. The `bool` return hides *why* the first attempt failed, so the loop retries nodes it already knows are down. |
| `crates/rex-server/src/cluster/forward_relay.rs` (180 LOC) | Local fan-out + manual `ForwardAck` broadcast | Duplicates `Forwarder::deliver`. The ack broadcast directly violates `CONTEXT.md`'s *"ACKs are not broadcast from inside deliver; the dispatch loop reads the outcome and decides"* — `forward_relay` has no dispatch loop, so the rule is broken by construction. |
| `crates/rex-server/src/cluster/server_cluster.rs::forward_message` (~80 LOC) | Wire send + retry | Byte-for-byte duplicates `Forwarder::try_send` and `try_reconnect_and_send`. |
| `crates/rex-server/src/cluster/server_cluster.rs::broadcast` (~60 LOC, two signatures) | `()`-returning inherent version + `usize`-returning trait impl that hard-codes `1` | Same name, two signatures, two paths to wire I/O. |
| `crates/rex-server/src/handler/session_mutator.rs:91` | `cluster.broadcast(msg)` | Wire I/O routed through the wrong port. |

The architectural intent — *one port owns cross-node wire I/O* — was clear but unenforced. `CONTEXT.md` already recorded the anti-pattern: *"Don't add wire-level methods back to `ClusterPort` 'for convenience.'"* But the convenience pressure was real: `ServerClusterManager` was already a god-struct holding both membership state and the cluster transport; reaching for it was the path of least resistance.

## Decision

**All cross-node wire I/O flows through `Forwarder`.** `ServerClusterManager` no longer exposes `forward_message`, `broadcast`, or `try_reconnect_and_send`. `handler/title.rs`, `session_mutator.rs`, and the dispatch loop in `ServerClusterManager::handle_messages` all consume `services.forwarder.*` exclusively.

Concretely:

1. **`handler/title.rs::fallback-loop` → `services.forwarder.forward(req)`.** The whole 50-line block shrinks to a single `match FwdResult { ... }`. The handler stops knowing the names of other cluster nodes — it asks the forwarder, which owns that knowledge.
2. **`cluster/forward_relay.rs` → deleted.** Its fan-out path moved into `Forwarder::deliver` (already there); its inbound `ForwardAck` broadcast moved to `Forwarder::announce_ack`. The dispatch loop in `ServerClusterManager::handle_messages` calls `announce_ack` when `DeliveryOutcome::acked_back` is true, fulfilling the `CONTEXT.md` contract.
3. **`ServerClusterManager::forward_message` / `try_reconnect_and_send` / `broadcast` → deleted.** `NetworkForwarder::forward` is now the only writer to the cluster transport for outbound messages.
4. **`session_mutator.rs::broadcast(msg)` → `services.forwarder.broadcast(msg)`.** Wire I/O for `TitleRegister` / `TitleUnregister` goes through the same port as everything else.
5. **`ClusterPort::forward_message` and `ClusterPort::broadcast` → not re-introduced.** The slimmed `ClusterPort` (5 sync methods, membership only) stays slim.

### What "all cross-node wire I/O" includes

- Outbound cross-node delivery (`forward`).
- Inbound relay to local subscribers (`deliver`).
- Cluster-internal fan-out for control messages (`broadcast` — `TitleRegister`, `TitleUnregister`).
- Cross-node ack back-out (`announce_ack`).

### Lifecycle (no change from ADR-0002)

`NetworkForwarder` continues to hold `Arc<ArcSwap<Option<Arc<NodeManager>>>>` populated by `ServerClusterManager::start`. The slot stays empty until cluster start, so unit tests construct the forwarder with no cluster — `forward` returns `PeerUnreachable("cluster-not-started")`.

## Consequences

### Wins

- **Single read path for forwarding bugs.** Bug in any cross-node send now lives in `forwarder/` (mod.rs / network.rs / tests.rs). The handler stops being the second place to grep.
- **Handler loses cluster knowledge.** `title.rs` no longer calls `cluster.get_nodes()` to walk fallback peers; it doesn't need to know how the cluster routes. That knowledge lives once, on `Forwarder`.
- **Test coverage doubled.** Pre-this-ADR, `cluster/forward_relay.rs` had 5 tests; `Forwarder::deliver` had 1. Post-ADR, `deliver` has 4 tests covering no-subscribers / unicast / broadcast / unknown-client paths, and `forward` has 4 tests covering unstarted-cluster early-return / direct-send-no-fallback / fallback-walk / ack-fan-out. The integration tests (`crates/rex-test/tests/cluster_test.rs`) exercise the seam end-to-end.
- **Slimmer `ClusterPort`.** The 5-method membership-only port is the result. `CONTEXT.md`'s `ClusterPort` glossary entry matches reality.
- **Drops two duplicate signatures.** `ServerClusterManager::broadcast` no longer has both a `()` inherent version and a `usize`-returning trait impl.

### Costs

- **`ServerClusterManager::handle_messages` now reaches into `Services.forwarder` directly.** That coupling was already present via `services.cluster.broadcast(...)` (now `services.forwarder.broadcast(...)`); the seam enforcement does not add it, only redirects it.
- **One wider `Forwarder` trait surface.** `announce_ack` was added in this ADR (was previously in `forward_relay.rs`). Kept the trait cohesion: outbound + inbound relay + ack-back-out all share the `NodeManager` transport slot.
- **No new external dependencies.** All changes are within `rex-server` / `rex-cluster`.

### Non-decisions

- We did **not** further split `Forwarder` into `OutboundForwarder` + `InboundRelay`. They share the `NodeManager` transport and the local registry; one trait keeps that cohesion. Revisit when either side gains a second consumer.
- We did **not** introduce a generic `MessageHandler<M>` for `ClusterMessage` variants. The dispatch loop has 5 arms today (Join, NodeList, Heartbeat, Forward, ForwardAck, TitleRegister, TitleUnregister); the seam-enforcement removed one (`ForwardAck` was the only forward_relay-driven arm) and made the remaining ones read off `services.*` ports. Worth re-evaluating only when the count crosses ~10.
- We did **not** move `announce_ack` into `deliver`. `CONTEXT.md` records the explicit rule that `deliver` does not broadcast; the dispatch loop owns that decision based on `DeliveryOutcome::acked_back`. Keeping the rule enforced via the type system, not a comment.

## Alternatives considered

- **Document the rule, leave the code alone.** Rejected. The rule was already in `CONTEXT.md` and was being broken. Documentation does not stop bypasses; port-boundary enforcement does.
- **Force `Forwarder` to know about `ClusterPort`'s membership state.** Rejected: inverts the dependency. `Forwarder` already reads the route table via `Arc<GlobalRouteTable>` (owned by `ServerClusterManager`) and the transport via `Arc<NodeManager>` (also owned there). It does not need `ClusterPort` directly.
- **Make `ServerClusterManager` itself a `Forwarder` adapter.** Rejected: would force the dispatch loop to go through a self-reference for the ack-broadcast path. `CONTEXT.md` records why the current shape — Forwarder as a separate port owned by `Services`, not by `ServerClusterManager` — was chosen.
- **One PR instead of nine plan tasks.** Rejected: each task verified build + tests + clippy clean before the next. The nine-task incremental plan is documented in `.superpowers/sdd/progress.md` (Enforce Forwarder seam section).
- **Delete `Forwarder` and put the dispatch loop's wire I/O back on `ServerClusterManager`.** Rejected: the dispatch loop has 5 arms but only one of them (`Forward`) touches wire I/O. Embedding wire I/O in a dispatch loop makes the loop untestable without spinning up a cluster transport — exactly the testability we gained by extracting `Forwarder`.

## Followups (recorded, not done)

- `ServerClusterManager::handle_messages` is still ~130 LOC of match-arm dispatch. Candidate for further deepening if a 6th `ClusterMessage` arm appears or the per-arm bodies grow past ~30 LOC.
- The remaining `failover` / `gossip` / `sync` modules in `rex-cluster` are speculative scaffolding (Raft-style election + state sync). They have zero callers today. See first-wave cleanup notes in the project planning.
