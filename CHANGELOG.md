# Changelog

All notable changes to RexMQ. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/) — though pre-1.0 the
versioning is pragmatic: anything in `0.y.0` is a development cycle; minor bumps may include
breaking changes.

## [Unreleased]

### Changed (in progress)

- **Cluster crate slimmed: Raft-era scaffolding deleted.** `crates/rex-cluster/src/failover.rs`, `gossip.rs`, and `sync.rs` are gone — they had zero callers anywhere in the workspace. `NodeManager` lost its `term` / `voted_for` / `votes_received` / `role` fields plus `start_election` / `handle_request_vote` / `handle_vote_response` / `check_election_timeout`. `ClusterMessage` no longer carries `RequestVote` / `VoteResponse` / `AppendEntries` / `AppendEntriesResponse` / `StateSyncRequest` / `StateSyncResponse`. `ClusterRole` is gone (only `Standalone` was ever initialised; that branch was the lone path). `HeartbeatMessage` lost its `term` and `leader_commit` fields (always 0, never read). `NodeInfo` lost `is_leader` / `with_leader` / `state` / `term` / `version`. `ClusterConfig` lost `election_timeout_min_ms` / `election_timeout_max_ms`. Net: ~1,000 LOC of speculative scaffolding removed; the cluster crate now carries only what `NodeManager` + `Forwarder` + `ServerClusterManager` actually consume.
- **`Forwarder` module split.** `crates/rex-server/src/system/forwarder.rs` (930 LOC) is now `crates/rex-server/src/system/forwarder/{mod,network,tests}.rs`. `mod.rs` carries the types (`FwdResult`, `DeliveryOutcome`), the `Forwarder` trait, and `NetworkForwarder`'s slot management. `network.rs` carries the `Forwarder` impl plus the private `try_send` / `try_reconnect_and_send` helpers. `tests.rs` carries all 14 unit tests and their `drain_listener` / `recording_listener` / `started_forwarder` fixtures. No behavioural change.
- **ADR-0003 committed.** `docs/adr/0003-enforce-forwarder-seam.md` records the decision that closed the last five cross-node wire-I/O bypass sites during the 2026-07 Forwarder-seam enforcement (handler/title.rs hand-rolled fallback loop, cluster/forward_relay.rs duplicate fan-out, ServerClusterManager::forward_message, ServerClusterManager::broadcast, session_mutator.rs::broadcast).

## [0.4.0] — 2026-07-20 — Bindings cross-language interop

### Added

- **Python bindings (`bindings/rex4p`).** PyO3 extension exposing `RexClient` / `RexData` / `RexCommand` / `RexHandler` / `RexConfig` to Python. `examples/rex_engine.py` is a working `snd` / `rcv` / `bench` driver. The crate's `build.rs` emits `.so` (Linux) or `.dylib` (macOS) named `rex4p.so`.
- **Java bindings (`bindings/rex4j`).** JNI extension exposing `RexClient` / `RexData` / `RexCommand` / `RexHandler` / `RexConfig` to Java. `examples/RexEngine.java` is the corresponding driver. `build.rs` invokes `mvn dependency:build-classpath` to assemble the runtime classpath. Maven `spotbugs:check` runs in CI.

### Verified

- `bindings/rex4p/tests/interop.rs` and `bindings/rex4j/tests/interop.rs` start a real `rex-server` (TCP listener on `127.0.0.1`) and run end-to-end publish / receive through the binding under test. Both pass in CI.

### Spec / plan

- `docs/superpowers/specs/2026-07-20-bindings-cross-language-interop-test-design.md`

## [0.3.0] — 2026-07-17 — Forwarder seam enforcement

### Changed

