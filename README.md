# slot-graph

Slot Graph is a lightweight, embeddable typed slot graph for dependency-driven synchronous and asynchronous task execution.

Version 0.5.1 provides graph editing, immutable compilation snapshots, inline
sync/async execution, cancellation, reusable runners, and optional external
Ready-node dispatch. The observable contract is described in
[the design document](documents/design.md).

## Examples

The numbered Rust files in `examples/` each describe one usage scenario, starting
with synchronous data flow and progressing to version editing, cancellation and
automatic connections. They are compiled by `cargo test --all-targets` and can
also be run directly with `cargo run --example <name>`.

| Example | Scenario |
| --- | --- |
| [01_basic_sync.rs](examples/01_basic_sync.rs) | Connect and run synchronous nodes |
| [02_multiple_outputs.rs](examples/02_multiple_outputs.rs) | Commit a complete set of outputs |
| [03_async_node.rs](examples/03_async_node.rs) | Mix synchronous and asynchronous work |
| [04_optional_many.rs](examples/04_optional_many.rs) | Optional inputs and ordered fan-in |
| [05_run_inputs.rs](examples/05_run_inputs.rs) | Supply external inputs for each run |
| [06_target_output.rs](examples/06_target_output.rs) | Read and take an Active target output |
| [07_transient_resources.rs](examples/07_transient_resources.rs) | Pass transient resource handles |
| [08_select_active.rs](examples/08_select_active.rs) | Choose the execution target |
| [09_version_edit.rs](examples/09_version_edit.rs) | Edit the graph while retaining an old version |
| [10_local_send.rs](examples/10_local_send.rs) | Choose Local or SendMode |
| [11_runner.rs](examples/11_runner.rs) | Reuse a runner across frames |
| [12_failure.rs](examples/12_failure.rs) | Inspect failures and independent successes |
| [13_cancellation.rs](examples/13_cancellation.rs) | Cancel a pending run |
| [14_long_lived_resources.rs](examples/14_long_lived_resources.rs) | Read host-owned resources on demand |
| [15_auto_connect.rs](examples/15_auto_connect.rs) | Connect nodes by name and type |
| [16_manual_schema.rs](examples/16_manual_schema.rs) | Explicit Slot identities and Many auto-collection |
| [17_replace_schema.rs](examples/17_replace_schema.rs) | Preserve compatible connections across Schema changes |
| [18_replace_task.rs](examples/18_replace_task.rs) | Replace task behavior while retaining an old version |
| [19_concurrent_runs.rs](examples/19_concurrent_runs.rs) | Drive independent inputs through concurrent runs |
| [20_external_optional_many.rs](examples/20_external_optional_many.rs) | Supply Optional and Many external inputs |
| [21_abort_and_reuse.rs](examples/21_abort_and_reuse.rs) | Abort pending work, then reuse the runner |
| [22_manual_driver.rs](examples/22_manual_driver.rs) | Drive a GraphRun with a poll loop and Waker |
| [23_shared_fan_out.rs](examples/23_shared_fan_out.rs) | Share and forward a non-Clone payload |
| [24_structured_errors.rs](examples/24_structured_errors.rs) | Recover from edit, compile, and startup errors |
| [25_bound_task_io.rs](examples/25_bound_task_io.rs) | Resolve typed input/output keys before task execution |
| [26_collect_into.rs](examples/26_collect_into.rs) | Collect explicit source nodes into one Many input |
| [27_renderer_pipeline.rs](examples/27_renderer_pipeline.rs) | Combine seven render/UI nodes across two frames |
| [28_external_dispatch.rs](examples/28_external_dispatch.rs) | Dispatch independent Send nodes through a host-owned pool |

Named task I/O remains convenient; BoundSchema keys provide a separate path
designed to avoid runtime name/hash lookup. This is not an allocation-free or
measured performance claim. `collect_into(sources, target_input)` adds ordinary
edges only to the selected Many input; it does not introduce runtime discovery,
flatten Vec values, or replace `connect_nodes` and its `auto_collect` flag.

`execute_on(inputs, dispatcher)` is an optional node-level dispatch boundary for
`SendMode`. It lets a host pool run independent ready nodes concurrently without
making that pool a core dependency. The default `execute` path remains inline
and host-driven; spawning only a whole `GraphRun` does not itself create
node-level parallelism. See [28_external_dispatch.rs](examples/28_external_dispatch.rs).

## Source layout

`src/lib.rs` contains crate documentation, module declarations, and common API
re-exports. Definitions live in the corresponding responsibility modules:

```text
mode.rs      Local / SendMode and admissible value types
handles.rs   Graph/Slot identities and layout-scoped task keys
schema.rs    Ordered declarations, builders, and immutable bound layouts
value.rs     Shared ownership and task input/output bags
task.rs      Repeatable task factories and per-task context
graph.rs     Mutable declarations, edits, connections, compile entry
compiled.rs  Immutable execution versions
runtime.rs   RunInputs, GraphRun, runner, and cancellation controls
report.rs    Final node states, failures, and retained target outputs
error.rs     Structured error kinds and diagnostic context
macros.rs    schema! and outputs! declaration conveniences
```

Every module has English Rustdoc. Public documentation is checked by
`#![deny(missing_docs)]` to keep the API surface readable as it grows.

## Verification

```text
cargo check --all-targets --locked
cargo test --all-targets --locked
cargo test --doc --locked
cargo +1.71.0 test --all-targets --locked
```

Behavioral contract tests exercise graph edits, compilation, execution,
cancellation, version isolation, and dispatch. See [the test guide](tests/README.md)
for the contract matrix.

## Performance baseline

The benchmark suite covers graph editing and compilation, Local and SendMode
sync/async execution, reusable runners, external dispatch, async-runtime
integration, and allocation counts. The reproducible procedure and acceptance
gates are in [the benchmark plan](documents/bench-plan.md); recorded results are
under [documents/performance](documents/performance/).

The core has no third-party dependencies. `futures-lite` is used by examples
and tests; Criterion and `async-runtime` are benchmark-only development
dependencies. The core does not own an executor.
