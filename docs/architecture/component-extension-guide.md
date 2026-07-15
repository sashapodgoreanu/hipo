# Component extension guide

> This is a map of the current extension points, not a claim that a plugin
> interface already centralizes all component behavior.

## Current component path

For an existing or new component ID, the observed path is:

1. Add or update the entry in `frontend/src/workflow-ui/palette-data.ts`.
2. Define properties, sections, ports, schema source and autodetect behavior
   in `frontend/src/workflow-ui/fields/component-manifests.ts` or through
   `manifest-synth.ts`.
3. Keep `DuckleNodeData`, `NodeKind`, `Column` and repository payloads aligned
   with Rust serialization.
4. Add the component branch in `crates/duckdb-engine/src/plan/` if it is not
   generic SQL; construct a `Stage` and, when needed, a `RuntimeSpec`.
5. Add runtime dispatch in `crates/duckdb-engine/src/lib.rs` or its connector
   modules for driver/HTTP/process work.
6. Define connection/context/secret behavior and redaction.
7. Add planner tests and engine tests appropriate to the side effects.
8. Update the MCP catalog only through its documented catalog-generation path
   when the component is exposed there.

There is no single Rust `Component` trait or registry. The component ID is the
join key across these locations.

## Source variants

### Source, pure SQL / DuckDB table function

Current examples include file readers and DuckDB-native scans. The manifest
defines path/options and autodetect; the planner emits SQL or an attach/table
function expression; the stage remains `runtime: None` when no helper is
needed. Verify schema inference, preview, path escaping and materialization.

### Source, driver/runtime based

For remote or driver-only sources, define a spec in
`crates/duckdb-engine/src/plan/specs.rs`, add a `RuntimeSpec` variant in
`plan/mod.rs`, and implement the executor connector. State whether the runtime
creates a local table/Parquet relation, streams rows, or only produces a side
effect. Add cancellation between pages/batches and redact credentials.

## Transform variants

### Transform, pure SQL

Properties are compiled into a SQL expression/query. Validate input count and
schema assumptions in the planner. Preserve aliases, reject/filter relations,
fan-in rules and downstream relation names. Add compile tests and at least one
execution test when SQL semantics are non-trivial.

### Transform, runtime / AI / custom code

Use a dedicated `RuntimeSpec` variant and spec struct. Document whether the
runtime replaces SQL, precedes/follows pass-through SQL, creates a relation,
or returns no relation. Current runtime families include connector/HTTP, AI,
WASM/JavaScript/Python, dbt and control operations. Define row limits,
cancellation, retries, error classification and secret handling.

## Sink variants

### Sink using DuckDB `COPY`

The planner emits `StageKind::Sink` and sink path/mode metadata. The executor
must preserve existing-target behavior, format/options, row counts, preview
semantics and cleanup. Add tests for overwrite/append/error-if-exists and
multi-consumer materialization where applicable.

### Sink using a driver or external API

Use a sink `RuntimeSpec` and a spec in `plan/specs.rs`. Document connection
fields, authentication, TLS/proxy, write mode, batching, idempotency, partial
failure and retry safety. Do not put raw credentials in generated SQL, logs or
error messages. Add service-gated integration tests only when a local fixture
cannot prove the behavior.

## Control nodes

Control IDs use `ctl.*` in the palette/planner. They may run a child pipeline,
iterate, foreach, parallelize, wait/throttle, log/warn or fail based on a
condition. The planner’s data-edge graph does not automatically include every
UI trigger edge. Before changing a control node, verify whether the edge is a
data dependency, a trigger, a reject route or a side-effect hook.

## Contract checklist

| Concern | Evidence to update |
|---|---|
| Catalog/palette | `palette-data.ts`, generated catalog if applicable |
| Properties/schema | `component-manifests.ts`, `manifest-synth.ts`, field types |
| Node data | `metadata/src/lib.rs`, `frontend/src/pipeline-types.ts` |
| Planner | `plan/mod.rs`, `plan/graph.rs`, `plan/specs.rs` |
| Stage/materialization | `Stage`, `StageKind`, alias, consumer count, partial run |
| Runtime | executor dispatch, connector module, subprocess/HTTP behavior |
| Connection/context/secrets | `repo-types.ts`, `workspace.ts`, `run-resolve.ts`, `secrets.rs`, `context.rs` |
| Preview/lineage | `NodePreview`, schema inference, lineage paths |
| Cancellation/retry | engine cancel flag, retry fields, external call boundaries |
| IPC | Tauri command/Channel and `tauri-bridge.ts` when exposed |
| Tests | planner unit, engine integration, service-gated coverage and gaps |

## Implemented vs recommendation

- Implemented: distributed component contract keyed by component ID.
- Implemented: SQL vs `RuntimeSpec` distinction and executor dispatch.
- Gap: no central manifest-to-planner-to-executor validator.
- Gap: no single plugin/component trait covering all sources, transforms,
  sinks and controls.
- Recommendation: any future central registry must preserve existing IDs,
  serialized properties and planner behavior through explicit compatibility
  tests; it is not an assumption of the current architecture.
