# Quack sidecar — Phase 0 report

## Outcome

**Conditional GO for Phase 1 (`RunDatabase` abstraction). NO-GO for replacing
the production DuckDB CLI backend.**

The spike proves that the proposed process boundary works on Windows x64:

- one child process embeds and owns the run-scoped DuckDB database;
- the parent embeds a data-empty DuckDB client and uses Quack over loopback;
- complete SQL statements execute in the sidecar;
- the measured two-writer append workload completes without lost rows;
- cloned DuckDB client connections inherit an existing Quack attachment, but
  true concurrent stages require a distinct sticky attachment alias per
  request;
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
- Quack server bound to `127.0.0.1` with a supervisor-generated 256-bit token;
- token-free atomic ready file containing endpoint, version and security profile;
- parameterized temporary secret reused by stateless `quack_query` calls;
- no client-side Quack `ATTACH` in the selected ordinary stage path;
- complete ordinary statements sent through stateless `quack_query`, without
  token inline;
- ordinary DuckDB SQL requests use Quack; Parquet remains an explicit fallback
  for relation transfer and is benchmarked separately;
- RAII guards kill the child and clean its per-run directory on every exit.

The standalone crate currently requires Rust `1.85.1`; the production
workspace still advertises Rust `1.80`. This must be resolved before bringing
the embedded dependency into a production crate.

The PoC child-only environment bootstrap is still a deliberate shortcut and
fails the normative
[worker bootstrap security contract](adr-worker-identity-bootstrap-security.md).
Production integration must replace it with an inherited anonymous bootstrap
pipe plus a token-free control channel before any pipeline is routed to Quack.

## `ATTACH`: transport vs Data Source

The Phase 0 decision distinguishes two operations that happen to use the same
DuckDB keyword:

- Client `ATTACH … TYPE quack` creates a sticky transport session in the main
  process. It is measured but not selected for ordinary execution.
- Server `ATTACH` for Postgres, MySQL, DuckLake, DuckDB/SQLite files or similar
  Data Sources executes inside the sidecar catalog. The planner/orchestrator
  owns its resolved SQL, alias and dependency order; the client only forwards
  it with a serialized stateless `quack_query` setup call.

The new probe creates an external DuckDB file, forwards a server `ATTACH` via a
stateless request, then reads its table from a separate stateless request. It
returns `42`, proving the Data Source attachment persists in the sidecar
catalog. S3 is normally server-side secret/`httpfs` configuration plus a table
function read, rather than a DuckDB `ATTACH`.

## Quack security and deployment audit

The official security, deployment, reverse-proxy and WebAssembly guidance was
reviewed after the first spike. Material findings:

- Quack exposes the complete SQL surface visible to its server session;
- default token authentication is active, but default authorization permits
  every query;
- non-local deployment must use a proven TLS-terminating reverse proxy;
- sticky server state was observed while a client `ATTACH` remained live;
  whether HTTP connection reuse is required is not established, and
  `httpfs_connection_caching` is treated only as an optimization;
- streaming proxies must disable buffering and raise request-size/timeouts;
- DuckDB-Wasm supports Quack, but browser networking requires HTTPS and CORS
  when cross-origin;
- a SQL-prefix regex is not production read-only enforcement.

The PoC now declares `execution_trusted_full_sql_v1`. Full SQL is deliberate
for the trusted pipeline supervisor and is never a future browser profile. The
smoke test proves token-free readiness, explicit hook configuration, rejection
of a random invalid token, absence of tokens from the emitted authentication
error, and absence of tokens from ordinary client query text.

Future Book publication is recorded only as an architectural constraint. It
must use a separate publication process/database and security profile, never a
live execution worker or its token.

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
rows and a deterministic 128-byte payload per row. The sort connection lowers
the global budget to 96 MB to make spill observable and repeatable:

| Check | Result | Duration |
|---|---:|---:|
| Sidecar listener/`ready.json` availability (not worker readiness) | pass | 187 ms |
| Security profile, scoped secret, invalid-token rejection | pass | 142 ms |
| Sticky `TEMP` retained by one `ATTACH`; isolated from a separately attached Quack alias/session | pass | 170 ms |
| Remote create + aggregate, 2M rows | pass | 2,212 ms |
| 2 concurrent full readers | pass | 229 ms |
| 2 concurrent `INSERT … SELECT range(100000)`, same table | 200,000 rows, no loss | 248 ms |
| Client and server attachment | pass | 183 ms |
| Sort under 96 MB budget | 2M rows, 884,736 peak spill bytes observed | 2,457 ms |
| Process kill | sidecar exited | 224 ms |
| Quack client unblock after kill | expected transport error | 2,029 ms |

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

The `TEMP` check above must not be generalized to every clone: clones that use
the master’s same Quack `ATTACH` alias share its sticky server session and can
see its `TEMP` state. Isolation was observed only between separately attached
Quack sessions.

## Clone, ATTACH and parallelism characterization

Command:

```powershell
cargo run --manifest-path spikes/quack-sidecar/Cargo.toml --release -- clone-attach-smoke
```

