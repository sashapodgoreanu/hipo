# Duckle Constitution

## Project identity

Duckle is a local-first ETL/ELT data-pipeline studio. The implemented system is a Rust Cargo workspace with a React/TypeScript/Vite frontend, a Tauri 2 desktop shell, a DuckDB-CLI-backed execution engine, a headless runner, a scheduler, and an MCP server. This constitution governs future changes; the architecture documents record current implementation details and gaps.

## Core principles

### I. Preserve observable brownfield behavior

Do not alter the observable behavior of pipelines, component identifiers, workspace files, connection handling, runner commands, or internal IPC without an explicit specification, compatibility analysis, regression coverage, and—when formats change—a migration or documented compatibility decision. When prose and implementation conflict, record the code/test-confirmed behavior as the current state rather than silently treating the prose as implemented.

### II. Treat serialized domain data as versioned contracts

The `duckle-metadata` package (imported in Rust as `duckle_metadata`) is authoritative for serializable pipeline/schema types; `duckle_duckdb_engine::plan::PipelineDoc` is the executable pipeline payload. Their JSON-compatible frontend counterparts must remain semantically aligned. Changes to pipelines, nodes, edges, schemas, connections, contexts, run history, component IDs, or workspace layout require compatibility review and migration planning where applicable.

### III. Keep presentation, IPC, planning, and execution separate

The frontend owns visualization, graph editing, input collection, and result presentation. It is not authoritative for graph semantics, topological ordering, SQL generation, execution, or secret resolution. New Tauri commands should be thin adapters to Rust services; existing commands may retain legacy orchestration until separately decomposed. Domain/planner/engine behavior must be testable without a WebView or Tauri runtime.

### IV. Pipeline graph semantics are explicit

A persisted pipeline represents the logical graph, not its compiled plan. Node ID, component ID, and optional SQL alias are distinct contracts. Execution order derives from validated graph dependencies, never canvas position. Any change touching handles, data/control edges, cycles, fan-in/out, reject outputs, leaves, aliases, or partial run must state its graph and compatibility impact.

### V. Components are capabilities; nodes are configured instances

Component IDs connect the palette, property manifests, planner and executor. A new or modified component must make its category, required properties, ports, schema inference/preview, connection and secret use, cancellation/retry behavior, side effects, SQL/runtime mode, and tests determinable. Do not claim an unimplemented component behavior merely because a palette item exists.

### VI. The planner is deterministic and side-effect free

The planner receives a resolved `PipelineDoc` and produces ordered `Stage`s, leaves, materialization decisions, and optional `RuntimeSpec`. It validates graph and alias constraints, resolves upstream dependencies, distinguishes SQL from runtime work, and does not perform external side effects. Equivalent inputs must preserve semantic compilation behavior.

### VII. Preserve the DuckDB execution model

The current engine uses the DuckDB CLI, a per-run temporary database, batched execution only for eligible pure-SQL stages, and per-stage execution for runtime/control/other ineligible stages. Materialization, temporary files, cleanup, preview, cancellation, retries, sink behavior, and run history are externally observable execution semantics and require regression tests when changed.

### VIII. Connections, contexts, and secrets are separate concerns

Connections are persisted separately from pipelines; contexts resolve execution-time variables; secrets require redaction from logs, errors, previews, and SQL exports by default. Preserve and document precedence across explicit values, contexts, environment variables, and defaults. Current connection selection copies values into node properties; any move to dynamic ID-based runtime resolution is a compatibility-sensitive change, not an assumed current behavior.

### IX. Tauri and web boundaries are internal APIs

Every Tauri command and web bridge operation must have serializable input/output, errors, side effects, filesystem/network/process/secret access, events, and cancellation behavior understood before change. New capabilities, permissions, plugins, sidecars, or process-spawn paths require explicit security review and least-privilege justification.

### X. Dependencies and multiplatform binaries are deliberate

New Rust crates, frontend packages, Tauri plugins, DuckDB extensions, driver libraries, or sidecars require a plan documenting problem, alternatives, binary-size/licensing/security impact, maintenance owner, and Windows/macOS/Linux compatibility. Significant architectural choices belong in the feature plan or an ADR.

### XI. Quality gates match the repository

Run the relevant repository commands for touched areas: `cargo fmt --all --check`, `cargo test --workspace`, `npm --prefix frontend run lint`, and `npm --prefix frontend run build`. For a strict local Rust lint pass, use `cargo clippy --workspace --all-targets --all-features` when optional platform dependencies are available. Routine CI currently uses `cargo test --workspace --exclude duckle-lance` and `cargo clippy --workspace --all-targets --exclude duckle-lance`; formatting and clippy are informational there. Account for documented environment-dependent integration tests. Avoid `unwrap`/`expect` in application paths unless an invariant is explicit and justified.

### XII. Test behavior at the right layer

Add or update focused Rust tests for parsing, planning, materialization, runtime dispatch, secret masking, cancellation, retries, and persistence where behavior changes. Use integration tests for DuckDB and external services only when necessary; identify frontend, Tauri IPC, and end-to-end coverage explicitly when they are absent.

## Current gaps to keep visible

- Component contracts are distributed across palette, frontend manifests, MCP catalog, planner, and executor.
- Frontend/Rust IPC DTOs are manually mirrored.
- The frontend exposes trigger edge types that the Rust data-edge planner does not include in topological ordering.
- The desktop filesystem capability is broad and the desktop CSP is unset; security changes must not widen this surface casually.

## Governance

Feature specifications must identify the affected contract boundaries and state whether behavior is implemented, proposed, or a known gap. Plans must record deviations from these principles and their rationale. This constitution complements `AGENTS.md` and the verified notes in `docs/architecture/`; it does not override verified code behavior without an approved change.

**Version**: 1.0.0 | **Ratified**: 2026-07-15 | **Last Amended**: 2026-07-15
