# RexMq - Follow-up Roadmap

> Living document for sequencing work after the 2026-08 dev cycle.
> Anchored to evidence in the working tree; revisit whenever a phase
> completes or the workspace health numbers drift.

## 0. Baseline (verified 2026-08-11)

- Branch: `dev`, HEAD `56b18ec`, working tree clean.
- Tests: `cargo test --workspace` -> **201 passed, 0 failed**
  (33 test result lines: 166 lib + 35 integration).
- Lints: `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Workspace lints `unwrap_used` / `expect_used` / `panic` denied.
- ADRs on file: 0001 (split RexSystem into ports), 0002 (extract
  Forwarder from ClusterPort), 0003 (enforce Forwarder seam).
- Open GitHub issues: #11 (C3 split RexSystem), #12 (C4 Forwarder
  extraction). Both have their work landed on `dev`; the issues
  themselves were never closed (see Phase 1.2).

## 1. Phase 1 - Close out the current cycle (small, safe)

Goal: finish what is already on `dev`, clear the bookkeeping backlog,
and start the next phase from a versioned, documented baseline. No
new architectural moves.

### 1.1 Cut `0.5.0` from the current `dev` HEAD

- Move the three items under `CHANGELOG.md` `[Unreleased]` into a
  dated `[0.5.0]` section:
  1. Cluster crate slimmed (Raft-era scaffolding deleted).
  2. `Forwarder` module split into `mod` / `network` / `tests`.
  3. ADR-0003 committed.
- Tag `0.5.0` (annotated). No new code; this is bookkeeping.

### 1.2 Close stale GitHub issues

- **#11** - *Refactor: split RexSystem into ports (C3)*. The work
  landed across the ClientState Restoration plan
  (`docs/superpowers/specs/2026-07-16-client-state-restoration-design.md`),
  ADR-0001 (`docs/adr/0001-split-rex-system-into-ports.md`), and the
  `Services` / `Janitor` introductions. Comment with the ADR +
  representative commits (`49de459`, `37a5b68`, `dca6b15`,
  `9079a19`), then close.
- **#12** - *Refactor: extract Forwarder port from ClusterPort
  (ADR-0002 / C4)*. Landed across the Forwarder seam enforcement
  (`docs/superpowers/specs/2026-07-17-enforce-forwarder-seam-design.md`)
  + ADR-0002 + ADR-0003. Comment with the ADR + commit range
  (`96bfd2d` ... `b7859e6`), then close.

### 1.3 Reclaim the `bindings/rex4j` interop test

- **Discrepancy**: `CHANGELOG.md` [0.4.0] claims
  `bindings/rex4j/tests/interop.rs` runs end-to-end in CI, but
  `git ls-tree HEAD bindings/rex4j/tests/` shows no `interop.rs`.
  The commit recorded in `.superpowers/sdd/progress.md` as
  `0886c9d` exists in the object store but is not reachable from
  any branch on the dev lineage (`git branch -a --contains 0886c9d`
  is empty). This is a documentation/reality drift.
- **Action**: either cherry-pick `0886c9d` onto `dev` (and verify it
  builds with the current `rex4j` API) or remove the rex4j interop
  line from the [0.4.0] changelog. Do not leave the CHANGELOG
  asserting coverage that does not exist.

### 1.4 Sweep the recorded Minor findings

From `.superpowers/sdd/progress.md` follow-up ledger:

- **M7** - *stale `#[allow(dead_code)]` on `cluster_port.rs:26`*:
  resolved out-of-band by commit `1ebd2e0` (chore: remove stale
  dead-code suppressions + 2 dead items). `grep -rn 'allow(dead_code)' crates/`
  now returns zero hits; no further action.
- **M5** - *T5 review was skipped due to subagent quota*: budget
  reviewer slots separately from implementer slots in the next
  multi-PR refactor. Process note only; no code change.
- **M1-M4, M6** - deferred. M1 (lost ack error string) needs a
  trigger event (ack-driven observability becoming a requirement).
  M2 (`127.0.0.1:1` stub) is a test-harness convenience. M3
  (metric-label cardinality) was subsumed by F3's commit note.
  M4 (JoinHandle drop in tests) is a CI-trace improvement. None
  block the next phase.

### 1.5 Stale feature branches - triage or retire

Unmerged branches as of HEAD `56b18ec`:

| Branch | Last commit | State vs `dev` | Disposition |
|---|---|---|---|
| `feature/metrics-and-timing-wheel` | `b34e01a` rustls fix for QUIC | `DelayedMessageScheduler` + `TimingWheel` integrate at startup (`38c4777`) | rebase + run integration tests; **highest value** of the four |
| `feature/message-pool` | `56ffa9a` message buffer pool | pool module added, integration with publish path unclear | rebase + benchmark against baseline before merging |
| `feat/local` | `7a959bb` wire dead metrics, fix token leak | observability wiring + criterion bench scaffold | rebase + verify `<5% p99 overhead` claim actually measured |
| `rkyv` | `f8fdc1a` perf | speculative performance work, no correctness story | **retire** unless rebased onto current dev and benchmarked |

