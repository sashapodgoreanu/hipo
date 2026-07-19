# ADR: DuckDB per-run sidecar con protocollo Quack

## Status

Proposed — Phase 0 produced a conditional go for the Phase 1 abstraction, but
not for production replacement of the CLI. See the
[Phase 0 report](quack-sidecar-phase-0-report.md).

This ADR is not accepted until the spike and benchmark gates below pass. Until
then, the DuckDB CLI remains the production backend.

## Feature 003 amendment — authoritative Phase 1 policy

The following rules supersede every non-historical reference in this ADR to a
bounded worker pool, hard worker maximum, admission queue, backpressure,
70-percent threshold, or incremental growth step. Historical Phase 0 evidence
is retained for context only and is not an implementation contract.

- Every pipeline run calls `WorkerPoolControl` exactly once. The controller
  atomically leases a `ready` warm worker or immediately provisions and assigns
  a dedicated single-use on-demand worker. Neither a run nor an orchestrator
  may select or spawn a worker directly.
- There is no worker/pipeline budget, hard maximum, admission queue, or
  backpressure. A run without `ready` capacity waits only for the authenticated
  handshake of the worker decided by the controller.
- Warm target is evaluated every five seconds as
  `max(base_capacity, ceil(peak_5_minutes * 1.20))`; base defaults to 3, the
  peak window is five minutes, and every on-demand-served run contributes once
  and fully to demand. On-demand workers are never warm capacity and terminate
  with their run.
- Scale-in terminates only `ready` workers. Leased workers are not interrupted,
  resized, or replaced when above the recalculated target. Peak/extra target is
  not persisted across restart.
- Quack remains test/compatibility-only until the Feature 003 cutover evidence
  passes. The single cutover enables it for production and then removes CLI,
  affinity, and the retained Phase 0 spike together.

## Context

The current engine starts DuckDB CLI processes and communicates through
stdin/stdout or run-local marker files. Query Source affinity introduced a
persistent CLI worker, but statements are serialized and runtime compatibility
depends on component-specific classification.

The execution boundary is also visible outside the engine: desktop setup,
headless runner artifacts, scheduler, MCP, drift, branch/diff, extension
installation and CI all know about the DuckDB CLI binary.

The target product behavior requires one DuckDB database for the whole
pipeline, concurrent clients, controlled memory/spill, and process isolation
from the Duckle main process.

## Proposed decision

Each pipeline run owns one dedicated `duckle-db-runner` sidecar process.

- Runs acquire a prewarmed worker through an exclusive, single-use lease. The
  worker is terminated after completion or cancellation and is never returned
  to `WorkerPoolControl`'s ready set.
- `WorkerPoolControl` applies a bounded elastic policy for desired capacity and
  admission. It counts
  starting, ready and leased workers, never bypasses the hard maximum with an
  unbounded on-demand creation path, and replenishes only to its current target.
- Elastic policy and queueing are independent from a `WorkerProvider`.
  `LocalProcessProvider` is implemented first; a future Kubernetes provider
  must satisfy the same state, readiness, lease and termination contract.
- The sidecar embeds DuckDB through the Rust client library.
- The sidecar starts a Quack server bound to localhost.
- Duckle main or `duckle-runner` embeds a client-only DuckDB instance and acts
  as a Quack client.
- The existing orchestrator remains responsible for query boundaries, SQL
  batching, stage events, retries and parallel dispatch. `RunDatabase` replaces
  only the low-level CLI transport and must not eagerly submit, merge, split or
  reorder pipeline work.
- Complete ordinary SQL statements and server-setup commands are sent with
  `quack_query` so analytical operators and Data Source setup execute in the
  sidecar.
- The sidecar owns the run catalog, relations, attachments, memory budget and
  spill directory.
- A run may use multiple Quack connections to the same sidecar.
- `quack_parallelism` configures the stateless-query semaphore for the one
  sidecar owned by a pipeline run and defaults to `8`. It is the maximum number
  of queries sent concurrently to that sidecar, not the number of sidecars in
  `WorkerPoolControl` and not a limit on requested pipeline runs. Feature 003
  makes it part of the workspace resource profile, resolved to an effective
  value under host/pool and future license policy; it remains independent from
  DuckDB `threads`.