The probe keeps one sidecar alive, sets the server to `threads=2`, executes
remote `sleep_ms(250)` statements sequentially and concurrently, and reads the
structured Quack logs to count distinct `quack_connection_id` values. It also
checks that `Connection::try_clone()` sees the existing attachment, sticky
`TEMP` state and regular tables, and that dropping clones does not invalidate
the master. The table contains the observed range from six recorded release
runs on Windows x64 on 2026-07-17:

| Client topology | Concurrent 2 | Concurrent 4 | Concurrent 8 | IDs at 8 | Interpretation |
|---|---:|---:|---:|---:|---|
| clones, shared master alias | 517–527 ms | 1,023–1,046 ms | 2,069–2,070 ms | 1 | serialized |
| clones, distinct aliases on same master database | 262–267 ms | 266–268 ms | 270–271 ms | 8 | parallel |
| clones, stateless `quack_query` | 265–266 ms | 266–269 ms | 272–277 ms | 8 | parallel |
| independent client databases | 262–275 ms | 264–275 ms | 282–286 ms | 8 | parallel |

The result resolves the design question. `try_clone()` opens another local
connection to the same client database and inherits visibility of the Quack
catalog, but all clones that query one attachment alias still target one sticky
server session. DuckDB locks/serializes the work on that session. Creating
distinct aliases on the master produces distinct Quack server connections and
approximately 2x/4x/8x overlap. Stateless `quack_query` on clones produces the
same overlap and one server ID per concurrent request without any attachment;
separate client databases and preattached aliases add no required capability
for ordinary stage SQL.

The production client is therefore one worker-scoped master database plus a
per-worker `QuackPermitGate` semaphore. `WorkerPoolControl` is the only elastic
pool: it contains warm workers between runs, while `QuackPermitGate` contains
no connections or members.
Before public readiness the worker creates its scoped secret and passes one
authenticated stateless health query; it owns zero precreated clones and zero
Quack attachments. The initial concurrency default and Phase 1 hard maximum
are `8` (validated configuration range `1..=8`). An admitted
execution unit pays the 18–96 µs `try_clone()` cost, runs the stage request or
existing SQL batch selected by the orchestrator, then drops the clone and
releases its permit. There is no `ATTACH`, and the backend does
not change stage ordering, batching, event emission or parallel-dispatch
decisions. `threads` remains the independent per-query CPU-parallelism budget.

The probe also sends the same three-statement remote batch through both the
attached and stateless paths: materialize stage A, materialize dependent stage
B, then select B. Both return one final row with value `10`; a later stateless
request reads the regular result table successfully. This proves only that the
tested ordered statements execute in one remote request and leave the expected
regular table. It does not prove transaction atomicity, rollback after an
intermediate failure, marker compatibility, preview behavior or attribution of
a failure to the originating stage; those remain Phase 2 integration tests.

### Rejected sticky-ATTACH bootstrap characterization

Before selecting stateless execution, the same command profiled the initial
client bootstrap and, from an already authenticated master, the exact sequence
required to add a sticky member:
`try_clone -> ATTACH on clone -> first remote query`. Three profiled release
runs, 20 slave samples per run, produced:

| Operation | Observed result |
|---|---:|
| Initial master `ATTACH`, sidecar already ready | 1.11–1.21 s |
| Complete master client bootstrap, latest three runs | 1.24–1.27 s |
| `try_clone()`, 60 samples | 18–47 µs |
| `ATTACH` executed by the new clone, 60 samples | 6.37–12.10 ms |
| `ATTACH` median per run | 8.28–10.08 ms |
| First query on the new alias, 60 samples | 2.10–4.59 ms |
| First-query median per run | 2.73–2.99 ms |
| Clone start through first successful query | 8.99–15.39 ms |
| Total median per run | 10.75–13.61 ms |
| Total p95 per run | 12.15–14.83 ms |

The complete bootstrap includes opening the in-memory client database, loading
Quack, configuring HTTP, creating the scoped secret and the initial attach, but
the initial-attach timer isolates the `ATTACH` statement itself. Every profiled
run produced 20 distinct server `quack_connection_id` values for the 20 slave
aliases. The clone could reuse the parameterized temporary secret created by
the master, so no credential duplication or second client database is needed.
One earlier unprofiled complete-connect sample reached 3.39 s; more
cold-cache/p50/p95 runs are required before defining an SLO.

The 8-way stateless matrix supersedes this topology for ordinary stages. The
attachment measurements remain useful evidence for any future explicitly
sticky operation, but they do not define normal readiness.

The result resolves the warm-worker decision. `WorkerPoolControl` is one
bounded elastic pool of warm workers. Each worker is provisioned with one local client master, a
scoped temporary secret and one successful stateless health query.
`quack_parallelism` defaults to `8` and has a Phase 1 maximum of `8`, but
readiness includes zero precreated clones and zero attachments. A ninth
concurrent request waits FIFO for a cancellable permit; each admitted request
clones on demand and never executes `ATTACH`. There is no connection-member
failure or eager/growing client-connection pool: clone/query errors release
their permit and return to the orchestrator; a sidecar failure is a worker
failure handled by `WorkerPoolControl`.

### TEMP and thread ownership decision

