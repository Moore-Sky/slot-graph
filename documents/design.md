# Slot Graph Design

**Version: 0.3**

This is the authoritative 0.3 design. It defines observable behavior rather
than a mandated implementation.

The current crate is an API skeleton: examples and contract tests compile, but
graph editing, compilation, and scheduling remain explicitly unimplemented.
Ignored behavioral tests are specifications, not evidence of a working engine.

## Purpose

`slot-graph` compiles a runtime-typed Slot graph into an immutable execution
DAG. A run pushes successful results to successors and drives sync/async tasks
through Rust `Future`. It is for render preparation, transient-result pipelines,
resource preparation, data conversion, and small async dependency chains.

It is not a renderer, frame graph, resource manager, ECS scheduler, async
runtime, executor, thread pool, or durable workflow product.

```text
editable Graph -> compile -> immutable ExecutionGraphVersion -> GraphRun
```

The declaration graph remains editable. A compiled version and its runs are
immutable snapshots and are unaffected by later edits.

## Graph, schema, and values

Each node has a graph-unique name, stable `NodeId`, input Slots, output Slots,
and one sync or async task. Names are lookup aliases, not identity; rename keeps
the NodeId and compatible edges. Node, Slot, and Edge handles are graph-scoped.
Removal or Schema replacement must make old handles stale, never aliases of a
reused object. A handle from another graph returns `ForeignHandle`.

An edge is one output Slot to one input Slot and expresses both data flow and
execution dependency. Users do not separately write `depends_on`.

Inputs have direction-local unique non-empty name and Slot identity, runtime
type, presence, cardinality, and `auto_collect`. Outputs have identity, name,
and type. Connection identity includes node, direction, and Slot identity.

```rust
schema! {
    ("color": RenderTargetHandle,
     "depth": Optional<RenderTargetHandle>,
     "lights": Optional<Many<LightBuffer>>)
    -> ("result": RenderTargetHandle)
}
```

| Declaration | Meaning |
| --- | --- |
| `T` | Required + One |
| `Optional<T>` | Optional + One |
| `Many<T>` | Required + Many |
| `Optional<Many<T>>` | Optional + Many |

Outputs are single values in 0.3; fan-out uses several edges, never a Many
output. `schema!` is only convenience: ordinary Schema APIs have equal power,
and macros never connect, compile, schedule, or execute.

The core type contract is `SlotTypeId`, not a static generic graph universe. An
MVP may wrap `std::any::TypeId`, but must not claim TypeId is stable across
binaries, processes, languages, or serialization.

Slot values use shared ownership exposed as opaque `Shared<T, Mode>`:

```rust
inputs.required::<T>("input")?; // Shared<T, Mode>
inputs.optional::<T>("input")?; // Option<Shared<T, Mode>>
inputs.many::<T>("input")?;     // Vec<Shared<T, Mode>>
```

`Shared` is cloneable and read-only through `as_ref`/`Deref`; cloning shares
ownership rather than cloning `T`. Storage, reference count, and address are
not API. Local may use `Rc<dyn Any>` and Send may use `Arc<dyn Any + Send +
Sync>`. Values need not implement Clone, Copy, Default, Debug, or Serialize.

## Scenarios

Examples compile against the public skeleton. Until runtime implementation,
running them deliberately reaches an explicit unimplemented-runtime panic.

- [`01_basic_sync.rs`](../examples/01_basic_sync.rs): `produce.value` commits,
  then `consume.value` becomes ready. Ready sync tasks run inline.
- [`02_multiple_outputs.rs`](../examples/02_multiple_outputs.rs): three
  GBuffer bindings to Lighting are one node dependency; GBuffer commits all
  outputs and unlocks Lighting exactly once.
- [`03_async_node.rs`](../examples/03_async_node.rs): inputs can move into an
  async Future. Pending never unlocks successors; sync/async completion has
  identical output, failure, and cancellation semantics.
- [`04_optional_many.rs`](../examples/04_optional_many.rs): an absent optional
  resolves to None/empty; a connected optional is a dependency. Many waits for
  all connected producers and has deterministic binding order. Optional is not
  lazy.
- [`05_run_inputs.rs`](../examples/05_run_inputs.rs): an exposed input is a
  run-scoped source validated before task start. It cannot coexist with a
  producer. Required missing, duplicate, unexpected, cardinality, and type
  errors are StartErrors with no partial run. Many input uses ordered `extend`.
