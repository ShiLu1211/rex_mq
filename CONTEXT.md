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
titles are owned by which node, which peers are known. Pure state about this
node only; nothing here drives a wire send.
"Cross-node wire I/O lives on [[Forwarder]]; this port is membership only — see ADR-0003."
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

## Observability

The server exposes Prometheus metrics, structured tracing, health probes,
and an admin HTTP surface on `:9090` via the `rex-observability` crate
(2026-07-06). See
[`docs/superpowers/specs/2026-07-06-observability-framework-design.md`](superpowers/specs/2026-07-06-observability-framework-design.md)
for the full design.

**ObservabilityHandle**:
The handle `open_server` returns alongside the transports. It owns the
admin HTTP server, the shared `HealthRegistry`, and the `Shutdown`
integration; dropping it (or calling its `shutdown` method) stops the
HTTP server.
_Avoid_: admin server, metrics server

**HealthRegistry**:
Aggregator for `HealthProbe` adapters. Public routes are aggregated into
`/readyz` with status 200 (Healthy/Degraded) or 503 (Unhealthy). Uses
`parking_lot::Mutex<Vec<Arc<dyn HealthProbe>>>` so probes can be
registered after construction.

**RegistryObsAdapter / ClusterAdapter / PersistenceAdapter / ForwarderAdapter**:
Production adapters that implement `rex-observability::probe::traits::*`,
calling into the corresponding `Services` fields. The adapters are the
bridge that lets `rex-observability` observe `rex-server` without depending
on it.
_Avoid_: probe adapter, default snapshot

## 埋点守则

**必埋的点**：
- `handler/title.rs`、`cast.rs`、`group.rs` 进入 publish 路径时（counter + histogram）
- 每条 `Forwarder::forward` 调用的结果（counter with reason label）
- `transport/{tcp,quic,websocket}.rs` 每次 send / receive 完成（bytes counter）
- `cluster/server_cluster.rs` 节点加入时（peer gauge）

**Label 命名规范**：
- `title` 允许（业务可控、基数有限）
- `transport` 允许（值域 = {tcp, quic, websocket}）
- `target` 允许（值域 = {local, remote}）
- `reason` 允许（值域 = {unreachable, rejected, no_peer, parse, timeout}）
- **`client_id` / `msg_id` 禁止作为 label**（爆 cardinality）
- 自定义 label 必须先在 PR 描述里说明基数上界

**直方图桶**：
- 延迟类指标使用 `LATENCY_BUCKETS = [0.0001, 0.0005, 0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0]`
- 任何修改桶的 PR 必须附 criterion bench 报告

## Configuration

**RexConfig**: Single root for the server's static configuration in `crates/rex-config`. Loaded by `rex_config::Loader` from a 4-layer cascade (defaults → TOML → env → CLI → validate). `RexSystemConfig` and `RexServerConfig` in `rex-server` are projections (`From<&RexConfig>`); they no longer own defaults.
_Avoid_: secondary config types, per-subsystem defaults straying from `RexConfig::default()`.

**Loader**: `rex_config::Loader::load() -> Result<RexConfig, ConfigError>`. Owns the file-discovery cascade (CLI `--config` → env `REX_CONFIG` → `./rex.toml` → `./config/rex.toml` → `/etc/rex/rex.toml`), the env parser (`REX__SECTION__KEY`), and the fail-fast validator. `ConfigError` carries section.key paths for grep-able boot failures.
_Avoid_: silent fallback to defaults, `Option` defaults inside subsystems.

**TOML section vocabulary**:
- `[server]` — identity + cross-cutting lifecycle
- `[[endpoints]]` — per-listener transport
- `[cluster]` — top-level cluster membership
- `[persistence]` / `[persistence.offline]` — sled + offline queue
- `[ack]` — application-level acknowledgement
- `[observability]` — admin HTTP + tracing

## Anti-patterns recorded here

- Adding wire-level methods (`forward_message`, `broadcast`, `send_to`, …) back to
  `ClusterPort` or `ServerClusterManager`. Two earlier anti-patterns (ADR-0002,
  ADR-0003) called this out; the third instance (recorded in PR 2 of the
  2026-07 architecture review) is now deleted.
- Treating one broadcast signal as the right shape — the C4 split moved cluster I/O
  away from [[ClusterPort]] specifically so [[Forwarder]] could own a single I/O seam.
  Don't add wire-level methods back to [[ClusterPort]] "for convenience."
- Returning `bool` from cross-node sends — that was the bug class the
  [[FwdResult]] enum was invented to fix. Always prefer the structured outcome.
- Adding埋点 calls in hot paths without a `#[allow(dead_code)]` or test
  to keep them in the call graph — observability that nobody reads is
  worse than no observability, but observability that breaks the build
  is worse than that. Either wire it to a real handler or remove it.
- Creating a new `Box<dyn Trait>` in a hot path to satisfy a port's
  `Send + Sync` bound without checking whether a reference works — the
  `AssertUnwindSafe` pattern from [[health.rs|HealthRegistry::check_all]]
  shows how to opt out of `UnwindSafe` cleanly.
- Constructing `RexSystemConfig::new(...11 args)` directly — deprecated;
  use `RexSystemConfig::from(&RexConfig::default())` or load from TOML.
- Hardcoding a duplicate default in a subsystem — one source of truth:
  `RexConfig::default()`. Mirroring it elsewhere invites drift.
- Using `unwrap()` / `expect()` in config loading paths — workspace
  lints deny them; the loader uses `ConfigError` and explicit match arms.
  `#[allow(clippy::unwrap_used)]` on binary crates is a deliberate
  exception for test-only paths and static-address constructors.
