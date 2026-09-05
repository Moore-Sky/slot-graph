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

Rustdoc also checks rejected Send values/factories/Futures, Local controls,
repeatable factories, and exclusive runner borrowing using `compile_fail`.

Once an implementation satisfies a contract, remove that test's ignore marker.
Controlled asynchronous tests use bounded poll loops for intermediate states;
completion is driven only after the test releases a gate or requests abort.
No test should depend on the completion order of independent nodes.

This is the initial behavioral matrix, not a coverage percentage or the v0.3
release gate. Benchmark baselines, platform coverage, and additional adversarial
interleavings remain part of implementation and release work.