- Parquet remains an explicit fallback transport where benchmarks show it is
  preferable or a runtime cannot use Quack.
- Cancelling a run terminates its process scope, including the sidecar. There
  is no requirement to cancel one DuckDB statement while keeping the run alive.
- SlothDB and `xf.dbt` are disabled during this initiative and are not migrated
  to Quack.

No additional REST/JSON data API is introduced. Process spawning and the
provider-owned bootstrap/control channel are lifecycle mechanisms, not a second
query protocol.

## Ownership boundary

### Duckle main / headless orchestrator

Owns planning, DAG readiness, stage events, history, retry policy, runtime
processes, worker admission/lease and final cleanup. Local process handles or
future Pod references are owned by the selected provider, not by the scheduler.
The main must not open the pipeline database locally or execute its
joins/materializations.

### `WorkerPoolControl`

`WorkerPoolControl` owns the worker state machine, elastic target and atomic
`ready -> leased` assignment. It has no admission queue or worker/pipeline
budget: absence of `ready` capacity selects immediate per-run on-demand
provisioning. A listener or `ready.json` is only infrastructure-ready and is not
acquirable. The published worker is a warm bundle containing the sidecar, one
client-only DuckDB database, its retained master connection and scoped Quack
secret. One authenticated stateless health query must pass before the bundle
changes to `ready`. Clone connections are not precreated. Capacity includes workers still starting so a slow bootstrap
cannot trigger duplicate provisioning.

At lease release the assigned worker is always terminated. Scale-in destroys
only idle ready workers; excess leased workers are marked not to be replenished
when they terminate. Scale-in observations use renewable windows so a historic
peak cannot prevent later contraction forever.

### Worker provider

Owns provisioning, observation, endpoint discovery and idempotent termination
for an opaque worker reference. The first provider maps these operations to a
local process, Job Object/process group, localhost endpoint and run directory.
A future Kubernetes provider preferably maps them to an ephemeral Kubernetes
Job with one Pod, readiness probe, network endpoint, Secret/bootstrap and
bounded `emptyDir` spill volume. PID, port, filesystem path, Job name, Pod name
and Pod IP do not enter the scheduler contract. Termination deletes the owning
Job rather than only its Pod, preventing workload-controller replacement.

### `duckle-db-runner`

Owns the DuckDB instance and executes all DuckDB work. It configures extensions,
connections, catalog, memory, spill and Quack before reporting ready.

### Quack client wrapper and `QuackPermitGate`

Owns one worker-scoped client-only DuckDB database, a master connection kept
alive from prewarm until worker termination, and a per-worker semaphore named
`QuackPermitGate`, bounded by `quack_parallelism`. Ordinary execution acquires a permit, calls
`Connection::try_clone()`, executes stateless `quack_query` with a parameterized
scoped secret, then drops the clone and releases the permit. It creates no
client-side Quack attachment. If no permit is available, the request waits; it
does not use a connection pool. The raw
connections remain private; callers receive typed remote
execution/query/import/export methods implemented exclusively with
`quack_query(uri, sql)`. `remote.query(...)` and `quack_query_by_name(...)` are
not part of the production wrapper. This prevents accidental local execution
over remote scans.

`WorkerPoolControl` is the only entity called a *pool*: it contains warm
sidecar workers between runs. `QuackPermitGate` is not a pool and contains no
connections or members. Its permits bound concurrent stateless requests within
one leased worker. In Phase 1 `WorkerSpec.quack_parallelism` is configurable in
the validated range `1..=8`, with default and hard maximum `8`. Raising that
maximum requires a new concurrency benchmark and ADR update; `8` is a tested
initial limit, not a throughput SLO.

