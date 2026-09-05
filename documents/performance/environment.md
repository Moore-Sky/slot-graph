# Benchmark Environment Manifest

This manifest records the machine and toolchain used for a formal benchmark
round. Copy the template section for every machine or materially different
configuration. Do not compare measurements across manifests without calling out
the difference.

## Recorded Host: Initial Development Machine

| Field | Value |
| --- | --- |
| Recorded at | 2026-09-05, Asia/Tokyo |
| Operating system | Windows 11 Home, version 10.0.26200, build 26200, 64-bit |
| CPU | AMD Ryzen 7 8845H with Radeon 780M Graphics |
| Physical cores | 8 |
| Logical processors | 16 |
| Reported maximum clock | 3801 MHz |
| Physical memory | 64,163,266,560 bytes (approximately 59.8 GiB) |
| Cargo target | `x86_64-pc-windows-msvc` |
| Active power plan before a formal run | Balanced (`381b4222-f694-41f0-9685-ff5bb260df2e`) |
| Formal-run power plan | `High performance` - record the resolved GUID below |
| Source revision observed | `0ffa48a0285958dbaec2b030156206e91b59c4d4` |
| Worktree state observed | `documents/bench-plan.md` was untracked; formal baseline requires a clean revision |
| Rust toolchain observed | `rustc 1.97.0-nightly (507271bc1 2026-05-17)` |
| Cargo observed | `cargo 1.97.0-nightly (4d1f98451 2026-05-15)` |
| LLVM observed | 22.1.4 |
| MSRV verification toolchain | `1.71.0` (record `rustc -Vv` at execution time) |

The observed nightly toolchain is useful for development, but it is not a
formal baseline identity. A formal report must contain the exact timing toolchain
and source revision used for its results.

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

Replace the placeholder with the High Performance GUID resolved on the host.
The wrapper restores the original plan even when the benchmark command fails.

```powershell
$previousPlan = (powercfg /getactivescheme | Select-String -Pattern '[0-9a-fA-F-]{36}').Matches.Value
$highPerformancePlan = '<resolved-high-performance-guid>'
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
