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
| `E-01` | Direct internal synchronous-task path | A baseline profile shows immediate-ready task boxing/polling is material | Planned |
| `E-02` | Reuse `GraphRunner` run buffers and capacities | Allocation probe identifies reusable-run allocation cost | Planned |
| `E-03` | Wake-driven runnable tracking | Pending-heavy async profile identifies full-vector polling cost | Planned |
| `E-04` | Compile-time incoming-edge and output indexes | Compile profile identifies repeated full-edge scans | Planned |
| `E-05` | Compact small collections; evaluate `smallvec 1.15.1` | Allocation evidence identifies small fan-in/fan-out or task-I/O collections | Planned |
| `E-06` | Hash strategy A/B | Lookup/profile evidence identifies a hash-table hotspot | Planned |
| `E-07` | Dispatch-contention primitives | Dispatch profile proves queue or lock contention | Planned |

## Completed Experiments

No experiments have been measured yet.