- [`06_target_output.rs`](../examples/06_target_output.rs): reports strongly
  retain successful Active outputs. `output` borrows and `take_output` moves a
  Shared value; repeated take is OutputTaken and non-target is NotCollected.
- [`07_transient_resources.rs`](../examples/07_transient_resources.rs): graph
  passes handles only. Renderer owns allocation, command recording, submission,
  fences, aliasing, residency, and pool return; GraphRun drop never frees GPU.
- [`08_select_active.rs`](../examples/08_select_active.rs): Active selects and
  merges reverse dependency closure. Incomplete/cyclic inactive branches do not
  fail the selected version; no active target is NoActiveTarget.

Long-lived assets remain in host AssetManager/ResourceManager and are pulled by
handle inside nodes. A resource becomes a Slot value only when produced by this
execution.

## Atomic output commit and readiness

A success returns every Schema output exactly once. Missing, unknown, duplicate,
or wrong-typed outputs, task errors, and unwind panics fail the node and publish
nothing.

```text
Future Ready -> validate complete outputs -> choose cancel or commit
cancel wins: discard all outputs, Cancelled
commit wins: publish all outputs, Succeeded
```

Later cancellation cannot revoke a committed output. Atomicity protects Slot
visibility only, never I/O, allocations, GPU recording, or other task effects.
Connected One/Many inputs wait for all producers; a failed/cancelled/blocked
producer transitively blocks consumers. This is the sole 0.3 readiness policy.

## Editing, compile, and versions

Graph supports add/remove/rename, task and Schema replacement, connect,
disconnect, reconnect, automatic connection, exposed inputs, and Active changes.
`compile()` fully compiles current declarations into an immutable version and
does not publish it; the host chooses when to replace a current Arc version.
Failure never changes old versions or publication.

Edits are atomic. Connect rejects stale/foreign handles, direction/type errors,
duplicate edges, and One overflow. Reconnect validates before replacement;
removal deletes incident edges and Active mark. Schema replacement keeps NodeId
but stales all old Slot handles. It retains EdgeId and Many order only when
direction, Slot identity, and exact type remain compatible, not name or
position. If candidate retained bindings cannot jointly satisfy new cardinality,
replacement fails atomically and preserves the old state. Presence and
auto_collect changes alone do not remove compatible edges.

Selected compilation validates Schema, required inputs, bindings, exact types,
cardinality, and cycles; it lowers Slot bindings into dense execution data and
deduplicates node dependencies. Hot execution does no name/hash lookup or
dynamic declaration traversal. 0.3 uses full, not incremental, compilation.

## Local, Send, execution, and cancellation

Local and SendMode are type-level choices, not Cargo features. Local accepts
!Send values/factories/Futures. Send requires values `Any + Send + Sync +
'static`, Send Futures, and Send+Sync factories. Send permits cross-thread host
use but does not create threads or select a pool.

Versions can start independent concurrent runs; one GraphRun cannot be polled
concurrently. Factories are repeatable and cross-run mutable state belongs in
application handles/cells/locks. `execute` is an ordinary Future: core may call
short sync work inline, poll pending async work, and wake the host, but does not
spawn or select any particular async runtime or CPU scheduler. Runner reuse is CPU-only and
one runner has one live run.

States are Pending, Ready, Running, Succeeded, Failed, Cancelled, Blocked.
Independent branches continue after failure. Under unwind, task/Future-poll
panic becomes node failure; `panic = abort` cannot report. Cooperative cancel
stops new tasks, signals async tasks, and cannot preempt sync work. Tasks may
check cancellation. Abort drops pending Futures at next poll and returns a
cancelled report; dropping a run also drops Futures but returns no report.
Neither rolls back external effects. Retry, fallback, rollback, and durable
checkpoint/recovery are not core features.

## Automatic connection, errors, and release gate

Explicit connect is authoritative. `connect_nodes` is deterministic and atomic:
it considers exact-type source outputs and target inputs; One prefers exact
name+type then unique type; Slot identity is not cross-node semantic identity;
Many participates only with auto_collect; existing edges are not duplicated;
ambiguity errors; and success applies target Schema then source Schema order.
Fuzzy names, registration order, and hash iteration never resolve a tie. That
order defines report and Many order; independent task completion order is not
stable and side-effect order needs edges.

