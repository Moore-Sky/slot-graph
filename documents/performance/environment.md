# Benchmark Environment Manifest

This manifest records the machine and toolchain used for a formal benchmark
round. Copy the template section for every machine or materially different
configuration. Do not compare measurements across manifests without calling out
the difference.

## Recorded Host: Initial Development Machine

| Field | Value |
| --- | --- |
| Recorded at | 2026-09-05, 21:05-21:48 Asia/Tokyo |
| Operating system | Windows 11 Home, version 10.0.26200, build 26200, 64-bit |
| CPU | AMD Ryzen 7 8845H with Radeon 780M Graphics |
| Physical cores | 8 |
| Logical processors | 16 |
| Reported maximum clock | 3801 MHz |
| Physical memory | 64,163,266,560 bytes (approximately 59.8 GiB) |
| Cargo target | `x86_64-pc-windows-msvc` |
| Active power plan before a formal run | Balanced (`381b4222-f694-41f0-9685-ff5bb260df2e`) |
| Formal-run power plan | High performance (`8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c`) |
| Source revision observed | `f486dbc9bea896ab614bb5cdd53e30441f33d034` |
| Worktree state observed | Clean at the start of all timing rounds; generated artifacts remained under ignored `target/` |
| Rust toolchain observed | `rustc 1.98.0 (88d9e12ae 2026-08-18)` |
| Cargo observed | `cargo 1.98.0 (797e8a9bc 2026-08-05)` |
| LLVM observed | 22.1.8 |
| MSRV verification toolchain | `rustc 1.71.0 (8ede3aae2 2023-07-12)`, LLVM 16.0.5; `cargo 1.71.0 (cfd3bbd8f 2023-06-08)` |
| Power source during observation | AC power reported (`BatteryStatus = 2`); battery charge 96% |
| Thermal instrumentation | No package-temperature sensor was recorded; no cooling constraint was observed |

All formal timings used the explicit `+stable` toolchain above. The active power
plan was restored to Balanced after every Criterion round and after the
allocation probe.

## Formal-Run Procedure

1. Close foreground applications that can create sustained CPU, GPU, disk, or
   network load. Connect AC power and allow the system to reach a steady state.
2. Capture `git status --short`, `git rev-parse HEAD`, `rustc -Vv`, `cargo -V`,
   and `powercfg /getactivescheme`.
3. Resolve and save the prior plan GUID. Switch to High Performance only for the
   measurement window, then restore the prior GUID in a PowerShell `finally`
   block.
4. Run the required correctness and MSRV commands before timing.
5. Run three independent Criterion rounds. Use a new process for each round and
   record the round start/end time, command, and Criterion baseline name.
6. Run allocation probes serially. Include allocations performed by workers in
   dispatched workloads.
7. Restore the original power plan even when a command fails. Commit only the
   English report and experiment summaries; generated Criterion artifacts remain
   under `target/`.

## PowerShell Power-Plan Wrapper

This host resolved the High Performance alias to the GUID used below. The
wrapper restores the original plan even when the benchmark command fails.

```powershell
$previousPlan = (powercfg /getactivescheme | Select-String -Pattern '[0-9a-fA-F-]{36}').Matches.Value
$highPerformancePlan = '8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c'
try {
    powercfg /setactive $highPerformancePlan
    cargo bench --locked --bench <criterion-target> -- --save-baseline v0.5.0-r1 --warm-up-time 3 --measurement-time 10 --sample-size 100 --confidence-level 0.99 --significance-level 0.01
}
finally {
    powercfg /setactive $previousPlan
}
```

## Required Metadata Template

Copy this table into each version report. Values in square brackets must be
filled from the actual run; do not infer them from this initial record.

| Field | Value |
| --- | --- |
| Report version | `[v0.5.0]` |
| Round identifier | `[baseline-round-1]` |
| Start and end time | `[ISO 8601 timestamps with timezone]` |
| Git revision | `[full commit hash]` |
| Worktree state | `[clean / exact tracked or untracked changes]` |
| Package version | `[Cargo.toml version]` |
| Benchmark profile | `[bench / release-derived profiling profile]` |
| Rust compiler | `[rustc -Vv output]` |
| Cargo | `[cargo -V output]` |
| Target triple | `[target]` |
| CPU and logical processor count | `[model; physical/logical counts]` |
| Memory | `[installed bytes or GiB]` |
| Operating system | `[edition, version, build, architecture]` |
| Power plan before / during / after | `[GUID and display name for each]` |
| AC/battery and thermal state | `[state and any cooling constraint]` |
| Criterion version and settings | `[version; warm-up; measurement; samples; confidence; significance]` |
| Allocation probe configuration | `[allocator, serial/worker policy, warm-up]` |
| Command lines | `[verbatim commands]` |
| Artifact paths | `[target/criterion paths; WPR/ETL path when collected]` |

## Required Verification Commands

Run these against the same locked dependency set used by the benchmark source:

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

Run every numbered example after these commands:

```powershell
Get-ChildItem examples -Filter '*.rs' | Sort-Object Name | ForEach-Object {
    cargo run --locked --example $_.BaseName
}
```
