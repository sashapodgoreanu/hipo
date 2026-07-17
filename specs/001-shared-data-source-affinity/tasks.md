# Tasks: Data Source condivisi e Query Source con affinità DuckDB

**Input**: `spec.md`, `plan.md`, `research.md`, `data-model.md`, `contracts/`, `quickstart.md`.

## Dependencies and execution order

Setup → Foundational → US1 (workspace Data Source) → US2 (Query Source) → US3 (affinity execution) → US4 (errors/security). US2 depends on persisted types from US1; US3 depends on US2 and planner contracts; US4 depends on runtime events from US3. Tasks marked `[P]` may run in parallel when they touch different files.

## Phase 1 — Setup and regression discovery

- [x] T001 Add a compatibility inventory for existing `RepoItem`, `PipelineDoc`, `Stage`, `RuntimeSpec`, Tauri commands and event DTOs in `specs/001-shared-data-source-affinity/plan.md` — every changed serialized boundary is named.
- [x] T002 [P] Add regression fixtures for existing Source execution and materialization in `crates/duckdb-engine/tests/execution.rs` — baseline `two_duckdb_sources_same_database` remains green.
- [x] T003 [P] Record the CLI worker framing, cancellation and fallback ADR in `docs/architecture/adr-affinity-session.md` — decision and unresolved prototype risk are explicit.

## Phase 2 — Foundational types, persistence, and migration

- [x] T004 Extend `RepoItemType` and `RepoPayload` with `data_source` and `DataSourcePayload` in `frontend/src/repo-types.ts` — existing item discriminants remain backward compatible.
- [x] T005 Add the `data-sources/` payload directory mapping and generic load/save/delete handling in `frontend/src/workspace.ts` — existing workspaces load without migration errors and no secrets are copied.
- [x] T006 Extend shared pipeline node/property types for `src.query` in `frontend/src/pipeline-types.ts` and the existing generic `NodeData.properties` contract in `crates/metadata/src/lib.rs` — old node JSON deserializes unchanged and new fields are optional where appropriate.
- [x] T007 Define serializable Data Source, Query Source preview, affinity diagnostic and versioned event DTOs in `apps/desktop/src/lib.rs`, `frontend/src/tauri-bridge.ts` and `crates/duckle-runner/src/serve.rs` — desktop and web bridge shapes include schema version, run/context ids, sequence, status and sanitized error envelope.
- [x] T008 Add planner-domain error/status types and stable identifiers in `crates/duckdb-engine/src/plan/mod.rs` — missing references, invalid SQL and attach failures are distinguishable before execution.

## Phase 3 — User Story 1: Gestire Data Source condivisi (P1)

**Goal**: create, edit, duplicate, validate and persist reusable Data Source items without duplicating credentials.

**Independent test**: a workspace can create two compatible Data Sources, reject case-insensitive alias collisions, and report dependencies on rename/delete.

- [x] T009 [US1] Add Data Source editor state and CRUD actions in `frontend/src/App.tsx` — create/edit/duplicate/delete use the existing repository update path.
- [x] T010 [P] [US1] Add the Data Source system folder/tree item and dependency presentation in `frontend/src/ProjectTree.tsx` — Data Sources are distinct from Connections and pipelines.
- [x] T011 [P] [US1] Add Data Source field validation and Connection-kind compatibility helpers in `frontend/src/workflow-ui/editors/DataSourceEditorModal.tsx` — only `duckdb` and `postgres` are accepted, other connector kinds are rejected explicitly, alias uniqueness is case-insensitive and secrets never enter the payload.
- [x] T012 [US1] Implement confirmed alias rename propagation for dependent Query Source SQL in `frontend/src/workspace.ts` — only explicit confirmation mutates dependents and all affected ids are listed.
- [x] T013 [US1] Implement confirmed Data Source deletion with dependency invalidation in `frontend/src/workspace.ts` — deleted references produce an explicit invalid state rather than silent repair.
- [x] T014 [US1] Add `data_source_test` command handling in `apps/desktop/src/lib.rs` and web parity in `crates/duckle-runner/src/serve.rs` — compatibility diagnostics identify connector/extension failures without credentials.
- [x] T015 [US1] Add persistence and alias/dependency requirement tests in `frontend/src/data-source-validation.test.ts` if a runner is introduced; otherwise document the absent frontend test runner in `specs/001-shared-data-source-affinity/quickstart.md`.

