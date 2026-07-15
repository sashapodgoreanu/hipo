# Duckle contributor guide for agents

## Project map

Duckle is a local-first ETL/ELT studio. The frontend is React/TypeScript/Vite in `frontend/`; the desktop shell is Tauri in `apps/desktop/`; execution is primarily in `crates/duckdb-engine/` through the DuckDB CLI. The repository is a Cargo workspace.

| Area | Primary paths | Responsibility |
|---|---|---|
| Serializable domain | `crates/metadata/src/lib.rs` | Pipeline, nodes, edges, schema, columns, data types |
| Planner | `crates/duckdb-engine/src/plan/mod.rs`, `crates/duckdb-engine/src/plan/graph.rs`, `crates/duckdb-engine/src/plan/builders.rs`, `crates/duckdb-engine/src/plan/specs.rs` | PipelineDoc → Stage / RuntimeSpec / leaves |
| Executor | `crates/duckdb-engine/src/lib.rs` | DuckDB CLI, runtime dispatch, materialization, cancellation, preview |
| Context/history/state | `crates/duckdb-engine/src/context.rs`, `crates/duckdb-engine/src/history.rs`, `crates/duckdb-engine/src/watermark.rs`, `crates/duckdb-engine/src/run_log.rs` | Resolution and workspace persistence |
| Frontend | `frontend/src/App.tsx`, `frontend/src/pipeline-types.ts`, `frontend/src/tauri-bridge.ts` | Canvas, persistence, IPC adapter |
| Components | `frontend/src/workflow-ui/palette-data.ts`, `frontend/src/workflow-ui/fields/component-manifests.ts`, `frontend/src/workflow-ui/fields/manifest-synth.ts` | Palette, forms, ports |
| Desktop | `apps/desktop/src/lib.rs` | Tauri builder and command registry |
| Desktop services | `apps/desktop/src/engine_manager.rs`, `apps/desktop/src/secrets.rs`, `apps/desktop/src/app_settings.rs`, `apps/desktop/src/workspace_git.rs`, `apps/desktop/src/llama_chat.rs` | Engines, secrets, settings, Git, AI |
| Scheduler | `crates/scheduler/src/lib.rs` | Cron, interval, file-watch |
| Runner | `crates/duckle-runner/src/main.rs`, `crates/duckle-runner/src/serve.rs` | Headless run, artifact, web server |
| MCP | `crates/duckle-mcp/src/main.rs`, `crates/duckle-mcp/src/tools.rs` | MCP stdio server/tools |

## Read before changing critical behavior

- Pipeline/graph: `crates/metadata/src/lib.rs`, `crates/duckdb-engine/src/plan/mod.rs`, `crates/duckdb-engine/src/plan/graph.rs`, `frontend/src/validation.ts`.
- Components: `frontend/src/workflow-ui/palette-data.ts`, `frontend/src/workflow-ui/fields/manifest-synth.ts`, planner branch and executor runtime dispatch for the component ID.
- Execution/materialization: planner stage construction and `DuckdbEngine::execute_pipeline_with_events`.
- Connections/secrets: `frontend/src/workspace.ts`, `frontend/src/run-resolve.ts`, `apps/desktop/src/secrets.rs`, `crates/duckdb-engine/src/context.rs`.
- IPC: `apps/desktop/src/lib.rs` handler and `frontend/src/tauri-bridge.ts`.

## Rules

- Preserve JSON/IPC compatibility unless a spec and migration explicitly say otherwise.
- Do not treat frontend validation or manifest metadata as the sole planner authority.
- Keep node ID, SQL alias, and component ID distinct.
- Keep new Tauri command adapters thin; existing commands in `apps/desktop/src/lib.rs` may retain legacy orchestration until separately decomposed. Test domain/planner/engine behavior outside Tauri.
- Never put real plaintext secrets in logs, errors, previews, exported SQL, or fixtures. Synthetic test values are allowed only when clearly non-production and covered by redaction tests.
- Treat broad filesystem capability, process spawning, runner web mode, sidecars, and unsigned extensions as security-sensitive.
- Do not change ignored/generated output unless the task requires it; regenerate catalog assets only through the documented script.

## Commands (PowerShell)

```powershell
npm --prefix frontend ci
npm --prefix frontend run lint
npm --prefix frontend run build
cargo fmt --all --check
cargo clippy --workspace --all-targets --exclude duckle-lance   # CI parity
cargo test --workspace --exclude duckle-lance                    # CI parity
cargo run -p duckle-desktop
```

For a strict local feature-inclusive lint pass, additionally run `cargo clippy --workspace --all-targets --all-features` when optional platform dependencies are available. Engine integration tests need `DUCKLE_DUCKDB_BIN`; the full local workspace test remains `cargo test --workspace`.

## Change checklists

### Add a Source

1. Register `src.*` in palette and define/synthesize its manifest/ports.
2. Add planner behavior and choose SQL/ATTACH/table function versus `RuntimeSpec`.
3. Add executor dispatch if runtime-driven; define schema inference, preview, cancellation and materialization.
4. Cover secrets/connection fields and add planner plus execution tests.

### Add a Transform

1. Define input/output ports and schema behavior.
2. Add `xf.*` planner logic; choose pure SQL or `RuntimeSpec`.
3. Handle reject/output relation and multi-input rules where applicable.
4. Test graph validation, SQL/runtime result, retry/cancellation if relevant.

### Add a Sink

1. Define upstream requirement, target config and write mode.
2. Add `snk.*` planner and executor behavior.
3. Specify idempotency, existing-target behavior, partial failure and retry safety.
4. Test row count, errors, secrets and cleanup.

### Add a Control node

1. Document whether its behavior is graph-data, pass-through, or side effect.
2. Add `ctl.*` planning/runtime behavior and cancellation semantics.
3. Do not assume UI trigger edges are planner data edges; verify the actual graph path.
4. Test child pipeline, branch, error and concurrency behavior.

### Change PipelineNode or PipelineEdge

1. Update Rust metadata and TypeScript counterpart together.
2. Check serialization, aliases/handles, workspace compatibility and migration.
3. Update planner, validation, runner/MCP, IPC and tests as required.

### Add RuntimeSpec

1. Define the spec in `plan/specs.rs` and variant in `plan/mod.rs`.
2. Build it from component properties and dispatch it in the engine.
3. State whether it replaces, precedes, or follows stage SQL and whether it produces a relation.
4. Add cancellation, retry, secret redaction and integration coverage.

### Add a Tauri command

1. Define serializable DTO/error behavior and add it to `generate_handler!`.
2. Add matching bridge wrapper/types and web-shim/runner handling when needed.
3. Document filesystem, network, process, secret, event and cancellation effects.
4. Review capability/permission scope.

### Change Connection or Secrets

1. Preserve encrypted payload and placeholder behavior.
2. Check frontend hydration/copy semantics and headless context resolution.
3. Test redaction in exported SQL, runtime errors, history and logs.

### Change materialization, planner, or executor

1. Read compile, partial compile and execution batch/per-stage paths.
2. State impact on consumer count, views/tables/Parquet, aliases and partial runs.
3. Add regression tests before changing observed behavior.
4. Verify cleanup, cancellation, preview, run history and multiplatform effects.
