# Quack sidecar spike

This standalone crate validates Phase 0 of the proposed per-run DuckDB sidecar.
It does **not** replace Duckle's production CLI backend.

Both processes embed the exact same DuckDB version (`1.5.4`):

- `server` opens the run database and exposes it only on `127.0.0.1` through
  Quack;
- `query` embeds a client-only DuckDB instance and sends complete SQL statements
  through a temporary scoped secret and Quack;
- `smoke` starts a real child sidecar and validates remote SQL, concurrent
  reads, one measured two-writer append workload, server-side and client-side
  attachments, spill and process-kill cancellation.
- `pool-smoke` is a local-process-only lifecycle probe. It validates exclusive
  per-run leases, bounded queueing, single-use kill-and-replace workers and
  publication only after a replacement is query-ready. It is not the target
  elastic policy or a production supervisor.
- `clone-attach-smoke` verifies `Connection::try_clone()` sticky-state
  inheritance and measures 1/2/4/8-way execution for a shared attachment,
  distinct attachment aliases on one master client database, and independent
  client databases. It also measures 1/2/4/8-way stateless `quack_query` on
  clones and counts distinct server `quack_connection_id` values, so elapsed
  time is not the only parallelism signal.
- `benchmark` compares full-relation Quack transfer with a reusable Parquet
  snapshot for both in-memory and temporary file-backed sidecars.

Quack is installed explicitly by the spike if it is not already cached. This
requires network access once. Offline packaging of the pinned extension is a
separate release gate and is intentionally not solved by this PoC.

```powershell
cargo run --manifest-path spikes/quack-sidecar-phase0-spike/Cargo.toml --release -- smoke
cargo run --manifest-path spikes/quack-sidecar-phase0-spike/Cargo.toml --release -- pool-smoke
cargo run --manifest-path spikes/quack-sidecar-phase0-spike/Cargo.toml --release -- clone-attach-smoke
cargo run --manifest-path spikes/quack-sidecar-phase0-spike/Cargo.toml --release -- ready-worker-smoke
cargo run --manifest-path spikes/quack-sidecar-phase0-spike/Cargo.toml --release -- benchmark
```

The clone/attachment probe demonstrates that clones using one shared alias are
serialized by the single sticky Quack server connection. Distinct `ATTACH`
aliases created once on the same master client database use distinct server
connections and execute the 2/4/8-query probes in parallel. Stateless
`quack_query` on cloned local connections produces the same 8-way overlap with
8 distinct server connection IDs and no `ATTACH`. The selected production
shape therefore keeps one client master and a semaphore. The configurable
`quack_parallelism` default is 8, but clones are created on demand because
`try_clone()` measured only 18–96 µs. Readiness contains zero precreated clones
and zero attachment aliases. The only elastic pool is the worker pool.
The same command verifies a dependent three-statement remote batch and its
final result, proving ordered execution for that sample without moving batching
decisions into the Quack backend. It does not prove batch atomicity, rollback,
marker compatibility or per-stage failure attribution.
It also reports microsecond-resolution timings for master-client bootstrap,
initial `ATTACH`, 20 additional warm-master `ATTACH` operations, first query and
warm query. These characterize the rejected sticky topology and isolate the
18–96 µs clone cost that supports on-demand stateless clones.
The same probe distinguishes the rejected client `ATTACH … TYPE quack` from a
server-side Data Source `ATTACH`: it attaches a DuckDB file through stateless
Quack and reads it successfully from a later stateless request.

Manual use:

```powershell
$env:DUCKLE_QUACK_SPIKE_BOOTSTRAP_TOKEN = '<64 lowercase hex characters>'
cargo run --manifest-path spikes/quack-sidecar-phase0-spike/Cargo.toml --release -- server --ready C:\temp\quack-ready.json --port 19494
cargo run --manifest-path spikes/quack-sidecar-phase0-spike/Cargo.toml --release -- query --ready C:\temp\quack-ready.json --sql "SELECT 42"
```

The ready file is token-free bootstrap metadata; ordinary SQL requests and
direct query results use Quack. Parquet remains an explicit fallback for
relation transfer. The supervisor-generated 256-bit token is supplied to the PoC
child through a child-only environment entry and removed by the child before
DuckDB starts. This is better than argv or `ready.json`, but remains
**non-production bootstrap**. The target
[worker bootstrap security contract](../../docs/architecture/adr-worker-identity-bootstrap-security.md)
uses explicitly inherited anonymous pipes and publishes a worker only after an
authenticated identity/protocol handshake. Do not reuse the spike environment
bootstrap in production code.

The server declares the `execution_trusted_full_sql_v1` profile: token
authentication plus intentionally full SQL authorization for the trusted
pipeline supervisor. It is not suitable for browsers, Books, user code or any
public endpoint.

The production design keeps admission and elastic decisions independent from
worker provisioning. This PoC directly owns child-process guards only to test
the future `LocalProcessProvider`; a Kubernetes provider will expose the same
opaque worker identity, endpoint, readiness and idempotent termination
semantics without leaking Pod details into pipeline scheduling.
