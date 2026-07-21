# ADR: DuckDB per-run sidecar con protocollo Quack

## Status

**Accepted — direct activation approved by the feature owner on 2026-07-21.**

Duckle has one database execution route: the packaged Quack sidecar. The former
CLI/Quack migration gate, entry-point classes and benchmark prerequisite are
superseded. Historical Phase 0 measurements remain context, not runtime policy.

## Context

The previous engine launched DuckDB CLI processes and communicated through
stdin/stdout or marker files. Query Source introduced a persistent CLI worker
and affinity classification. Desktop setup, headless artifacts, scheduler, MCP,
drift and CI consequently knew about a DuckDB binary.

That architecture produced multiple execution paths, process-spawn overhead,
connection-affinity rules and configuration combinations. The target behavior
needs one database per pipeline run, concurrent clients, controlled resources,
process isolation and an offline package.

## Decision

Each pipeline run acquires one dedicated sidecar worker through
`WorkerPoolControl`.

- The controller is the only owner of provisioning, readiness, lease and release.
- A ready warm worker is leased atomically; otherwise a single-use on-demand
  worker is created and assigned immediately.
- A run or orchestrator cannot select or spawn a worker directly.
- There is no worker budget, hard maximum, admission queue or second backend.
- The warm target is evaluated every five seconds as
  `max(base_capacity, ceil(peak_5m × 1.20))`.
- Base capacity defaults to 3; the demand window is five minutes.
- On-demand workers count toward demand but never toward warm capacity.
- Scale-in terminates only ready workers and never interrupts leased workers.
- Restart begins from base capacity; peak and extra target are ephemeral.

## Execution boundary

The sidecar owns DuckDB, catalog, relations, attachments, memory, spill and the
Quack server. `RunDatabase` and `RunSession` expose typed operations to the
engine. The planner and orchestrator remain responsible for DAG readiness, SQL
batching, stage events, retries and runtime boundaries.

Ordinary SQL is sent as complete statements or multi-statement batches through
Quack. Cross-stage results are regular sidecar relations. Temporary state that
must remain local to one request is kept inside one batch. Query Source setup is
performed server-side and deduplicated per run; no affinity key or pinned CLI
worker is required.

`quack_parallelism` is a per-worker semaphore in the validated range `1..=8`.
It limits concurrent stateless requests to one leased sidecar; it is not a pool
and does not cap the number of requested pipeline runs.

## Resource profile

`RunnerResourcesProfile` contains:

- version;
- memory limit;
- CPU threads;
- spill limit and temporary location policy;
- Quack parallelism;
- base warm capacity.

The complete profile is persisted atomically. A worker reports ready only after
receiving the effective profile and passing the authenticated protocol/version
health check. A starting worker cannot publish an obsolete generation. A
leased worker drains active queries and then applies the newest coalesced
profile. Failure preserves the prior effective profile and returns
`configuration_apply_failed`.

## Security and lifecycle

- Quack binds only to loopback.
- Each worker receives a random credential through inherited bootstrap state.
- Token, endpoint, port, PID, path, SQL and capabilities never enter user-facing
  IPC, history, logs or metrics.
- Process group / Job Object containment terminates the full process tree.
- Cancellation, parent death, crash and startup failure produce deterministic,
  sanitized outcomes.
- A sweeper removes stale run artifacts after abnormal termination.
- Tauri capabilities and CSP are not expanded.

## Packaging

The DuckDB/Quack versions, checksum, license and provenance are pinned. The
extension is stored under `crates/duckle-runner/bin/` and verified during build.
The sidecar and extension are packaged as an adjacent pair and embedded in the
desktop application.

From `apps/desktop`, the single command is:

```text
cargo tauri build
```

Tauri's `beforeBuildCommand` builds the release runner binaries and frontend.
The desktop build rejects a missing, incomplete or mismatched Quack pair. No
runtime download, `--duckdb` argument, `DUCKLE_ENTRY_POINT_CLASS` or cutover
manifest is required.

## Failure contract

There is no silent fallback:

- missing or unverifiable bundle → `runner_unavailable`;
- invalid resource profile → `invalid_profile`;
- profile apply failure → `configuration_apply_failed`;
- transport loss → `runner_crashed`;
- cancellation → `cancelled`.

SlothDB and `xf.dbt` remain readable but disabled and do not select another
engine.

## Consequences

### Positive

- one runtime and one operational model;
- no CLI process-per-stage overhead;
- no affinity classification;
- deterministic package and startup behavior;
- centralized resource, security and cleanup policy;
- simpler desktop build and support diagnostics.

### Costs

- the package includes the runner and extension;
- every supported platform needs a verified pair;
- sidecar bootstrap and lifecycle are product-critical;
- features formerly implemented through direct CLI helpers must use typed
  `RunDatabase` operations.

## Validation

Required evidence is provided by unit/integration tests for pool state,
autoscaling, profile application, secret redaction, lifecycle, preview, partial
run, Query Source, concurrency and offline packaging. Frontend lint/build,
workspace clippy/tests and a real packaged desktop build complete the feature.

Performance comparison with the retired CLI may still be performed as optional
engineering analysis; it cannot enable, disable or reclassify the runtime.
