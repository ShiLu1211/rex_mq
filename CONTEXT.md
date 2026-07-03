# RexMq

A message-queue and pub/sub broker that speaks the Rex protocol over TCP, QUIC, and
WebSocket. Clients subscribe to **titles**; messages are routed to local subscribers
first and forwarded across **cluster nodes** when needed.

## Concepts

**Title**:
A named channel that a client subscribes to; messages published to a title reach
every subscriber on that title (locally first, then cluster peers).
_Avoid_: topic, channel, topic name

**Client**:
A connected endpoint, identified by a UUID; holds one or more title subscriptions.
_Avoid_: connection, peer, session (a session is broader than the registered client)

**Publisher**:
A client that sends a message to a title.
_Avoid_: producer, sender (overloaded with the transport's sender)

**Subscriber**:
A client that has registered to receive messages on a title.
_Avoid_: consumer, listener

**Cluster node**:
A server process running one instance of the broker; nodes exchange membership info
via gossip and route titles by consistent hash.
_Avoid_: peer (used only for the transport-level wire connection), broker, instance

**Acknowledgement**:
An application-level confirmation that a published message reached its target;
optional, gated by configuration.
_Avoid_: ACK (the noun is the project term)

## Architecture Ports

The server is wired as a bag of dependency-injected ports; each port has a single
production adapter and, where it adds leverage, a test adapter.

**ClientRegistry**:
The in-memory map that records which clients are connected and which titles each
client subscribes to. Pure state — no cluster handshake, no persistence.
_Avoid_: client store, session map, registry (the bare word is too generic)

**Router**:
Answers "where does this title route to?" — a local subscriber, a remote cluster
peer, or none. Local has priority over remote.
_Avoid_: dispatcher (Router = resolution; dispatcher = fan-out)

**Forwarder**:
Owns cross-node message delivery. Encapsulates the cluster transport wiring,
the retry-on-disconnect path, the known-peer fallback walk, and inbound peer-message
relay. Returns rich outcomes that distinguish *delivered* from *peer unreachable*
from *no peer for title*. The C4 deepening split this out of [[ClusterPort]]
when the second consumer ([[ForwardRelay]]) appeared.
_Avoid_: cluster I/O module, forward module, proxy, forwarder service

**ClusterPort**:
Membership state of the local node — which clients are registered here, what
titles are owned by which node, which peers are known. After the C4 split,
**ClusterPort** no longer carries wire I/O; those responsibilities moved to the
[[Forwarder]] port.
_Avoid_: cluster adapter (overloaded with the whole wiring), cluster manager

**OfflineBuffer**:
Per-client persistence of messages queued for offline subscribers; drained on login.
_Avoid_: offline queue, persistence buffer

**AckTracker**:
Pure state for pending acknowledgements; expired entries are returned for the
[[Janitor]] to time out.
_Avoid_: ack store, ACK state, pending ACK map

**Shutdown**:
A single broadcast signal shared by every long-running task — transports, the
[[Janitor]], the cluster manager — so one signal stops the whole server.
_Avoid_: shutdown channel, stop signal, signal (the semantic is "one signal per server")

## Wiring

**Services**:
The bag every long-running task holds: an `Arc<dyn ...>` for each of [[ClientRegistry]],
[[AckTracker]], [[OfflineBuffer]], [[ClusterPort]], [[Router]], [[Forwarder]],
plus the [[Shutdown]] signal and config. Plus a small set of composite operations
(`add_client`, `remove_client`, `setup_message_ack`) that orchestrate multiple ports.
_Avoid_: container, registry, DI graph

**Janitor**:
A periodic task driven by [[Shutdown]] and a timer; uses [[AckTracker]] and
[[ClientRegistry]] to time out pending ACKs and inactive clients.
_Avoid_: cleaner, janitor task

## Outcome Types

**RoutePlan**:
What [[Router]] returns — `Local(client) | Remote(node) | None`.
_Avoid_: routing decision, route result

**FwdResult**:
What [[Forwarder::forward]] returns — `Delivered | NoPeerForTitle | PeerUnreachable(node) | PeerRejected(node)`.
Replaces the original `bool` that hid why a send failed.
_Avoid_: forward result, send status, bool

**DeliveryOutcome**:
What [[Forwarder::deliver]] returns — counts of local deliveries and failures
plus whether an acknowledgement went back. Replaces a void return whose outcome
could only be read from logs.
_Avoid_: delivery summary, deliver result, void

## Adapters Behind the Seams

These are the concrete adapters behind each port — recorded here as glossary
entries so future explorers know there is a real second (test) adapter.

**ClusterRouter**:
Production [[Router]] implementation. Looks up the local subscriber first, then
falls back to the cluster route table.
_Avoid_: default router, RealRouter

**NetworkForwarder**:
Production [[Forwarder]] implementation. Holds a slot for the cluster's
`NodeManager` (populated when the cluster starts; reads return
`PeerUnreachable("cluster-not-started")` before then), the global route table,
and the local registry.
_Avoid_: RealForwarder, default forwarder

## Anti-patterns recorded here

- Treating one broadcast signal as the right shape — the C4 split moved cluster I/O
  away from [[ClusterPort]] specifically so [[Forwarder]] could own a single I/O seam.
  Don't add wire-level methods back to [[ClusterPort]] "for convenience."
- Returning `bool` from cross-node sends — that was the bug class the
  [[FwdResult]] enum was invented to fix. Always prefer the structured outcome.