The gate has FIFO, cancellable waiters. Cancelling a pending stage/run removes
its waiter without creating a clone or a worker. It must not keep an
independent unbounded queue: the DAG orchestrator retains stage admission and
the gate merely waits after a request has been submitted. On clone-factory or
query failure, the clone is dropped if created and the permit is released; the
typed result goes to the orchestrator, which remains responsible for retry.
A sidecar failure makes the worker unhealthy and ends its leased run;
`WorkerPoolControl` decides whether to provision a replacement. There is no
member-failure, eager-creation or bounded-growth policy for client connections,
because no client connection is retained other than the master.

### TEMP state and connection ownership

The ordinary stateless contract deliberately selects the first option for
temporary state: `TEMP` tables and `SET` values never cross execution units.
If several statements need them, the unchanged orchestrator sends those
statements as one remote multi-statement batch (`RequestLocal`). Every
cross-stage result must be a regular sidecar relation in `run_data`. There is
no connection-pinned lease, affinity key or pool-member reacquisition in the
first implementation.

`duckdb-rs::Connection` is `Send` but not `Sync`. Consequently the client
runtime has these non-negotiable ownership rules:

1. The retained master is owned by one dedicated blocking factory/control
   thread; it is never placed in `Arc<Connection>` or used concurrently.
2. An async request acquires a `quack_parallelism` permit and asks that factory
   to call `try_clone()`. The factory serializes only this short operation.
3. The resulting clone is moved to one blocking query worker and is exclusively
   owned for the complete `quack_query` call. Two queries never use one
   `Connection` concurrently.
4. The query worker drops the clone before releasing the permit. Cancellation
   abandons pending work and terminates the worker process; it must not wait on
   a shared client connection.

The public async API is therefore a message/worker boundary over blocking
DuckDB calls, not an `Arc` around a connection. A future feature that genuinely
requires state across calls must introduce a separately specified pinned-session
resource; it cannot silently alter this stateless contract.

### Two different meanings of `ATTACH`

`ATTACH` in this architecture names two unrelated resources and they must never
share one abstraction or deduplication key.

1. **Client transport attachment**: `ATTACH 'quack:…' AS … (TYPE quack)` is an
   attachment in the client-only DuckDB database. It creates a sticky Quack
   session. It is characterized by the spike but is not used in the ordinary
   stage path, which uses stateless `quack_query`.
2. **Server Data Source attachment**: for example `ATTACH '<Postgres DSN>' AS
   sales (TYPE postgres)`, `ATTACH 'ducklake:…' AS lake`, or a DuckDB/SQLite
   catalog attachment. This command must execute in the **sidecar** DuckDB
   catalog. It is pipeline data setup, not transport setup.

The planner/orchestrator remains the authority that resolves Data Sources and
decides which server setup commands are required and in what order. This is the
same responsibility currently represented by Query Source attachment preludes.
The client master exposes a serialized `ensure server setup` operation that
sends the already-resolved SQL verbatim through `quack_query` before the
dependent node; it does not parse SQL, decide attachments or attach a source
locally. It deduplicates by the planner-provided resource/alias identity only
within one worker run. The setup gate must complete before parallel stages that
depend on that resource are admitted.

S3 is normally not a server `ATTACH`: the setup is a server-side `CREATE
SECRET (TYPE s3)`/`httpfs` configuration and the stage reads `s3://…` through
DuckDB table functions. DuckLake, Postgres, MySQL and file/catalog sources do
use server `ATTACH` where their DuckDB extensions require it.

### Verified stateless client concurrency semantics

The `clone-attach-smoke` probe executed on Windows x64 on 2026-07-17 establishes
the stateless-concurrency contract:

| Client topology | 2 × 250 ms | 4 × 250 ms | 8 × 250 ms | IDs at 8 |
|---|---:|---:|---:|---:|
| clones, one shared `ATTACH` alias | 517–527 ms | 1,023–1,046 ms | 2,069–2,070 ms | 1 |
| clones, distinct aliases on one master database | 262–267 ms | 266–268 ms | 270–271 ms | 8 |
| clones, stateless `quack_query` | 265–266 ms | 266–269 ms | 272–277 ms | 8 |
| independent client databases | 262–275 ms | 264–275 ms | 282–286 ms | 8 |