Each branch should be rebased onto the post-1.1 `dev` HEAD, then
either merged or closed with a one-line note. Do not carry stale
branches forward.

### 1.6 Verify

- `cargo test --workspace` -> 201+ passed.
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `git tag -l '0.5*'` shows the new tag.
- `gh issue list --state open` shows no project-planning issues.

---

## 2. Phase 2 - Architectural deepening (medium risk)

Goal: continue the sequence started by ADR-0001 / 0002 / 0003.
Each candidate gets its own spec + plan + SDD ledger entry; none
ship without going through the same per-step verification model
(commits leave the workspace green at every step).

### 2.1 C1 - Command dispatch table

The architecture-review sequence lists C1 next after C3 / C4 land
(both done). The handlers (`cast`, `group`, `title`, `login`,
`check`, `del_title`, `reg_title`, `ack`) all depend on `&Services`
post-C3, which makes them easier to put behind a single dispatch
table than before. Spec, design, six-to-nine-task rollout mirroring
the Forwarder seam enforcement.

**Why first**: removes the last handler-shaped duplication; opens
the door to per-command metrics, per-command timeouts, and per-
command middleware (auth, rate limit).

**Spec shape** (preliminary): `docs/superpowers/specs/2026-MM-DD-command-dispatch-table-design.md`. Each task <= one PR.

### 2.2 C2 - One cluster manager, not three

`ServerClusterManager` + `ClusterIntegration` + the (now-dead)
`ClusterManager` were always flagged for unification. The cluster
crate slim in 0.5.0 cleared out a lot, but the duplication remains.
Investigate whether C2 is now small enough to fold directly into C1
or whether it needs its own plan.

### 2.3 C6 - `TestServer` over `open_server`

Move `rex-test`'s factory from constructing subsystems by hand to
calling `open_server` against a test config. Depends on C1 (so
`open_server` exposes a clean `Services` shape) and on the config
cascade (already shipped in 0.2.0).

**Why**: cuts the cost of every future integration test by ~30%
and lets new ports drop in without re-wiring the factory.

### 2.4 C5 - `ConnectionDriver` pattern

Transport-side deepening. Depends on `ServerBase` shrinking enough
to make the driver pattern visible (a side-effect of C1 + C3).
Defer until after 2.3 lands.

---

## 3. Phase 3 - Hardening (longer horizon)

Goal: turn the in-repo infrastructure into something safe to run
in production without a paper trail of "we should add a test for
this" follow-ups.

### 3.1 Protocol fuzzing

- Add `cargo-fuzz` workspace (currently absent).
- Targets: `RexCommand` decode in `rex-core::data`, `RexData` decode
  in `rex-client`, `ClusterMessage` decode in `rex-cluster`.
- Run a short fuzz campaign on CI nightly; gate the build on
  "zero crashes in 60s".

### 3.2 Multi-server integration tests

Move from single-process `rex-test` to launching two or more
`rex-server` processes in a `rex-test/tests/cluster_e2e.rs` style.
Exercises the cluster wire path the new `ClusterTransport` tests
only cover piece-wise.

### 3.3 Performance regression baseline

The 2026-04 baseline report
(`docs/superpowers/specs/2026-04-22-performance-optimization-design.md`)
is now stale relative to the seam enforcement. Re-run the baseline
scenarios (TCP / QUIC / WebSocket; Rust / Java / Python clients;
1024-byte payload; TPS + latency) and publish a 2026-08 baseline
that all future perf work references. Then add a CI check that
flags any benchmark that regresses > 10% from that baseline.

### 3.4 Observability completion

- Surface ack-driven observability if M1's lost ack error string
  ever becomes load-bearing.
- Add `p99` SLO dashboards for the dispatch path post-C1.
- Wire `criterion` benchmark output to a tracked JSON so the
  3.3 regression check has data to compare against.

---

## 4. Out-of-scope (deliberately deferred)

- **Replace `arc-swap` with `tokio::sync::RwLock`** in `NetworkForwarder`:
  tracked in ADR-0002 followups; no urgency.
- **Generic per-variant handlers in the cluster dispatch loop**:
  candidate E from the architecture review. Revisit when a third
  `ClusterMessage` variant wants first-class handling.
- **Decompose `cast` / `group`** to use `Router` directly: they
  currently call `ClientRegistry::find_all_by_title` and never
  cross node. Until cross-node fan-out is a requirement, the
  extra indirection is dead weight.

---

## 5. Sequencing rationale

- Phase 1 is **bookkeeping** that costs < 1 day and unblocks
  Phase 2 by retiring stale branches and closing the issue tracker.
- Phase 2 picks up the existing architecture-review queue in the
  order it was written (C1 before C2 before C6 before C5). Each
  candidate depends on the previous one's seams.
- Phase 3 starts once Phase 2 produces a `Services` + `open_server`
  shape stable enough to test against the wire.

If the next demand shifts (e.g. a production incident, a new
client language), interrupt the sequence at the current phase
boundary and file the demand as its own plan in
`docs/superpowers/plans/`.