## Phase 4 — User Story 2: Creare una Query Source (P1)

**Goal**: provide `src.query` with reference-only Data Source selection, one unrestricted SQL statement, schema and preview.

**Independent test**: a Query Source with one or more Data Source refs previews a row-producing relation; missing refs and multi-statement SQL are rejected with sanitized diagnostics.

- [x] T016 [US2] Add the `src.query` component manifest, palette entry and node editor in `frontend/src/workflow-ui/palette-data.ts` and `frontend/src/workflow-ui/fields/component-manifests.ts` — editor persists refs/SQL only.
- [x] T017 [US2] Add multi-Data-Source selection without copying ConnectionPayload in `frontend/src/DataSourceRefField.tsx` — selected values are stable ids and aliases are shown read-only.
- [x] T018 [US2] Implement single-statement SQL validation in `crates/duckdb-engine/src/plan/builders.rs` — statement type is unrestricted; only multi-statement input returns a typed error.
- [x] T019 [US2] Resolve Data Source refs to ephemeral Connection material in `apps/desktop/src/lib.rs`, `apps/desktop/src/secrets.rs` and `crates/duckle-runner/src/serve.rs` — the frontend transmits only ids/non-sensitive metadata; runtime secrets stay in memory and are never persisted or logged.
- [x] T020 [US2] Implement `query_source_preview` in `apps/desktop/src/lib.rs`, `frontend/src/tauri-bridge.ts` and `crates/duckle-runner/src/serve.rs` — response includes schema, at most 1000 rows, duration and context id; preview timeout is 30 seconds and diagnostics are masked.
- [x] T021 [P] [US2] Add planner unit tests for SQL grammar, ref resolution and preview error taxonomy in `crates/duckdb-engine/src/plan/tests.rs` — SELECT, DDL, DML and multi-statement rejection assertions are present; execution is subject to the environment-gated Cargo test run.

## Phase 5 — User Story 3: Eseguire Query Source con affinità (P1)

**Goal**: execute each connected Query Source component in one DuckDB session, attach each Data Source once, and preserve DAG semantics.

**Independent test**: direct and transitive shared refs yield one context/session; independent branches continue and intermediate applicable stages do not split affinity.

- [x] T022 [US3] Define `AffinityGroup`/`AffinityPlan` and bipartite connected-component construction in `crates/duckdb-engine/src/plan/affinity.rs` — only the selected subgraph contributes groups and ordering is stable.
- [x] T023 [US3] Extend `Stage`/`CompiledPipeline` metadata for group membership and materialization boundaries in `crates/duckdb-engine/src/plan/mod.rs` — existing stages compile with no affinity metadata.
- [x] T024 [US3] Implement the persistent DuckDB CLI worker, statement framing and attach-once lifecycle in `crates/duckdb-engine/src/affinity_session.rs` — extension setup, sanitized stderr, cancellation and cleanup are bounded.
- [ ] T025 [US3] Define the stage compatibility matrix in `crates/duckdb-engine/src/plan/affinity.rs` and integrate group scheduling in `crates/duckdb-engine/src/lib.rs` — `session-preserving` stages run with the worker active; `session-suspending` stages materialize required outputs, preserve process/attachment ownership, block concurrent statements and resume in the same process; process termination invalidates the group; `unsupported` stages fail compilation without per-stage fallback.
- [x] T026 [US3] Materialize Query Source results into the run database and expose downstream relations in `crates/duckdb-engine/src/plan/specs.rs` and `crates/duckdb-engine/src/lib.rs` — VIEW/TABLE choice follows documented consumer rules.
- [ ] T027 [US3] Emit versioned affinity lifecycle events and diagnostics from `crates/duckdb-engine/src/lib.rs`, `apps/desktop/src/lib.rs` and `crates/duckle-runner/src/serve.rs` — context, attachments, durations, sequence ordering, duplicate tolerance and sanitized statuses are observable in desktop/web.
- [x] T028 [P] [US3] Add connected-component, attach-once and transitive-affinity unit tests in `crates/duckdb-engine/src/plan/tests.rs` — direct, transitive and independent cases are covered.
- [ ] T029 [P] [US3] Add service-gated DuckDB integration tests in `crates/duckdb-engine/tests/execution.rs` — interleaved stages, partial runs, session suspension/resume in the same process, worker termination without fallback, cancellation and cleanup verify the same-session contract.