- **`Forwarder` is now the only port that touches cross-node wire I/O.** Five sites that previously reached into `cluster.forward_message`, `cluster.broadcast`, or duplicated the fan-out in `cluster/forward_relay.rs` are migrated:
  - `handler/title.rs::fallback-loop` — 50 lines of hand-rolled multi-peer walk collapses to a `match FwdResult { ... }` against `services.forwarder.forward(req)`.
  - `cluster/forward_relay.rs` — deleted (180 LOC). Its fan-out moved into `Forwarder::deliver`; its inbound ack broadcast moved into a new `Forwarder::announce_ack` method, called by `ServerClusterManager::handle_messages` when `DeliveryOutcome::acked_back` is true (fulfilling the `CONTEXT.md` contract).
  - `ServerClusterManager::forward_message` / `try_reconnect_and_send` — deleted. `Forwarder::try_send` was already a byte-for-byte duplicate.
  - `ServerClusterManager::broadcast` — deleted (the `()` inherent version and the `usize`-returning trait impl that hard-coded `1` are both gone).
  - `handler/session_mutator.rs::broadcast(msg)` — now `services.forwarder.broadcast(msg)`.
- **`FwdResult::PeerRejected(String)` removed.** Whole-branch review found the variant unreachable after the seam enforcement landed; only `Delivered`, `NoPeerForTitle`, and `PeerUnreachable(String)` survive.
- **`NetworkForwarder` direct-send test hardened.** Previously, `forward_to_known_target_accepted_does_not_fallback` wired a single peer and asserted `Delivered`. That passed even if the fallback walk regressed (because the walk had no candidate to attempt). Now wires two peers — node-b as target, node-c as a fallback candidate — and asserts node-c receives nothing within 100ms, proving the fallback walk was never entered.

### Spec / plan

- `docs/adr/0003-enforce-forwarder-seam.md`
- `docs/superpowers/specs/2026-07-17-enforce-forwarder-seam-design.md`
- `.superpowers/sdd/progress.md` — "Enforce Forwarder seam" section (9 tasks, 10 commits, 2026-07-17 → 2026-08)

## [0.2.0] — 2026-07-17 — rex.toml configurability

### Added

- **`crates/rex-config` workspace member.** Single source of truth for server configuration. `RexConfig` is the root; subsystems (server / persistence / cluster / ack / observability) are projections via `From<&RexConfig>`.
- **4-layer configuration cascade: `defaults → rex.toml → env (REX__SECTION__KEY) → CLI → validate`.** Each layer overrides the previous; the validator runs after merge. Unknown TOML keys cause boot to exit with code 78 (`EX_CONFIG`) — typos surface as boot failures rather than silent defaults.
- **`rex_config::Loader::load() -> Result<RexConfig, ConfigError>`.** Owns file-discovery (CLI `--config` → env `REX_CONFIG` → `./rex.toml` → `./config/rex.toml` → `/etc/rex/rex.toml`), env parsing, and the fail-fast validator. `ConfigError` carries section.key paths for grep-able failures.
- **`--print-default-config` / `--print-effective-config` CLI flags.** First prints `RexConfig::default()` (all defaults applied); second merges TOML + env + CLI and prints the result. Both are wired through `clap` derive.
- **Workspace lints.** `unwrap_used` / `expect_used` / `panic` are `deny` workspace-wide; `clippy.toml` allows them in tests. The config-loading paths follow this without exception.

### Migration

- `RexSystemConfig::new(...11 args)` deprecated — use `RexSystemConfig::from(&RexConfig::default())` or load from TOML.
- `Cargo.toml` dependency bumps: `tokio` 1.52.3 → 1.53.1, `futures` 0.3.32 → 0.3.33, `async-trait` 0.1.89 → 0.1.91, `rustls` 0.23.41 → 0.23.43, `serde` 1.0.228 → 1.0.229, `toml` 1.1.2 → 1.1.4.

### Spec / plan

- `docs/superpowers/specs/2026-07-17-rex-toml-configurability-design.md`
- `.superpowers/sdd/progress.md` — "rex.toml Configurability" section (17 tasks, 17 commits)

## [0.1.0] — 2026-07-16 — Client state restoration

### Added

