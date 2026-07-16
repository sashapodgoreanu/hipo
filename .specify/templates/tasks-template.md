# Tasks: [FEATURE NAME]

**Input**: `spec.md`, `plan.md`, research, contracts, and data-model artifacts.

## Format

`- [ ] T### [P?] [US?] <action> in <exact path> — <verification>`

Use exact repository paths. Mark `[P]` only for tasks that do not contend for files or ordered contracts. Each change affecting an existing behavior starts with a regression/compatibility task.

## Required ordering when applicable

### Phase 1 — Regression and contract discovery

- [ ] T001 [US?] Add/update regression coverage for the current observable behavior in [path].
- [ ] T002 [US?] Record Pipeline/Node/Edge/Schema/Connection/IPC compatibility constraints in the feature artifacts.

### Phase 2 — Types, persistence, and migration

- [ ] T003 [US?] Update shared Rust/TypeScript DTOs in [path].
- [ ] T004 [US?] Implement workspace/pipeline/history migration or backward compatibility in [path], when required.

### Phase 3 — Rust domain, planner, and execution

- [ ] T005 [US?] Update domain metadata in `crates/metadata/...`, when required.
- [ ] T006 [US?] Update component planner/Stage/RuntimeSpec behavior in `crates/duckdb-engine/src/plan/...`, when required.
- [ ] T007 [US?] Update executor, cancellation, retry, materialization, preview, or logging in `crates/duckdb-engine/src/lib.rs`, when required.

### Phase 4 — Runtime surfaces

- [ ] T008 [US?] For every new or modified Duckle connector, source, sink, or component, implement and test its MCP API in `crates/duckle-mcp/...`; otherwise update affected scheduler, runner, MCP, or sidecar integration in the exact crate.
- [ ] T009 [US?] Update Tauri command DTO/channel behavior in `apps/desktop/src/...`, when required.
- [ ] T010 [US?] Update palette, manifest, bridge, and UI in `frontend/src/...`, when required — for every affected node, define and test both mandatory output flows, `main` and `reject`, across metadata, palette/manifest, frontend validation, planner, executor, and MCP; reuse existing components, styles/tokens, spacing, states, and interaction patterns; document any intentional deviation.

### Phase 5 — Security and verification

- [ ] T011 [US?] Review secrets, filesystem/network/process access, CSP/capabilities, and exported SQL behavior.
- [ ] T012 [P] [US?] Add/update unit tests in the owning crate/module.
- [ ] T013 [P] [US?] Add/update service-gated integration tests where needed.
- [ ] T014 Run `cargo fmt --all --check`.
- [ ] T015 Run `cargo clippy --workspace --all-targets --exclude duckle-lance` for CI parity; run `--all-features` separately when optional platform dependencies are available.
- [ ] T016 Run `cargo test --workspace` (routine CI uses `--exclude duckle-lance`; record justified exclusions/environment requirements).
- [ ] T017 Run `npm --prefix frontend run lint` and `npm --prefix frontend run build` for frontend changes.
- [ ] T018 Update architecture/docs/ADR and perform final compatibility review.

## Completion criteria

- Every task identifies its file/module and verification.
- No task silently changes an existing serialized or IPC contract.
- Connector/source/sink/component tasks include MCP API implementation and verification.
- Node tasks include `main` and `reject` output-flow implementation and verification.
- UI tasks name the existing visual/interaction patterns they preserve and include an appropriate regression or review check.
- Security-sensitive tasks identify secret, path, process, or network effects.
- The final task reports checks run, skipped checks, and known coverage gaps.
