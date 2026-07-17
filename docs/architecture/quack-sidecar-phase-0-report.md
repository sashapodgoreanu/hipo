# Quack sidecar — Phase 0 report

## Outcome

**Conditional GO for Phase 1 (`RunDatabase` abstraction). NO-GO for replacing
the production DuckDB CLI backend.**

The spike proves that the proposed process boundary works on Windows x64:

- one child process embeds and owns the run-scoped DuckDB database;
- the parent embeds a data-empty DuckDB client and uses Quack over loopback;
- complete SQL statements execute in the sidecar;
- concurrent readers and concurrent appenders work;
- client-side Quack attachment and server-side DuckDB attachment work;
- an in-memory run database spills to the configured per-run directory;
- cancellation kills the complete sidecar process;
- Quack is faster than producing and rereading Parquet for the measured
  single-consumer workload, while Parquet remains a likely fan-out fallback.

This is not yet production approval because offline extension packaging,
cross-platform builds, broad parity and the full benchmark matrix remain open.

The follow-up local lifecycle probe also validates that prewarming is viable,
but it deliberately does not define the production elastic implementation.
Policy/admission and provisioning must be separated so the same control plane
can use local child processes first and Kubernetes Jobs/Pods later.

## Implementation under test

The isolated PoC is in
[`spikes/quack-sidecar`](../../spikes/quack-sidecar/README.md). It is not a
member of the production Cargo workspace and does not change the CLI execution
path.

Pinned stack:

- `duckdb-rs 1.10504.0`, embedding DuckDB `1.5.4`;
- the same DuckDB/Quack version in client and server;
- Duckle spike protocol version `1` checked before the first query;
- Quack server bound to `127.0.0.1` with a generated token;
- atomic ready file used only for bootstrap metadata;
- SQL data transported only through Quack;
- RAII guards kill the child and clean its per-run directory on every exit.

The standalone crate currently requires Rust `1.85.1`; the production
workspace still advertises Rust `1.80`. This must be resolved before bringing
the embedded dependency into a production crate.

## Product switches applied first

Phase 0 also made the approved temporary product changes:

- SlothDB is not selectable or installable; persisted `slothdb` selection is
  normalized to DuckDB;
- `xf.dbt` remains deserializable but is marked unavailable and compilation
  returns `component_disabled`;
- dbt Fusion/dbt Core provisioning is no longer published, started or offered
  by first-run setup;
- no silent fallback was introduced.

## Smoke validation

Command:

```powershell
cargo run --manifest-path spikes/quack-sidecar/Cargo.toml -- smoke
```

Representative release result on 2026-07-17. The smoke dataset contains 2M
rows and a deterministic 128-byte payload per row so optimized builds still
exceed the 128 MB memory budget during the sort:

| Check | Result | Duration |
|---|---:|---:|
| Sidecar startup/readiness | pass | 264 ms |
| Remote create + aggregate, 2M rows | pass | 3,359 ms |
| 2 concurrent full readers | pass | 176 ms |
| 2 concurrent appenders, same table | 200,000 rows, no loss | 241 ms |
| Client and server attachment | pass | 599 ms |
| Sort under 128 MB budget | 2M rows, 491,520 peak spill bytes observed | 1,988 ms |
| Process kill | sidecar exited | 12 ms |
| Quack client unblock after kill | expected transport error | 2,012 ms |

The first cancellation attempt exposed two useful issues and the final PoC
addresses both:

1. an optimized `sum(range(...))` was not a valid long-running cancellation
   workload;
2. a failing harness could leave an orphan sidecar without an ownership guard.

The final workload uses a nontrivial Cartesian computation and every spawned
sidecar is owned by a kill-on-drop guard. Normal stage clients use a 30-second
HTTP timeout with retries disabled; the cancellation probe uses a 2-second
timeout. The future runner must report cancellation immediately after process
death and drain blocked client workers without delaying pipeline state.

## Initial Quack vs Parquet benchmark

Command:

```powershell
cargo run --manifest-path spikes/quack-sidecar/Cargo.toml --release -- benchmark
```

Environment: Windows x64, DuckDB `1.5.4`, one sidecar, one client, 2,000,000
rows containing a `BIGINT` and a 32-byte MD5 string (64,000,000 payload bytes),
512 MB sidecar memory budget. Values below are one baseline sample and are not
release thresholds.

| Metric | In-memory sidecar | File-backed sidecar |
|---|---:|---:|
| Startup/readiness | 230 ms | 330 ms |
| Create dataset | 1,689 ms | 2,656 ms |
| Quack full transfer + local consume | **290 ms** | **530 ms** |
| Parquet ZSTD export | 774 ms | 753 ms |
| Parquet local read + consume | 185 ms | 172 ms |
| Parquet export + read | 959 ms | 925 ms |
| Shutdown | 29 ms | 26 ms |
| Run database file | 0 | 35,139,584 bytes |
| Parquet snapshot | 38,822,223 bytes | 38,822,223 bytes |

External 20 ms process sampling over both storage variants observed:

- parent/client peak Working Set: `59,588,608` bytes;
- sidecar peak Working Set: `166,375,424` bytes.

The executable was `28,178,944` bytes in release. The first uncached Windows
release build took `817,436` ms (13m 37s). Subsequent wrapper-only builds are
incremental, but CI cache and separate sidecar staging are mandatory.

### Interpretation