Errors are structured by Build/Edit, Compile, Start, Node, Report, and Execute
phase, carry relevant graph/node/slot/edge/name context, and do not collapse to
one anyhow error. Display text is not a machine format.

The target workload is about 100 nodes at 60/90/120 FPS. Benchmark 100, 1,000,
and 10,000 nodes across chains, fan-out/fan-in, Active closure, sync/async/mixed
work, fresh/reused runs; measure build, compile, run, reset, allocations, and
retained/peak memory including a repeated 100-node empty frame baseline.

Edition is 2021, std is required, and MSRV is Rust 1.71. Commit Cargo.lock and
test it on MSRV. CI covers fmt, clippy all targets/features, stable tests,
doctests/examples, MSRV tests, and rustdoc warnings. Core has no required async
runtime/executor dependency.

0.3 excludes lazy/Any/Min/custom readiness, streaming/channels/backpressure,
loops/feedback/continuous workflow, incremental compile/rerun/cache, built-in
scheduler, ECS access analysis, GPU barriers/queues/fences, cross-language type
identity, serialization, UI/plugins/remote workers/history, and inline/SBO
values or custom allocators.

## Resource lifetime and version boundaries

0.3 does **not** implement last-consumer early release. Ordinary Slot values remain until a run is terminal, and their exact drop timing is not contract. A future version may release intermediate values after their last consumer without breaking the public contract. Successful Active target outputs are strongly retained by `RunReport`; retaining a report can extend a transient handle's lifetime. Drop an unneeded report or use `take_output` to transfer the `Shared` value. This never changes renderer fence or pool ownership.

An exposed input survives `replace_schema` only if Slot identity, exact type, Presence, and Cardinality all remain unchanged. Otherwise it is removed and reported in `SchemaReplaceReport`. Old versions retain their own RunInput keys and output handles after edits; a new version rejects removed keys and stale declaration handles.

Cancellation is a linearized race, not a simple pre-commit check. After Future readiness and complete-output validation, cancellation or commit wins at one point: cancellation discards the whole set; commit publishes the whole set and leaves the node Succeeded. `abort` has the same ordering. Neither operation retracts already committed Active target outputs, which remain in reports.

For `Many + auto_collect`, automatic connection collects **all** remaining exact-type-compatible source outputs; it never applies One's name-first rule. Existing edges are skipped and retain their earlier binding order; new edges are atomically appended in source Schema order. One alone uses exact name+type, then only a unique type fallback.

## Combining the APIs

The complete numbered examples are the executable form of these usage patterns:

| Scenario | Reference |
| --- | --- |
| Keep v1 while editing and compiling v2 | [09_version_edit.rs](../examples/09_version_edit.rs) |
| Choose thread-local or cross-thread types | [10_local_send.rs](../examples/10_local_send.rs) |
| Reuse frame storage | [11_runner.rs](../examples/11_runner.rs) |
| Read failures and independent successes | [12_failure.rs](../examples/12_failure.rs) |
| Cancel a running task cooperatively | [13_cancellation.rs](../examples/13_cancellation.rs) |
| Read host-owned assets on demand | [14_long_lived_resources.rs](../examples/14_long_lived_resources.rs) |
| Match Slots by name and type | [15_auto_connect.rs](../examples/15_auto_connect.rs) |
| Declare explicit identities and auto-collection | [16_manual_schema.rs](../examples/16_manual_schema.rs) |
| Preserve compatible edges while replacing Schema | [17_replace_schema.rs](../examples/17_replace_schema.rs) |
| Replace a task without changing old versions | [18_replace_task.rs](../examples/18_replace_task.rs) |
| Run the same version with independent inputs | [19_concurrent_runs.rs](../examples/19_concurrent_runs.rs) |
| Supply Optional and Many external sources | [20_external_optional_many.rs](../examples/20_external_optional_many.rs) |
| Abort and reuse a runner | [21_abort_and_reuse.rs](../examples/21_abort_and_reuse.rs) |
| Supply a host poll loop and Waker | [22_manual_driver.rs](../examples/22_manual_driver.rs) |
| Forward shared values through fan-out | [23_shared_fan_out.rs](../examples/23_shared_fan_out.rs) |
| Handle edit, compile, and startup errors | [24_structured_errors.rs](../examples/24_structured_errors.rs) |