## Phase 6 — User Story 4: Errore, cancellazione e sicurezza (P1)

**Goal**: propagate failures by DAG, preserve independent branches, mask secrets and leave no runtime artifacts.

**Independent test**: context-init failure blocks dependent Query Sources; query failure marks only its downstream; independent branches finish; cancellation cleans workers/files.

- [ ] T030 [US4] Replace whole-loop first-error behavior with dependency-aware failure states in `crates/duckdb-engine/src/lib.rs` — FR-021 propagation and independent branch continuation are explicit.
- [ ] T031 [US4] Add secret redaction for attach/create-secret statements, stderr, history and events in `crates/duckdb-engine/src/` and `apps/desktop/src/secrets.rs` — known credential values cannot appear in diagnostics.
- [x] T032 [US4] Review Tauri capabilities, permissions, scopes and CSP for new preview/test channels in `apps/desktop/capabilities/` and `apps/desktop/tauri.conf.json` — only required IPC/network/process access is granted.
- [ ] T033 [P] [US4] Add failure, partial-attach rollback, masking and cleanup tests in `crates/duckdb-engine/tests/` — tests cover attach failure after the first attachment, invalid SQL, cancellation, worker termination, cleanup within 10 seconds and artifact removal.

## Phase 7 — Polish and cross-cutting verification

- [x] T034 Update `docs/architecture/system-overview.md`, `execution-model.md` and `ipc-contracts.md` — actual new boundaries, events, materialization and secret handoff are documented.
- [x] T035 [P] Run `cargo fmt --all --check` and record the result in `specs/001-shared-data-source-affinity/quickstart.md` — formatting gate is reproducible.
- [ ] T036 [P] Run `cargo clippy --workspace --all-targets --exclude duckle-lance` in CI parity mode — optional platform limitations are recorded.
- [ ] T037 [P] Run `cargo test --workspace --exclude duckle-lance` with required DuckDB/service environment — skipped suites and reasons are recorded.
- [x] T038 [P] Run `npm --prefix frontend run lint` and `npm --prefix frontend run build` — frontend contract/type checks pass.
- [x] T039 Review all acceptance criteria and update every checklist under `specs/001-shared-data-source-affinity/checklists/` — every FR and known coverage gap has traceability, and no checklist remains incomplete without an explicit accepted gap.

## Parallel execution examples

- After T008: T009, T010, T011 and T015 can proceed in parallel.
- After T018/T019: T016, T017, T020 and T021 can proceed in parallel where file ownership is separated.
- After T023: T024, T028 and T029 can proceed in parallel; T025/T026 depend on the worker contract.
- After T030: T031, T032 and T033 can proceed in parallel.

## Implementation strategy

MVP scope is US1 plus the single-statement validation portion of US2: persist Data Sources, enforce alias/dependency rules and reject multi-statement Query Source SQL. The next increment adds preview, then US3 session affinity, then US4 failure/security hardening. Existing Source execution remains the compatibility fallback throughout; no automatic migration is introduced.
