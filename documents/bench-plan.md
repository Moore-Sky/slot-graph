# Benchmark and Optimization Plan

## Goals and acceptance gates

- Release `0.5.0` as the benchmark baseline for the existing implementation.
- Optimize the reusable 100-node frame workload first. Protect build, compile,
  large-graph, async, and dispatch performance from regressions.
- Accept a candidate only when three independent A/B rounds agree and either:
  - target latency improves by at least 5%, with Criterion's 99% confidence
    interval excluding zero; or
  - allocation calls or bytes fall by at least 10% without more than 2% latency
    regression.
- Reject any candidate that regresses a protected 100-node sync or async
  workload by more than 2%.
- Stop when every measured hotspot and shortlisted strategy has been tested or
  ruled out, with no remaining candidate meeting the acceptance gate.

Use a staged evaluation funnel; do not rerun the complete timing suite for
every candidate:

1. Run static analysis and the allocation probe first. Reject a candidate that
   cannot approach an acceptance threshold.
2. Run three A/B rounds only for the one to three workloads directly affected
   by the isolated change.
3. Run the smallest representative sync, async, and dispatch protection set.
4. Run the full correctness/MSRV gate only for an accepted version. Repeat the
   complete performance suite only for a new release baseline or when the
   change has broad enough impact that focused protection is insufficient.

## Baseline suite

Use these development-only dependencies:

```toml
criterion = { version = "=0.5.1", default-features = false, features = ["html_reports"] }
async-runtime = "=0.3.4"
futures-lite = "2.6.1"
clap = "=4.3.24" # Criterion transitive dependency constrained for Rust 1.71
half = "=2.4.1" # Ciborium transitive dependency constrained for Rust 1.71
```

Criterion async measurements call `futures_lite::future::block_on` inside each
iteration, avoiding Criterion's heavier async feature. Verify every benchmark
target with Rust 1.71.

Create shared graph and workload builders plus six benchmark targets:

- `graph_build`: chain, fan-out/fan-in, `connect_nodes`, `collect_into`, and
  schema replacement.
- `compile`: full and 10%-Active closures at 100, 1,000, and 10,000 nodes.
- `runtime_sync`: Local and SendMode, fresh and reusable runs, and named and
  bound-key I/O.
- `runtime_async`: immediately-ready futures, deterministic Pending/wake,
  wake storms, and 80/20 mixed sync/async pipelines.
- `dispatch`: serial chains and independent widths using a benchmark-local
  fixed worker pool with one and eight workers.
- `async_runtime_integration`: the same dispatch workloads through
  `async-runtime 0.3.4`, excluding runtime construction and shutdown from the
  timed region.

The primary workload is a reusable 100-node renderer-like frame graph. Scaling
workloads use 1,000 and 10,000 nodes. Every benchmark consumes outputs through
`black_box` and verifies correctness outside the timed region.

Add a separate allocation probe using a benchmark-local counting global
allocator. After warm-up, report allocation calls, gross requested bytes
(realloc counts its complete new request), peak live-byte delta from the
operation baseline, and end live-byte delta. Run allocation probes serially.
Dispatched probes keep one prewarmed runtime across samples and wait on a
benchmark-local in-flight guard before closing each counting window. Runtime
shutdown remains outside the window. Treat these as quiesced end-to-end global
allocation measurements: async-runtime 0.3.4 exposes no strict all-worker
allocator barrier, so scheduler-envelope finalization may still cross a sample
boundary.

## Measurement and baseline records

- Use Cargo's default optimized bench profile for timing and a separate
  release-derived profile with debug symbols for profiling.
- Capture the active Windows power-plan GUID, switch to High Performance for
  formal runs, and restore the original Balanced plan in a `finally` path.
- Record CPU, core count, memory, OS, target, rustc, Cargo, power plan, commands,
  sample settings, and source revision.
- Run Criterion with a fixed warm-up, 10-second measurement time, 100 samples,
  99% confidence, and 1% significance.
- Capture three independent Criterion processes with
  `--save-baseline v0.5.0-r1`, `v0.5.0-r2`, and `v0.5.0-r3`. The
  `--baseline` option is reserved for comparing a later run with an existing
  saved baseline. Generated HTML and JSON remain under the ignored `target/`
  directory.
- Run the non-Criterion `allocations` target separately and save its CSV output;
  do not pass Criterion baseline arguments to it.
- Commit English summaries under `documents/performance/`: an environment
  manifest, one report per accepted version, and an experiment log containing
  rejected strategies and their measured reasons.
- Use WPR/WPA with Criterion's profile-time mode to attribute CPU hotspots. Use
  the allocation probe for heap attribution.

## Optimization campaign

Test one independently attributable strategy at a time. Choose the highest
measured hotspot; use this order when hotspots are within two percentage points:

1. Add a direct internal synchronous-task path to avoid boxing and polling an
   immediately-ready Future.
2. Make `GraphRunner` reuse run buffers and capacities while preserving
   generation isolation.
3. Replace full-vector Future polling with wake-driven runnable tracking for
   Pending-heavy async runs.
4. Build compile-time incoming-edge and output indexes once, removing repeated
   whole-edge scans.
5. Compact small fan-in/fan-out and task-I/O collections. A/B test
   `smallvec 1.15.1` only after allocation evidence identifies these paths.
6. A/B test hash strategies:
   - keep untrusted node-name hashing randomized;
   - test `ahash 0.8.12` with randomized state;
   - test `rustc-hash 1.1.0` only for trusted integer IDs; and
   - isolate `hashbrown 0.16.1` table-layout effects from hasher effects.
7. For proven dispatch contention, separately A/B test `parking_lot 0.12.5`
   and `crossbeam-queue 0.3.13`, retaining lost-wake and cancellation
   guarantees.

Do not adopt `xhash`; the identified crate is unrelated and unsuitable. Defer
`indexmap`, slab or slotmap, `async-task`, and core `futures-lite` integration
unless later profiles establish a new hotspot. Every new core dependency
requires documented MSRV, license, maintenance, security, and binary or
dependency-cost justification.

## Version, API, and Git policy

- `0.5.0` adds the benchmark infrastructure, environment record, and initial
  baseline.
- Each accepted optimization receives the next `0.5.x` patch version and a
  local Git commit.
- Rejected code is removed without consuming a version. Its measurements remain
  in the experiment log.
- Do not push benchmark or optimization commits.
- Keep `0.5.x` backward-compatible. Internal changes and additive APIs are
  allowed; breaking API changes are documented first as a `0.6.0` proposal and
  are not implemented during this campaign.
- Before implementing a compatible public fast path, add an English
  `design.md` entry containing its version, exact outer signature, motivation,
  compatibility behavior, and unchanged existing path.
- Preserve ordering, atomic output commit, cancellation, failure propagation,
  ownership, Local/SendMode bounds, and executor neutrality.

Every accepted version must pass:

```text
cargo fmt --all -- --check
cargo test --all-targets --locked
cargo test --doc --locked
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo +1.71.0 test --all-targets --locked
cargo +1.71.0 test --doc --locked
cargo +1.71.0 bench --no-run --locked
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --locked
```

All 28 examples must also run successfully. Benchmark source, documentation,
reports, comments, and commit messages remain English.