`Connection::try_clone()` does inherit the already-open client database,
including visibility of the Quack attachment and its sticky server-side state.
It does not turn one attachment into multiple remote sessions. Queries from
different clones through the same alias share one `quack_connection_id` and
serialize. Distinct aliases created on the same master database produce
distinct server sessions and real 2/4/8-way overlap. More importantly, stateless
`quack_query` on clones produces the same overlap and one server connection ID
per concurrent request without any attachment. Independent client databases
and preattached aliases are therefore unnecessary for ordinary stage SQL.

The same spike executes a **server-side** DuckDB file `ATTACH` through a
stateless `quack_query`, then reads the attached catalog from a later stateless
request. The later request returns the expected value (`42`), proving that the
Data Source attachment lives in the sidecar catalog rather than in the
transient Quack request. This is the required behavior for planner-owned Query
Source setup; it does not imply use of the discarded client transport
attachment.

The selected design therefore creates one local clone on demand per admitted
execution unit. It invokes `quack_query` with the scoped secret, executes
exactly the single stage request or pre-existing batch supplied by the
orchestrator, then drops the clone. Session/TEMP state is pinned to that
single request only; cross-stage state uses regular `run_data` relations or one
orchestrator-created multi-statement batch. `TEMP`/`SET` state across separate
stage calls is not part of the normal execution contract.

`QuackPermitGate` capacity bounds concurrent remote requests but does not decide which
stages are parallel. DuckDB's `threads` setting independently controls internal
parallelism within a query; it is not a substitute for multiple concurrent
Quack requests.
The same probe executes two dependent materializations and a final query as one
remote multi-statement request, leaving the expected table and final value. It
does not prove transaction atomicity, rollback after an intermediate failure,
marker compatibility, preview behavior or per-stage failure attribution; all
remain Phase 2 gates.

The rejected sticky-slot alternative was also characterized. Three profiled
Windows x64 runs measured the initial `ATTACH` at 1.11–1.21 s. Across 60 exact
`try_clone -> ATTACH on clone -> first query` sequences, cloning took 18–47 µs
and the complete path took 8.99–15.39 ms. These results prove that clones can
reuse the master's parameterized secret, but the 8-way stateless result above
removes the need to pay or manage this attachment cost for ordinary execution.
One earlier
unprofiled total-bootstrap sample reached 3.39 s, so these are characterization
results, not an SLO.

This resolves the two-pool question: `WorkerPoolControl` is the only elastic
pool. Every worker is a self-contained warm bundle with its local master and a
`QuackPermitGate`. The initial default and Phase 1 hard maximum are `8`, but a
ready worker owns zero precreated clones and zero Quack attachments. A ninth
concurrent request waits cancellably for a permit. Admitted execution pays the
measured 18–96 µs `try_clone()` cost and never performs `ATTACH`; there is no
independently elastic connection pool.

## Run lifecycle

1. `WorkerPoolControl` provisions its base capacity through the selected provider.
2. The sidecar listener becomes infrastructure-ready, but remains `starting`.
3. The worker creates and retains its client database/master, creates the scoped
   secret, then completes one authenticated stateless readiness query.
4. Only this complete warm bundle is published as `ready`.
5. A run request atomically leases a ready worker or receives a dedicated
   on-demand worker selected and provisioned by the controller.
6. Each dispatched request acquires one of the 8 default permits, clones the
   master locally, executes stateless SQL, then drops the clone and releases the
   permit. Excess requests wait.
7. Normal completion reads final metrics and releases the lease.
8. Cancellation terminates the leased worker immediately.
9. The provider removes process/Pod storage, spill, snapshots and bootstrap
   artifacts.
10. `WorkerPoolControl` provisions a replacement only if required by its elastic target;
   the replacement becomes available only after its own readiness handshake.

## Phase 0 hypotheses

The ADR can move to Accepted only if the spike demonstrates all of the
following on supported development platforms:

