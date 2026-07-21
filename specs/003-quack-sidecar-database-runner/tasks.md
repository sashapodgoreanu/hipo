# Tasks: Quack Sidecar Database Runner

**Input**: design artifacts in `/specs/003-quack-sidecar-database-runner/`.

**Final owner decision**: one packaged Quack runtime; no runtime cutover gate, backend class, benchmark prerequisite, or CLI fallback. `cargo fmt` is excluded. The final user validation command is the normal `cargo tauri build` from `apps/desktop`.

## Phase 1: Setup and migration baseline

- [X] T001 Align controller-only acquisition, no budget/queue, five-minute peak and 20% headroom in `docs/architecture/adr-quack-sidecar-runner.md`
- [X] T002 Align Feature 003 intent in `docs/feature-intents/003-quack-sidecar-database-runner.md`
- [X] T003 Preserve the Phase 0 PoC under a named spike during implementation
- [X] T004 Raise workspace MSRV and CI checks to Rust 1.88
- [X] T005 Create and workspace-register `crates/duckle-db-runner`
- [X] T006 [P] Create SQL/runtime/source/transform/sink/preview/partial fixtures
- [X] T007 [P] Create Windows/macOS/Linux package smoke workflow

## Phase 2: Foundational contracts

- [X] T008 Define `RunnerResourcesProfile`, migration, validation and diagnostics
- [X] T009 [P] Define opaque worker, lease, session, failure and telemetry types
- [X] T010 Implement complete-profile persistence and migration
- [X] T011 [P] Add resource-profile IPC DTOs and bridge declarations
- [X] T012 Implement loopback provider, inherited bootstrap, random credential and handshake
- [X] T013 Implement process containment, tree termination and artifact sweeper
- [X] T014 Implement sanitized runner events and allowlisted metrics
- [X] T015 [P] Add profile serialization, migration and bounds tests
- [X] T016 [P] Add bootstrap, readiness, mismatch and secret tests

## Phase 3: Elastic worker pool

- [X] T017 Implement worker states, atomic acquire and single-use release
- [X] T018 Implement five-minute demand peak ledger
- [X] T019 Implement five-second target calculation and start de-duplication
- [X] T020 Implement warm provisioning and ready-only scale-in
- [X] T021 Implement controller-owned on-demand assignment
- [X] T022 Emit sanitized allocation and lifecycle events
- [X] T023 [P] Test leases, backoff, cancellation, duplicate start and precedence
- [X] T024 [P] Test base, ceiling, tick, expiry, restart and scale-in
- [X] T025 Test 100 on-demand runs and target 120
- [X] T026 Test second wave using warm workers with 20 ready retained

## Phase 4: Atomic resource profile

- [X] T027 Implement generations, permits, cancellable waits and drain-before-apply
- [X] T028 Implement coalescing and worker-state apply precedence
- [X] T029 Preserve the prior effective profile on apply failure
- [X] T030 Connect base-capacity changes to autoscaling
- [X] T031 Implement atomic runner-resource IPC commands
- [X] T032 Add Runner resources UI and diagnostics
- [X] T033 [P] Test drain, coalescing, apply failure and permit limits
- [X] T034 [P] Test persistence, IPC, legacy profile and convergence
- [X] T035 Test the Runner resources form

## Phase 5: Per-run Quack database

- [X] T036 Implement Quack-backed `RunDatabase` and `RunSession`
- [X] T037 Integrate controller acquisition behind the engine adapter
- [X] T038 Implement the SQL remote / Quack / Parquet decision table
- [X] T039 Route preview and partial run through the runner
- [X] T040 Create one desktop controller per workspace
- [X] T041 [P] Add isolation, batch, runtime, preview and partial integration tests
- [X] T042 [P] Prove one controller acquire per normal run and no direct spawn

## Phase 6: Catalog sharing without affinity

- [X] T043 Preserve temporary state in planner batches
- [X] T044 Deduplicate per-run server setup
- [X] T045 Implement affinity-free planning
- [X] T046 Migrate Query Source callers to normal run sessions
- [X] T047 [P] Add catalog-sharing and 2/4/8 concurrency tests

## Phase 7: Cancellation and cleanup

- [X] T048 Wire cancellation to lease and process-scope termination
- [X] T049 Classify transport loss as `runner_crashed`
- [X] T050 Implement concurrent desktop run ownership
- [X] T051 Run sweeper and parent-death containment from startup
- [X] T052 [P] Add cancellation, crash and orphan integration tests

## Phase 8: Security

- [X] T053 Enforce opaque client handles and capability isolation
- [X] T054 Redact failures before UI, history, logs, headless and MCP output
- [X] T055 Verify Tauri capability and CSP scope are unchanged
- [X] T056 [P] Add secret-canary tests across persistent and transient surfaces
- [X] T057 [P] Add credential and external-runtime isolation tests

## Phase 9: Offline package and direct activation

- [X] T058 Pin DuckDB/Quack version, checksum, license and provenance
- [X] T059 Package and locate the sidecar/extension pair
- [X] T060 Route headless and web runs through the controller
- [X] T061 Route scheduler and MCP through the same controller
- [X] T062 Implement the original migration gate as an intermediate safety step
- [X] T063 Add original gate-selection and no-silent-fallback regressions
- [X] T064 Apply memory, CPU, spill and temporary-space limits before readiness
- [X] T065 Add resource and runner-unavailable integration coverage
- [X] T066 Add decision-table correctness coverage
- [X] T067 Record the owner decision that the CLI benchmark is optional analysis, not a runtime activation gate, in `quickstart.md`
- [X] T068 Route inspect, drift and branch/diff through controller abstractions
- [X] T069 Verify requested/effective profile consistency for every entry point
- [X] T070 Verify allowlisted telemetry and retention routing
- [X] T071 Activate one Quack route, remove runtime class/evidence configuration, disable CLI fallback and make normal desktop packaging require the verified pair
- [X] T072 Remove `spikes/quack-sidecar-phase0-spike/` after retaining the architectural conclusions
- [X] T073 Preserve readable-but-disabled SlothDB and xf.dbt diagnostics
- [X] T074 Add clean offline package and mismatch smoke coverage

## Phase 10: Polish and final validation

- [X] T075 Document pool states, profile application, autoscaling and diagnostics
- [X] T076 Assert complete sanitized autoscale telemetry
- [X] T077 Record the direct owner-approved activation decision and remove the obsolete CutoverEvidence template
- [ ] T078 Run clippy and workspace tests after the final code changes; `cargo fmt` is explicitly excluded by the owner
- [X] T079 Run frontend install, lint and build
- [ ] T080 Complete the final production-reference scan and validate the packaged desktop with the normal `cargo tauri build`

## Execution order

```text
Setup -> Contracts -> Worker pool -> Resource profile -> RunDatabase
      -> Catalog/concurrency -> Lifecycle -> Security -> Offline package
      -> Direct Quack activation -> Final validation
```

## Completion condition

The feature is complete after T078 and T080 pass. The only user-facing build command is:

```bat
@echo off
cd /d "%~dp0apps\desktop"
cargo tauri build
```