The selected stateless path has no cross-request `TEMP` or `SET` state. The
only supported scope for those objects is one orchestrator-created
multi-statement request; cross-stage state is a regular `run_data` relation.
There is no connection-pinned lease or affinity-key reacquisition in Phase 1.

The production wrapper must also respect `duckdb-rs::Connection: Send + !Sync`.
Its retained master belongs to one blocking factory/control thread. An async
request obtains a permit, asks that thread to clone, moves the clone to one
blocking query worker, then drops it before returning the permit. A connection
is never shared by concurrent queries and is never wrapped in
`Arc<Connection>`.

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
| Listener/`ready.json` availability (not authenticated worker readiness) | 230 ms | 330 ms |
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

The selected stateless implementation was run on 2026-07-17 with three local
workers limited to 128 MB each and `quack_parallelism = 8`:

| Measure | Result |
|---|---:|
| Isolated worker, stateless health-ready | 1,432 ms |
| Single cold worker query-readiness in pool probe | 1,450 ms |
| Three workers prewarmed concurrently | 1,542 ms |
| Warm exclusive checkout | 0 µs |
| Replacement query-ready after lease release | 1,297 ms |
| Queued fourth pipeline admitted through replacement | 1,457 ms |
| Precreated clones per ready worker | 0 |
| Attach aliases per ready worker | 0 |

The old 227–289 ms figures measured only sidecar listener/`ready.json`
availability and must not be interpreted as worker readiness. The corrected
probe stores the client master inside `WarmWorker` and publishes the worker
after one authenticated stateless health query. Clones are created on demand. It
verified distinct worker PIDs, no catalog leakage between two pipelines,
bounded blocking at fixed capacity, termination of every consumed worker and
publication of replacements only after that complete handshake. All PIDs
present after replenishment differed from the consumed workers.

The first 8-slot lifecycle attempt exposed a destructor-order defect in the
spike. Killing the sidecar before dropping sticky clients caused each client to
wait against a dead server: the isolated worker was internally ready in
1,519 ms but the command took 86.8 s to exit, and `pool-smoke` exceeded 180 s.
Dropping client connections before the process guard removed that delay. The
selected stateless worker has no sticky attachments, but the production owner
must preserve explicit connection-before-process cleanup on normal shutdown;
cancellation must not wait for graceful client detach.

This probe represents only the future `LocalProcessProvider` and
`WorkerPoolControl`. The latter requires a bounded elastic policy with a hard
maximum and global resource budget. Its saturation must queue rather than
create an unbounded on-demand worker. This is separate from `QuackPermitGate`,
whose cancellable waiters bound requests inside one already leased worker.

## Elastic policy review and Kubernetes portability

An existing C# elastic worker pool was reviewed as design input for
`WorkerPoolControl`, not as code to port.
Its useful ideas are base capacity, 70% growth threshold, coalesced growth,
quarter-base growth steps, peak-based scale-in and deferred release of active
resources. Duckle must change three aspects:

1. add `max_capacity` plus global RAM/CPU/disk reservation, including workers
   still starting;
2. remove the unlimited on-demand bypass used when `WorkerPoolControl` is saturated;
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
- [Quack reverse proxy](https://duckdb.org/docs/current/quack/setup/reverse_proxy)
- [Quack deployment](https://duckdb.org/docs/current/quack/setup/deployment)
- [Quack on WebAssembly](https://duckdb.org/docs/current/quack/setup/quack_wasm)
- [DuckDB configuration reference](https://duckdb.org/docs/stable/configuration/overview)
- [DuckDB connection and thread guidance](https://duckdb.org/docs/current/clients/c/connect)
- [`duckdb-rs::Connection::try_clone`](https://docs.rs/duckdb/latest/duckdb/struct.Connection.html#method.try_clone)
- [Quack release and concurrency benchmark](https://duckdb.org/2026/05/12/quack-remote-protocol)

## Open gates before production migration

1. Bundle the pinned Quack extension for offline startup; do not depend on
   transparent or first-run downloads.
2. Validate Windows arm64, Linux x64/arm64 and macOS targets.
3. Decide how the Rust minimum-version increase is handled.
4. Run the full 1M/10M/100M and 2/4/8-client performance matrix with p50/p95,
   CPU, RSS, disk bytes and loopback bytes. The 2/4-connection overlap and
   attachment-identity probe above is complete but is not a workload benchmark.
5. Measure fan-out crossover: one Quack stream per consumer versus one shared
   Parquet snapshot.
6. Test types, nulls, decimals, timestamps, nested values and zero-row results.
7. Validate disk exhaustion through `max_temp_directory_size` and free-space
   checks.
8. Add parent-death containment using a Windows Job Object and Unix process
   group/parent monitor; RAII only covers normal parent unwinding.
9. Replace the PoC child-environment bootstrap with inherited bootstrap/control
   pipes, then verify absence of the token from filesystem, argv, post-bootstrap
   environment, logs, errors, history, profiler and exported SQL.
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
path. `LocalProcessProvider` must satisfy the worker bootstrap security ADR
before the first production route. Kubernetes itself is not a Phase 1
deliverable; preserving this provider boundary is.
