# Contract tests

This crate is currently an API skeleton. The ignored tests are executable
specifications with real calls and assertions; they are compiled on stable and
Rust 1.71 but do not demonstrate working graph execution yet.

| File | Main contracts |
| --- | --- |
| `api_compile.rs` | Mode bounds, Shared ownership, typed handles, user error sources |
| `graph_contract.rs` | Build/edit validation, Active closure, auto-connect, atomic Schema replacement |
| `runtime_contract.rs` | External inputs, reports, failure, cooperative cancellation, abort, runner reuse |
| `async_contract.rs` | Controlled Pending Futures, wake behavior, run isolation, drop and abort |
| `output_contract.rs` | Output validation/atomicity, report errors/ownership, panic, ordered fan-in |
| `version_contract.rs` | Immutable snapshots, task/Schema changes, external-key compatibility |
| `bound_contract.rs` | Task-key layouts, input shape, mixed output writes, replacement conveniences |
| `collect_contract.rs` | Explicit source scope, one target, ordering, deduplication, atomic rollback |
| `dispatch_contract.rs` | Node-level overlap, dependency gates, failures, ordering, and stale jobs |

Rustdoc also checks rejected Send values/factories/Futures, Local controls,
repeatable factories, and exclusive runner borrowing using `compile_fail`.

Once an implementation satisfies a contract, remove that test's ignore marker.
Controlled asynchronous tests use bounded poll loops for intermediate states;
completion is driven only after the test releases a gate or requests abort.
No test should depend on the completion order of independent nodes.

Activate contracts in implementation-sized vertical slices: graph edits and
compile, synchronous execution and atomic outputs, async polling and wakes,
then cancellation, runner reuse, and version/layout interactions. A feature is
not implemented merely because its ignored tests compile. Its contracts must
run in default CI before the feature is claimed complete. Do not remove ignore
markers before the corresponding engine behavior exists; the complete v0.3
release gate must not leave claimed behavior covered only by ignored tests.

Collection tests verify values and order, not just edge counts. Bound-key tests
separate layout mismatch from shape mismatch and preserve the existing atomic
output checks when named and keyed writes address the same slot. Performance
claims additionally need benchmarks; passing these contracts does not establish
lookup costs, allocation counts, or throughput.

External-dispatch contracts use bounded gates and timeouts for adversarial
interleavings. They require independent Send nodes to overlap, while keeping
dependency unlock, cancellation/commit races, Many order, failure reports, and
runner generations under GraphRun's control. Local dispatch is owner-thread-only
and has no multi-core guarantee.

This is the initial behavioral matrix, not a coverage percentage or the v0.3
release gate. Benchmark baselines, platform coverage, and additional adversarial
interleavings remain part of implementation and release work.
