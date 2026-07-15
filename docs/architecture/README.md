# Duckle architecture notes

This directory records the verified brownfield architecture. It describes the
current implementation; it is not a target-state design.

Detailed views:

- [System overview](system-overview.md)
- [Domain model](domain-model.md)
- [Execution model](execution-model.md)
- [Tauri IPC contracts](ipc-contracts.md)
- [Component extension guide](component-extension-guide.md)

## Workspace and runtime

The repository is a Cargo workspace containing the desktop application,
execution engine, metadata, runner, MCP server, scheduler, connectors, and
supporting engine crates. The desktop shell is in `apps/desktop/` and uses
Tauri 2. The frontend is React/TypeScript/Vite under `frontend/` and is built
with npm using `frontend/package-lock.json`.

The desktop application invokes Rust through the command registry in
`apps/desktop/src/lib.rs`. The frontend adapter is
`frontend/src/tauri-bridge.ts`; the web runner supplies a separate shim in
`frontend/src/web-shim/tauri-core.ts`.

## Domain, planning, and execution

Serializable pipeline and schema types are defined in
`crates/metadata/src/lib.rs` (`Pipeline`, `PipelineNode`, `PipelineEdge`,
`Schema`, `Column`, and `DataType`). The executable payload and compiled
representation are in `crates/duckdb-engine/src/plan/`:

- `PipelineDoc` is the planner input.
- `Stage`, `StageKind`, and `CompiledPipeline` describe compiled work.
- `RuntimeSpec` describes runtime-driven sources, sinks, controls, and other
  non-pure-SQL work.
- `graph.rs` validates data-edge dependencies and ordering.

`DuckdbEngine` in `crates/duckdb-engine/src/lib.rs` executes the compiled
pipeline through the DuckDB CLI. Eligible pure-SQL pipelines use a batched
DuckDB session; runtime/control work and other ineligible pipelines use the
per-stage path. Materialization, cancellation, retries, previews, cleanup,
and run history are execution behavior, not planner-only metadata.

## Connections, contexts, and secrets

Connections are workspace repository items represented in frontend types and
persisted separately from pipeline documents. The desktop secret service in
`apps/desktop/src/secrets.rs` encrypts sensitive connection fields with a
per-workspace AES-256-GCM key. `${...}` placeholders remain placeholders.

Context resolution is mirrored in `frontend/src/run-resolve.ts` and
`crates/duckdb-engine/src/context.rs`. Current connection selection hydrates a
payload and copies its values into node properties; there is no central
runtime `Connection` type or dynamic connection-ID resolver.

## Tauri IPC and capabilities

Tauri commands are registered in `apps/desktop/src/lib.rs` and are mirrored by
the frontend bridge. Pipeline, install, chat, and self-update operations use
Tauri channels for progress/events. Capabilities are defined in
`apps/desktop/capabilities/default.json`. The current desktop capability has a
broad filesystem scope and `apps/desktop/tauri.conf.json` leaves CSP unset;
changes that widen this surface require explicit security review.

## Verification and known gaps

The repository has Rust unit, planner, scheduler, and engine integration tests.
The frontend currently exposes type-check/build scripts but no detected
frontend or end-to-end test framework. Routine CI excludes `duckle-lance` from
workspace tests and treats fmt/clippy as informational. Engine integration
tests require `DUCKLE_DUCKDB_BIN` and, for service tests, connector-specific
environment variables.

Component contracts remain distributed across the frontend palette/manifests,
the MCP catalog, planner branches, and executor dispatch. Frontend trigger
edges are not all data edges in the Rust planner. These are current gaps, not
claims of a single centralized component or engine abstraction.