1. `duckdb-rs` can load the required Quack version in both client and server
   builds without invoking DuckDB CLI.
2. A client can send a complete CTAS/query to the sidecar and the heavy work is
   measured in the sidecar rather than the client.
3. Two or more Quack connections can read concurrently from the same run
   database.
4. Concurrent append behavior and conflicting mutation behavior are observed
   and can be mapped to deterministic scheduling/retry rules.
5. Connection-sticky state and Data Source `ATTACH` visibility are characterized
   rather than assumed.
6. The chosen in-memory/file-backed candidates spill to the configured run
   directory and respect practical memory/disk budgets.
7. Killing the sidecar interrupts active queries promptly and the orchestrator
   can distinguish cancellation from an unexpected crash.
8. Client/server version mismatch fails during handshake.
9. Tokens, connection secrets and sensitive SQL are absent from persisted logs,
   ready files and error payloads.
10. The client/server binary-size, startup, RSS and latency costs are measured
    against the CLI baseline.

## Phase 0 deliverables

- isolated sidecar and client PoC, not wired into production execution;
- repeatable scripts/tests for query, write, concurrency, attachment, spill and
  kill;
- benchmark report comparing CLI, Quack and Parquet transfer where relevant;
- extension/version/packaging report for Windows, Linux and macOS targets;
- documented result for each hypothesis above;
- updated ADR status: Accepted, Rejected, or Superseded with evidence.

## Acceptance gates for the architecture

- The main process RSS must not scale with the full database size for remote SQL
  that returns only small metadata.
- Work exceeding the configured memory budget must complete through bounded
  spill rather than OOM.
- Cancellation must terminate the sidecar promptly and internal cleanup must
  finish within 10 seconds.
- No synthetic secret used by the test suite may appear in persisted output.
- The benchmark must establish the Quack/Parquet crossover instead of assuming
  one transport always wins.
- Active CLI consumers other than disabled SlothDB/dbt must have an identified
  migration path before production cutover.

Numeric performance thresholds will be set after recording the CLI baseline on
the same hardware.

## Alternatives considered

### Embedded DuckDB directly in the main process

Rejected because DuckDB CPU, memory pressure and native crashes would affect
the UI/orchestrator process and cancellation could not reclaim the full database
by terminating one isolated owner.

### Custom REST/JSON server

Rejected because it duplicates database protocol, loses DuckDB types, adds a
second serialization design and does not provide the intended DuckDB-to-DuckDB
client model.

### Continue the persistent CLI affinity worker

Rejected as the target architecture because stdin framing remains serialized,
component-specific affinity remains necessary and multiple coordinated
connections are not available.

### Parquet-only process boundary

Retained as fallback, rejected as the only transport because every boundary
requires file materialization even when remote SQL or Quack streaming is more
appropriate.

### Shared long-lived DuckDB service for multiple runs

Rejected for this initiative because cancellation, cleanup, resource isolation
and secret/catalog separation become cross-run concerns.

### `WorkerPoolControl` tied directly to local child processes

Rejected because it would leak PID, ports and filesystem lifecycle into
scheduling and make a future Kubernetes deployment a rewrite. Process-specific
operations belong behind `LocalProcessProvider`.

### Kubernetes Deployment or HPA as the lease manager

Rejected as the common contract. Replica controllers and HPA manage replica
counts, not exclusive single-use pipeline leases, ready-to-leased atomicity or
delete-on-completion semantics. A future provider may use a one-worker Job as
the lifecycle owner and a custom controller/CRD at larger scale, but Duckle
remains the owner of admission and desired worker capacity.

## Consequences

### Positive

- database state has one explicit per-run owner;
- prewarm removes most sidecar startup latency while preserving per-run
  isolation;
- provider-neutral leases allow a future Kubernetes backend without changing
  pipeline scheduling or database APIs;
- DuckDB work is isolated from the main process;
- multiple server connections can support safe parallel scheduling;
- Query Source affinity can become resource-based rather than component-based;
- cancellation has a simple process-level terminal action;
- one IPC model can serve desktop, headless and external runtimes.

