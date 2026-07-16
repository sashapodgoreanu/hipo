# Implementation Plan: [FEATURE]

**Branch**: `[###-feature-name]` | **Date**: [DATE] | **Spec**: [link]

## Summary

[Requirement, current behavior, chosen implementation, and compatibility result.]

## Technical Context

| Concern | Duckle baseline | Feature decision |
|---|---|---|
| Languages | Rust 2021; TypeScript/React | [ ] |
| Desktop/web | Tauri 2 desktop; runner web bridge | [ ] |
| Engine | DuckDB CLI plus Rust RuntimeSpec dispatch | [ ] |
| Storage | Git-friendly workspace JSON and run/state files | [ ] |
| Tests | Cargo tests; frontend type-check/build | [ ] |
| Platforms | Windows, macOS, Linux | [ ] |

## Constitution Check

- [ ] Preserves serialized contracts or includes a migration.
- [ ] Keeps frontend, IPC, planner, and executor responsibilities separate.
- [ ] Documents materialization, RuntimeSpec, secrets, and security impact where relevant.
- [ ] Justifies new dependency/plugin/extension/sidecar, if any.
- [ ] Identifies affected regression tests.
- [ ] For every new or modified Duckle connector, source, sink, or component, includes implementation and tests for its MCP API in `crates/duckle-mcp`.
- [ ] For every new or modified node, defines and verifies both mandatory output flows: `main` and `reject`.
- [ ] For UI changes, identifies the existing components/styles/tokens to reuse and documents any intentional visual or interaction deviation.

## Affected Modules and Contracts

| Layer | Paths / types | Change and boundary |
|---|---|---|
| Frontend | `frontend/src/...` | [ ] |
| Tauri | `apps/desktop/src/...` | [ ] |
| Metadata | `crates/metadata/...` | [ ] |
| Planner | `crates/duckdb-engine/src/plan/...` | [ ] |
| Executor | `crates/duckdb-engine/src/lib.rs` | [ ] |
| Runner/MCP/Scheduler | `crates/...` | [ ] |
| Persistence | `frontend/src/workspace.ts`, `crates/duckdb-engine/src/history.rs`, `crates/duckdb-engine/src/watermark.rs`, `crates/duckdb-engine/src/run_log.rs`, `apps/desktop/src/app_settings.rs` | [ ] |

## Design

### Domain and JSON contracts

[Pipeline/Node/Edge/Schema/Connection/Context/Run DTO changes and compatibility.]

### Planning and execution

[Graph validation, Stage, StageKind, RuntimeSpec, alias, materialization, batch/per-stage behavior, error and cancellation handling.]

### Frontend and IPC

[Component manifest/palette, bridge DTOs, command/event impacts, no duplicated planner semantics. For connectors, sources, sinks, and components, include the matching MCP API contract. For every node, align the required `main` and `reject` output flows across palette/manifest, metadata, frontend validation, planner, executor, and MCP. Reuse existing UI components, tokens, typography, spacing, control states, and interaction patterns; document intentional deviations.]

### Secrets, security, and operations

[Masking, filesystem/network/process access, Tauri capability impact, update/sidecar impact.]

### Migration and rollout

[Workspace/pipeline migration, fallback, release compatibility, documentation.]

## Test Plan

- Unit: [planner/domain/secret/context tests].
- Integration: [DuckDB or service-gated tests].
- MCP: [API registration, request/response contract, and `main`/`reject` output coverage for every affected Duckle connector, source, sink, or component].
- Frontend: [type-check/build and test coverage if introduced; visual and interaction regression checks for UI changes].
- Desktop/IPC: [command/channel coverage or explicit gap].
- Commands: local `cargo fmt --all --check`; optional strict `cargo clippy --workspace --all-targets --all-features`; `cargo test --workspace`; `npm --prefix frontend run lint`; `npm --prefix frontend run build`. CI parity uses `cargo test --workspace --exclude duckle-lance` and `cargo clippy --workspace --all-targets --exclude duckle-lance`.

## Dependency and ADR Decision

**New dependency/plugin/extension/sidecar?** [No / Yes.]
If yes: problem, alternatives, licensing, security, binary-size and multiplatform impact.
**ADR needed?** [No / Yes—link or rationale.]