- For one consumer in this workload, direct Quack transfer is materially
  faster than creating and rereading a Parquet snapshot.
- A Parquet snapshot is still attractive when several consumers can reuse one
  export. Its crossover depends on consumer count, projection, payload types
  and whether data is already file-backed.
- In-memory storage was faster here and successfully spilled in the bounded
  smoke workload. This does not yet prove that it is the best default for
  10M/100M rows, joins or multiple concurrent pipelines.
- The parent RSS stayed well below the sidecar RSS while consuming the stream,
  supporting the process-isolation goal. More sampling is required for large
  transfers and runtimes that materialize rows.

## Prewarm lifecycle probe

Command:

```powershell
cargo run --manifest-path spikes/quack-sidecar/Cargo.toml --release -- pool-smoke
```

Representative result on 2026-07-17 with three local workers limited to 128 MB
each:

| Measure | Result |
|---|---:|
| Single cold worker readiness | 289 ms |
| Three workers prewarmed concurrently | 503 ms |
| Warm exclusive checkout | 1 µs |
| Replacement ready after lease release | 227 ms |
| Queued fourth pipeline admitted through replacement | 384 ms |

The probe verified distinct worker PIDs, no catalog leakage between two
pipelines, bounded blocking at fixed capacity, termination of every consumed
worker and publication of replacements only after a successful Quack query.
All PIDs present after replenishment differed from the consumed workers.

This probe represents only the future `LocalProcessProvider`. The production
pool requires a bounded elastic policy with a hard maximum and global resource
budget. Saturation must queue rather than create an unbounded on-demand worker.

## Elastic policy review and Kubernetes portability

An existing C# elastic pool was reviewed as design input, not as code to port.
Its useful ideas are base capacity, 70% growth threshold, coalesced growth,
quarter-base growth steps, peak-based scale-in and deferred release of active
resources. Duckle must change three aspects:

1. add `max_capacity` plus global RAM/CPU/disk reservation, including workers
   still starting;
2. remove the unlimited on-demand bypass used when a pool is saturated;
3. renew the scale-in observation window even after a no-op evaluation, so an
   old peak cannot keep capacity high forever.

The target contract is split into a pure elastic policy, a cancellable
admission/lease control plane and an asynchronous `WorkerProvider`. Local
process and Kubernetes providers expose the same opaque worker identity,
endpoint, readiness and idempotent termination operations.

For Kubernetes, Duckle remains the owner of desired capacity and exclusive
single-use leases. A normal Deployment/HPA is not the lease manager: it cannot
atomically bind one ready worker to one pipeline or express single-use
completion. The first remote provider should create an ephemeral one-worker
Job and delete the Job, rather than only its Pod, on release/cancellation. A
later multi-replica supervisor can use CAS/Kubernetes Lease or a
CRD/controller. Provider readiness is followed by a Quack handshake before
admission. Pod resources map the worker memory/CPU profile, while disk-backed
`emptyDir` with `sizeLimit` provides bounded spill.

## Findings from current Quack documentation

Quack is HTTP-based and is designed for multiple clients and concurrent
writers, but DuckDB documents the extension as experimental in `1.5.3`; API
and protocol details may change before DuckDB `2.0`. Quack exposes no dedicated
request-timeout option. The client uses DuckDB's `http_timeout`, whose default
is 30 seconds, and `http_retries`, whose default is 3. These defaults must be
overridden for a local per-run sidecar.

Primary references:

- [Quack overview](https://duckdb.org/docs/current/quack/overview)
- [Quack function and setting reference](https://duckdb.org/docs/current/quack/reference)
- [Quack security model](https://duckdb.org/docs/current/quack/security)
- [DuckDB configuration reference](https://duckdb.org/docs/stable/configuration/overview)
- [Quack release and concurrency benchmark](https://duckdb.org/2026/05/12/quack-remote-protocol)

## Open gates before production migration

1. Bundle the pinned Quack extension for offline startup; do not depend on
   transparent or first-run downloads.
2. Validate Windows arm64, Linux x64/arm64 and macOS targets.
3. Decide how the Rust minimum-version increase is handled.
4. Run the full 1M/10M/100M and 2/4/8-client benchmark matrix with p50/p95,
   CPU, RSS, disk bytes and loopback bytes.
5. Measure fan-out crossover: one Quack stream per consumer versus one shared
   Parquet snapshot.
6. Test types, nulls, decimals, timestamps, nested values and zero-row results.
7. Validate disk exhaustion through `max_temp_directory_size` and free-space
   checks.
8. Add parent-death containment using a Windows Job Object and Unix process
   group/parent monitor; RAII only covers normal parent unwinding.
9. Define token-file permissions and verify redaction in every error/log path.
10. Exercise version/extension mismatch and corrupt readiness metadata.
11. Implement and test bounded elastic admission, renewable scale-in windows,
    startup failure backoff and global resource reservations.
12. Validate the provider contract with a fake remote provider that models
    Kubernetes Pending/readiness/delete latency before implementing Kubernetes.

## Phase 1 entry condition

Phase 1 may introduce `RunSession`, `WorkerPoolControl`, a provider-neutral
`WorkerProvider`, `LocalProcessProvider` and `RunDatabase` behind a
feature/compatibility boundary while keeping the CLI as the default backend.
It must not yet route production pipelines to Quack or remove any CLI packaging
path. Kubernetes itself is not a Phase 1 deliverable; preserving this provider
boundary is.