### Negative

- Quack currently requires DuckDB in both client and server processes;
- binary size, build time and baseline RSS may increase;
- Quack is beta and client/server must be pinned and upgraded together;
- extension offline packaging becomes a release requirement;
- all current path-based database helpers require migration;
- localhost process startup can be affected by antivirus, firewall or port
  policy;
- process groups/Job Objects and orphan cleanup require platform-specific code.
- a bounded queue, global resource budget and elastic control loop add control
  plane complexity;
- Kubernetes requires distributed atomic lease ownership, endpoint security and
  a clear single owner for desired capacity.

## Security constraints

The normative security contract is defined by
[Worker identity, bootstrap, and connection security](adr-worker-identity-bootstrap-security.md).
In summary:

- scheduling receives one opaque `VerifiedWorker`, not a separately exposed
  endpoint and token;
- `LocalProcessProvider` uses a random per-worker capability delivered through
  explicitly inherited anonymous-pipe handles;
- the target implementation does not persist bootstrap secrets or use
  `ready.json`;
- only a successful authenticated identity/protocol handshake can publish a
  worker as ready;
- the token is never passed in command-line arguments, environment, logs,
  errors, history, UI events or exported SQL;
- user code and external runtimes never receive the raw worker credential;
- the master client creates a parameterized temporary Quack secret; admitted
  stages clone on demand and call `quack_query` without repeating the token and
  without a Quack `ATTACH`;
- the execution profile intentionally permits full SQL only to the trusted
  supervisor and cannot be reused by a browser/publication client;
- termination destroys the worker and revokes its credentials;
- endpoint transport security can evolve without changing the lease contract.

Future Book publication is an architectural constraint only and remains out of
scope. It requires a distinct publication database/process, security profile
and lifecycle; it must never expose a live pipeline worker or its capability.

## Compatibility and rollout

The PoC is isolated behind a new crate/binary and cannot replace production
execution in Phase 0. A later `RunDatabase` abstraction will allow CLI and Quack
backends to coexist during migration. Removing CLI download, `AffinitySession`
or compatibility code is explicitly deferred until all active consumers pass
their migration gates.

The compatibility boundary also preserves the existing planner/orchestrator
behavior. An existing SQL batch remains one backend request and a query emitted
per stage remains one request; changing from CLI to Quack is not authorization
to compile or submit the whole pipeline eagerly.

Existing pipeline documents remain readable. Documents selecting disabled
SlothDB or containing `xf.dbt` fail with explicit `engine_disabled` or
`component_disabled` diagnostics and never silently fall back.

## Implemented controller diagnostics

The controller records only opaque worker/run/lease identities. Warm workers
move through `starting`, `ready`, `leased`, `terminating` and `terminated`;
only `ready` may be leased. A run without a ready worker receives a dedicated
on-demand worker that is never published as warm capacity and terminates with
the run.

The desired warm capacity is evaluated at startup, on a complete profile save,
and every five seconds. It is `max(base_capacity, ceil(peak_5m * 1.20))`;
the five-minute peak resets with the controller. Scale-in selects ready workers
only, never a leased worker. Autoscale telemetry includes the evaluation
reason, outcome, demand, peak, current/target/base capacity, warm-state counts
and provision/termination totals. It intentionally excludes endpoint, port,
PID, path, token, secret, SQL and capability.

A complete `RunnerResourcesProfile` is versioned. A save becomes the desired
generation atomically; starting workers converge before publication, ready
workers apply the newest generation, and leased sessions drain active queries
before applying the latest profile. A failed apply preserves the prior
effective generation and exposes only `configuration_apply_failed`.

## Related documents

- [Quack sidecar feature intent](../feature-intents/003-quack-sidecar-database-runner.md)
- [Worker identity, bootstrap, and connection security](adr-worker-identity-bootstrap-security.md)
- [Deferred Query Source / multi-input Query intent](../feature-intents/002-universal-query-source-and-multi-input-query.md)
- [Current CLI affinity ADR](adr-affinity-session.md)
