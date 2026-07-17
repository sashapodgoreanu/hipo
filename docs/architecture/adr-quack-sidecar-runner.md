# ADR: DuckDB per-run sidecar con protocollo Quack

## Status

Proposed — Phase 0 produced a conditional go for the Phase 1 abstraction, but
not for production replacement of the CLI. See the
[Phase 0 report](quack-sidecar-phase-0-report.md).

This ADR is not accepted until the spike and benchmark gates below pass. Until
then, the DuckDB CLI remains the production backend.

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
  to the ready pool.
- A bounded elastic policy controls desired capacity and admission. It counts
  starting, ready and leased workers, never bypasses the hard maximum with an
  unbounded on-demand creation path, and replenishes only to its current target.
- Elastic policy and queueing are independent from a `WorkerProvider`.
  `LocalProcessProvider` is implemented first; a future Kubernetes provider
  must satisfy the same state, readiness, lease and termination contract.
- The sidecar embeds DuckDB through the Rust client library.
- The sidecar starts a Quack server bound to localhost.
- Duckle main or `duckle-runner` embeds a client-only DuckDB instance and acts
  as a Quack client.
- Complete SQL statements are sent with `quack_query` or the attached remote
  catalog's `query(...)` macro so analytical operators execute in the sidecar.
- The sidecar owns the run catalog, relations, attachments, memory budget and
  spill directory.
- A run may use multiple Quack connections to the same sidecar.
- Parquet remains an explicit fallback transport where benchmarks show it is
  preferable or a runtime cannot use Quack.
- Cancelling a run terminates its process scope, including the sidecar. There
  is no requirement to cancel one DuckDB statement while keeping the run alive.
- SlothDB and `xf.dbt` are disabled during this initiative and are not migrated
  to Quack.

No additional REST/JSON data API is introduced. Process spawning and the
protected bootstrap/ready files are lifecycle mechanisms, not a second query
protocol.

## Ownership boundary

### Duckle main / headless orchestrator

Owns planning, DAG readiness, stage events, history, retry policy, runtime
processes, worker admission/lease and final cleanup. Local process handles or
future Pod references are owned by the selected provider, not by the scheduler.
The main must not open the pipeline database locally or execute its
joins/materializations.

### Worker pool control plane

Owns a cancellable FIFO admission queue, the worker state machine, bounded
elastic target, global resource reservations and atomic `ready -> leased`
assignment. Only workers that passed infrastructure readiness and a Quack
application handshake are published. Capacity includes workers still starting
so a slow bootstrap cannot trigger duplicate provisioning.

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

### Quack client wrapper

Owns the local client-only DuckDB connection. The raw connection remains
private; callers receive typed remote execution/query/import/export methods.
This prevents accidental local execution over remote scans.

## Run lifecycle

1. The pool provisions its bounded base capacity through the selected provider.
2. Provider readiness plus the protocol/DuckDB/Quack handshake publishes a
   worker as ready.
3. A run request acquires an exclusive worker lease or waits in the bounded
   admission queue.
4. The run executes through the leased worker's Quack endpoint.
5. Normal completion reads final metrics and releases the lease.
6. Cancellation terminates the leased worker immediately.
7. The provider removes process/Pod storage, spill, snapshots and bootstrap
   artifacts.
8. The pool provisions a replacement only if required by its elastic target;
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

### Pool tied directly to local child processes

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

- localhost binding for `LocalProcessProvider`; a future Kubernetes provider
  requires authenticated in-cluster endpoints, NetworkPolicy and transport
  protection appropriate to the cluster threat model;
- random per-run token;
- token never passed in command-line arguments or written to `ready.json`;
- protected bootstrap payload deleted after use;
- no remote/network deployment in this feature;
- default Quack authorization reviewed before exposing the token to user code;
- secret-bearing attachments preferably initialized inside the sidecar rather
  than shipped as SQL through the client;
- logs and errors redacted before persistence or UI emission.

## Compatibility and rollout

The PoC is isolated behind a new crate/binary and cannot replace production
execution in Phase 0. A later `RunDatabase` abstraction will allow CLI and Quack
backends to coexist during migration. Removing CLI download, `AffinitySession`
or compatibility code is explicitly deferred until all active consumers pass
their migration gates.

Existing pipeline documents remain readable. Documents selecting disabled
SlothDB or containing `xf.dbt` fail with explicit `engine_disabled` or
`component_disabled` diagnostics and never silently fall back.

## Related documents

- [Quack sidecar feature intent](../feature-intents/003-quack-sidecar-database-runner.md)
- [Deferred Query Source / multi-input Query intent](../feature-intents/002-universal-query-source-and-multi-input-query.md)
- [Current CLI affinity ADR](adr-affinity-session.md)
