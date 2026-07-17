# Quack sidecar spike

This standalone crate validates Phase 0 of the proposed per-run DuckDB sidecar.
It does **not** replace Duckle's production CLI backend.

Both processes embed the exact same DuckDB version (`1.5.4`):

- `server` opens the run database and exposes it only on `127.0.0.1` through
  Quack;
- `query` embeds a client-only DuckDB instance and sends complete SQL statements
  through `quack_query`;
- `smoke` starts a real child sidecar and validates remote SQL, concurrent
  reads, concurrent appends, server-side and client-side attachments, spill and
  process-kill cancellation.
- `pool-smoke` is a local-process-only lifecycle probe. It validates exclusive
  per-run leases, bounded queueing, single-use kill-and-replace workers and
  publication only after a replacement is query-ready. It is not the target
  elastic policy or a production supervisor.
- `benchmark` compares full-relation Quack transfer with a reusable Parquet
  snapshot for both in-memory and temporary file-backed sidecars.

Quack is installed explicitly by the spike if it is not already cached. This
requires network access once. Offline packaging of the pinned extension is a
separate release gate and is intentionally not solved by this PoC.

```powershell
cargo run --manifest-path spikes/quack-sidecar/Cargo.toml --release -- smoke
cargo run --manifest-path spikes/quack-sidecar/Cargo.toml --release -- pool-smoke
cargo run --manifest-path spikes/quack-sidecar/Cargo.toml --release -- benchmark
```

Manual use:

```powershell
cargo run --manifest-path spikes/quack-sidecar/Cargo.toml --release -- server --ready C:\temp\quack-ready.json --port 19494
cargo run --manifest-path spikes/quack-sidecar/Cargo.toml --release -- query --ready C:\temp\quack-ready.json --sql "SELECT 42"
```

The ready file is bootstrap metadata, not a data transport. Query data only
travels through Quack. The server generates the token and stores it in the
ready file; callers must keep the containing run directory private and delete
it after the process exits.

The production design keeps admission and elastic decisions independent from
worker provisioning. This PoC directly owns child-process guards only to test
the future `LocalProcessProvider`; a Kubernetes provider will expose the same
opaque worker identity, endpoint, readiness and idempotent termination
semantics without leaking Pod details into pipeline scheduling.
