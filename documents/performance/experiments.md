# Performance Experiment Log

Record every measured optimization strategy here, including rejected ones. One
entry must isolate one attributable change. Generated Criterion HTML, JSON, ETL,
and raw profiler artifacts remain under `target/` or the recorded local artifact
path; this file contains the durable English summary.

## Decision Rules

- Compare a candidate with its immediate baseline in three independent A/B
  rounds on the same environment manifest.
- Accept only when target latency improves by at least 5% and Criterion's 99%
  confidence interval excludes zero, or allocation calls or allocated bytes
  improve by at least 10% without more than 2% latency regression.
- Reject a candidate that regresses a protected 100-node sync or async workload
  by more than 2%.
- Remove rejected implementation code without consuming a version. Keep its
  measurements in this log.
- Do not push benchmark or optimization commits.

## Entry Template

Copy this complete section for every experiment.

### `[experiment-id]: [short strategy name]`

| Field | Value |
| --- | --- |
| Status | `[planned / running / accepted / rejected / ruled out]` |
| Date | `[ISO 8601 timestamp]` |
| Owner | `[name or handle]` |
| Base version and revision | `[v0.5.0 / full hash]` |
| Candidate revision | `[full hash or uncommitted local diff identifier]` |
| Environment manifest | `[link and section]` |
| Worktree state | `[clean / exact changes]` |
| Hypothesis | `[measurable claim]` |
| Isolated change | `[implementation-only description]` |
| Dependency change | `[none, or package/version/MSRV/license/maintenance/security/cost rationale]` |
| Protected workloads | `[100-node sync and async variants]` |
| Timed target workload | `[target and exact variant]` |
| Criterion command | `[verbatim]` |
| Allocation command | `[verbatim or not applicable]` |
| Profile command and artifacts | `[verbatim command; WPR/WPA/ETL path or not collected]` |
| Correctness gate | `[commands and pass/fail result]` |
| Round 1 | `[baseline/candidate estimate and 99% CI]` |
| Round 2 | `[baseline/candidate estimate and 99% CI]` |
| Round 3 | `[baseline/candidate estimate and 99% CI]` |
| Allocation comparison | `[calls, gross requested bytes, peak/end live-byte deltas; worker policy]` |
| Protected-workload comparison | `[each delta]` |
| Decision | `[accepted/rejected/ruled out and rule applied]` |
| Follow-up | `[next experiment or none]` |

#### Notes

`[Explain variance, correctness observations, ordering/cancellation checks, and
why the decision follows the stated rule.]`

## Planned Campaign

| ID | Strategy | Entry condition | Status |
| --- | --- | --- | --- |
| `E-01` | Direct internal synchronous-task path | Differential allocations plus source inspection show one ready-Future box per synchronous node | Accepted as `0.5.1` |
| `E-02` | Reuse `GraphRunner` run buffers and capacities | Allocation probe identifies reusable-run allocation cost | Planned |
| `E-03` | Wake-driven runnable tracking | Pending-heavy async profile identifies full-vector polling cost | Planned |
| `E-04` | Compile-time incoming-edge and output indexes | Compile profile identifies repeated full-edge scans | Planned |
| `E-05` | Compact small collections; evaluate `smallvec 1.15.1` | Allocation evidence identifies small fan-in/fan-out or task-I/O collections | Planned |
| `E-06` | Hash strategy A/B | Lookup/profile evidence identifies a hash-table hotspot | Planned |
| `E-07` | Dispatch-contention primitives | Dispatch profile proves queue or lock contention | Planned |

## Diagnostic Measurement: A/B/C/D

| Field | Value |
| --- | --- |
| Status | Complete |
| Date | 2026-09-05, Asia/Tokyo |
| Revision | `2fb4f75f753e3fe3c6bb9def534a578a817b469a` |
| Timing artifacts | [v0.5.0-diagnostics.csv](v0.5.0-diagnostics.csv) |
| Allocation artifacts | [v0.5.0-diagnostic-allocations.csv](v0.5.0-diagnostic-allocations.csv) |
| Timing command | `cargo +stable bench --locked --bench runtime_sync -- diagnostic_ --save-baseline v0.5.0-diag-r<N> --warm-up-time 3 --measurement-time 10 --sample-size 100 --confidence-level 0.99 --significance-level 0.01` |
| Allocation command | `cargo +stable bench --locked --bench allocations` |
| CPU profile attempt | `wpr.exe -start CPU -filemode`, followed by `cargo +stable bench --locked --profile profiling --bench runtime_sync -- diagnostic_b/scalar_chain/reusable/bound/100 --profile-time 30` |
| CPU profile result | Blocked before the benchmark: Windows rejected system profiling policy with `0xc5585011`; no ETL or CPU attribution was produced |

A reusable empty-node run costs 11.909 us and 103 allocation calls for 100
nodes. Adding scalar Slot delivery raises this to 47.716 us and 901 calls.
Adding the 10x10 fan-out/fan-in `Many` topology raises it to 59.209 us and
1,146 calls. The fixed one-worker dispatcher increases the independent Ready
workload from 29.443 us inline to 56.035 us while adding only five allocation
calls. These differences select E-01 as the first isolated core experiment;
dispatcher work remains separate.

## Completed Experiments

### E-01: Direct internal synchronous-task path

| Field | Value |
| --- | --- |
| Status | Accepted as `0.5.1` |
| Base | `0.5.0`, revision `080733ac87fdaaa40474f5e95caa5335ba73833b` |
| Isolated change | Private Task invocation representation; sync results bypass boxed Ready futures, async factories are unchanged |
| Dependency change | None |
| Primary evidence | [v0.5.1 report](v0.5.1.md) |
| Allocation gate | B reusable: 901 to 801 calls (-11.1%); 42,752 to 35,552 bytes (-16.8%) |
| Protected workloads | Three sync diagnostic rounds, three full Local/Send async rounds, and three interleaved eight-worker fixed-pool pairs |
| Decision | Accepted: allocation calls and bytes improve by more than 10%; no protected latency regression exceeds 2% in interleaved checks |
| Follow-up | E-02: measure and isolate the remaining per-node input/output buffers before changing their ownership |

The change preserves the public API. Synchronous results reach the same output
validation, cancellation, failure, and atomic commit routines used by completed
asynchronous tasks. The only non-contractual difference is that independent
inline sync side effects may occur earlier within one driver poll; dependency
edges remain the required ordering mechanism.
