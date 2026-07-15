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

[Component manifest/palette, bridge DTOs, command/event impacts, no duplicated planner semantics.]

### Secrets, security, and operations

[Masking, filesystem/network/process access, Tauri capability impact, update/sidecar impact.]

### Migration and rollout

[Workspace/pipeline migration, fallback, release compatibility, documentation.]

## Test Plan

- Unit: [planner/domain/secret/context tests].
- Integration: [DuckDB or service-gated tests].
- Frontend: [type-check/build and test coverage if introduced].
- Desktop/IPC: [command/channel coverage or explicit gap].
- Commands: local `cargo fmt --all --check`; optional strict `cargo clippy --workspace --all-targets --all-features`; `cargo test --workspace`; `npm --prefix frontend run lint`; `npm --prefix frontend run build`. CI parity uses `cargo test --workspace --exclude duckle-lance` and `cargo clippy --workspace --all-targets --exclude duckle-lance`.

## Dependency and ADR Decision

**New dependency/plugin/extension/sidecar?** [No / Yes.]
If yes: problem, alternatives, licensing, security, binary-size and multiplatform impact.
**ADR needed?** [No / Yes—link or rationale.]