- **`ClientStateStore` port + sled-backed adapter.** Persists the `(client_id, titles, ghost_until)` triple at `remove_client` time; loads on startup to repopulate the registry as ghost entries so a reconnecting client resumes its titles without re-registering.
- **`ClientRegistry` ghost lifecycle.** `add_ghost` / `claim_ghost` / `remove_ghost` plus the existing `add_client` / `remove_client`. A live client always wins over a ghost with the same id. `ghost_ttl_secs` (default 86,400) bounds how long an unreclaimed ghost stays in memory.
- **`Janitor` ghost-GC loop.** Periodic sweep alongside the existing ACK-timeout and inactive-client sweeps. Uses `ClientStateStore::take_expired_ghosts(now)` and removes the expired entries both in-memory and on disk.
- **Offline-queue drain on login.** When a client reconnects after a session loss, `services.offline.get_offline_messages(client_id)` is called and the queued messages are pushed to the live client before the next user-driven publish. Drained entries are then cleared via `clear_offline_messages`.
- **Persistence file-lock fix.** `PersistenceStore`, `SledOfflineBuffer`, and `SledClientStateStore` now share a single `Arc<sled::Db>` (was: each adapter opened its own `sled::Db` on the same path → file-lock contention at startup).
- **Observability hooks.** `observe_client_state_save_latency` (histogram), `set_clients_connected` / `set_titles_active` (gauges) wired through the `Janitor`'s GC loops.

### Spec / plan

- `docs/superpowers/specs/2026-07-16-client-state-restoration-design.md`
- `.superpowers/sdd/progress.md` — "ClientState Restoration" section (11 tasks, 11 commits)

## [0.0.x] — 2026-04 to 2026-07 — Pre-config observability and ports

### Added (chronological)

- **2026-07-06 — Observability framework (`crates/rex-observability`).** Prometheus metrics + structured tracing + health probes (`HealthRegistry` aggregating `RegistryObsAdapter` / `ClusterAdapter` / `PersistenceAdapter` / `ForwarderAdapter`) + admin HTTP on `:9090`. Replaces the 2026-04 monitoring design with a cleaner 4-pillar layout.
- **2026-07-03 — `Forwarder` extracted from `ClusterPort` (ADR-0002).** `ClusterPort` slimmed to 5 sync methods; `Forwarder` (3 async methods, `forward` / `deliver` / `broadcast`) owns cross-node wire I/O. `FwdResult` replaces the old `bool`; `DeliveryOutcome` replaces the void-with-logs path on inbound relay.
- **2026-07-01 — `RexSystem` split into ports (ADR-0001).** `ClientRegistry` / `AckTracker` / `OfflineBuffer` / `ClusterPort` / `Shutdown`. Replaces the 491-LOC god-struct.
- **2026-04-23 — Message pool + `rkyv` zero-copy design (spec).** `crates/rex-core/src/utils.rs` and `protocol/data.rs` carry the framing — twelve `unsafe` blocks for byte-level header casts. Spec written; deferred integration until latency budgets demand it.
- **2026-04-22 — Performance optimization design + criterion benches.** `crates/rex-test/benches/data_codec.rs` and `observability_overhead.rs` are the wiring.
- **2026-04-09 — Monitoring design (superseded by 2026-07-06).** Original Prometheus + dashboard + JSON API spec; replaced when the 4-pillar framework was approved.

### Spec / plan index

- `docs/adr/0001-split-rex-system-into-ports.md`
- `docs/adr/0002-extract-forwarder-from-cluster-port.md`
- `docs/superpowers/specs/2026-04-09-monitoring-design.md` (superseded)
- `docs/superpowers/specs/2026-04-22-performance-optimization-design.md`
- `docs/superpowers/specs/2026-04-23-message-pool-design.md`
- `docs/superpowers/specs/2026-04-23-rkyv-zero-copy-design.md`
- `docs/superpowers/specs/2026-05-07-cross-language-sdk-callback-design.md`
- `docs/superpowers/specs/2026-07-06-observability-framework-design.md`
- `docs/superpowers/specs/2026-07-16-client-state-restoration-design.md`
- `docs/superpowers/specs/2026-07-17-enforce-forwarder-seam-design.md`
- `docs/superpowers/specs/2026-07-17-rex-toml-configurability-design.md`
- `docs/superpowers/specs/2026-07-20-bindings-cross-language-interop-test-design.md`

[Unreleased]: https://example.com/rexmq/compare/v0.4.0...HEAD
[0.4.0]: https://example.com/rexmq/compare/v0.3.0...v0.4.0
[0.3.0]: https://example.com/rexmq/compare/v0.2.0...v0.3.0
[0.2.0]: https://example.com/rexmq/compare/v0.1.0...v0.2.0
[0.1.0]: https://example.com/rexmq/releases/tag/v0.1.0