### Host driving and target outputs

The library owns no executor. A host test or application may choose a driver explicitly:

```rust
let version = graph.compile()?;
let report = futures_lite::future::block_on(version.execute(RunInputs::new()))?;
```

`futures-lite` is a development/test host only, never a core dependency or part of
the scheduling contract. A caller may instead manually poll `GraphRun` with its
own `Context` and Waker. A target output is retained or moved with the typed API:

```rust
let final_slot = graph.output::<RenderTargetHandle>(post, "final")?;
let mut report = futures_lite::future::block_on(version.execute(inputs))?;
let handle: Shared<RenderTargetHandle, Local> = report.take_output(final_slot)?;
```

### Complete structured error surface

All public errors are structured and carry applicable `ErrorContext` with graph,
node, edge, Slot, and name information. Human `Display` text is not a machine
format. The intended kind matrix is:

| Phase | Kinds |
| --- | --- |
| Edit | `DuplicateNodeName`, `InvalidNodeName`, `InvalidSchema`, `UnknownNodeName`, `UnknownSlotName`, `StaleNodeId`, `StaleSlotHandle`, `StaleEdgeId`, `ForeignHandle`, `WrongDirection`, `TypeMismatch`, `CardinalityOverflow`, `DuplicateEdge`, `InputSourceConflict`, `AmbiguousAutoMatch` |
| Compile | `MissingRequiredInput`, `NoActiveTarget`, `CycleDetected`, `InvalidBinding` |
| Start | `MissingRunInput`, `DuplicateRunInput`, `UnexpectedRunInput`, `RunInputCardinality`, `RunInputTypeMismatch` |
| Node | `User`, `Panic`, `InvalidOutputs`, `Cancelled`, `InternalInvariantViolation` |
| Report | `NotCollected`, `OutputUnavailable`, `OutputTaken`, `ForeignHandle`, `StaleSlotHandle`, `TypeMismatch` |
| Execute | `Start`, `Failed`, `Cancelled` |

User task failures enter the graph through `NodeError::user(source)`. The
structured NodeError retains the application error as its source while report
consumers can still inspect node identity and error kind.

Public error kinds and node statuses are non-exhaustive. Version 0.3.x preserves
Active, ordering, atomic output, and cancellation behavior; incompatible changes
belong in 0.4. Internal invariant failures belong to node diagnostics rather than
bypassing the final run report.

### Required verification and definition of done

The release test matrix covers ordinary edges; multi-input/output; fan-out; a
node pair with many Slot bindings but one wake-up; every Presence/Cardinality
combination; deterministic Many order; automatic-match ambiguity and atomicity;
sync, async, and mixed polling; Pending/wake behavior; complete atomic outputs;
RunInputs; report borrowing and taking; repeated/concurrent versions; runner
reuse; user failure, panic, invalid outputs, cancellation/abort races, and
dropped Futures; edit/remove/rename/replace; stale and foreign handles;
inactive invalid branches; old/new version overlap; Local `Rc` and `!Send`
Futures; Send rejection of incompatible values, futures, and factories; manual
polling; and at least one real test runtime.

0.3 is complete only when every listed behavior has an automated test or
runnable example, benchmark baselines exist, and locked Rust 1.71 CI passes.
Users must be able to declare sync/async graphs naturally; use Optional, Many,
fan-out, RunInputs, target outputs, Active selection, edit/version isolation,
Local/Send boundaries, host-driven Futures, structured reports, atomic
cancellation/abort, and runner reuse while leaving asset and GPU lifetimes to
the host.

### Engineering and dependency constraints

The package uses edition 2021, requires `std`, and supports Rust 1.71. Commit
`Cargo.lock`; MSRV CI must run with `--locked`. CI covers formatting, clippy on
all targets/features, stable tests/doctests/examples, Rust 1.71 tests, and
rustdoc warnings as errors. Any core, development, example, or benchmark
dependency change must be resolved and tested on Rust 1.71.

The core dependency list remains empty. `futures-lite` is the minimal dev-only host
runtime for examples and integration tests; it must not become a public runtime
choice. Tokio, async-std, rayon, futures utility/executor crates,
`async-trait`, workflow storage, and concurrency containers are not justified
core dependencies. New dependencies require an implemented feature or a
measured benchmark result.
