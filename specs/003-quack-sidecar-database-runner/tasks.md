# Tasks: Quack Sidecar Database Runner

**Input**: Design artifacts and requirements-quality review in `/specs/003-quack-sidecar-database-runner/`.

**Tests**: Required for baseline parity, security, lifecycle, autoscaling, IPC/UI, benchmark, and offline-package gates.

**Quality review**: Resolve or explicitly accept the relevant findings in `checklists/runner-quality.md` before completing a cutover gate.

**Organization**: US4 comes first because every run must acquire a worker from its controller.

## Phase 1: Setup and migration baseline

- [ ] T001 Align controller-only acquisition, no budget/queue, five-minute peak, 20% headroom, and cutover rules in `docs/architecture/adr-quack-sidecar-runner.md`
- [ ] T002 Align Feature 003 intent with final pool, profile, and cutover rules in `docs/feature-intents/003-quack-sidecar-database-runner.md`
- [ ] T003 Rename retained PoC to `spikes/quack-sidecar-phase0-spike/` and update its repository references
- [ ] T004 Raise workspace MSRV and CI checks to Rust 1.85.1 in `Cargo.toml`, `rust-toolchain.toml`, and `.github/workflows/`
- [ ] T005 Create and workspace-register official runner crate in `crates/duckle-db-runner/Cargo.toml`, `crates/duckle-db-runner/src/lib.rs`, and `Cargo.toml`
- [ ] T006 [P] Create and baseline SQL/runtime/source/transform/sink/preview/partial parity fixtures in `crates/duckdb-engine/tests/quack_runner_parity.rs`
- [ ] T007 [P] Create bundled-runner Windows/macOS/Linux smoke workflow in `.github/workflows/runner-package-smoke.yml`

---

## Phase 2: Foundational contracts and shared infrastructure

- [ ] T008 Define versioned `RunnerResourcesProfile`, defaults, migration, validation, and diagnostics in `crates/duckle-db-runner/src/resources.rs`
- [ ] T009 [P] Define opaque worker/lease/session/provider/failure/event, sanitized-metric, and CutoverEvidence types in `crates/duckle-db-runner/src/model.rs`, `crates/duckle-db-runner/src/cutover.rs`, and `crates/duckle-db-runner/src/lib.rs`
- [ ] T010 Implement compatible complete-profile persistence and migration in `apps/desktop/src/app_settings.rs`
- [ ] T011 [P] Add resource-profile IPC DTOs and typed bridge declarations in `apps/desktop/src/lib.rs` and `frontend/src/tauri-bridge.ts`
- [ ] T012 Implement loopback-only LocalProcessProvider, inherited bootstrap pipe/handle, random credential, effective-profile-before-readiness, and authenticated handshake in `crates/duckle-db-runner/src/local_process_provider.rs`
- [ ] T013 Implement process containment, tree termination, and run-scoped artifact sweeper in `crates/duckle-db-runner/src/process_cleanup.rs`
- [ ] T014 Implement sanitized runner events, allowed metric fields/reason codes, and cutover-gate rejection events excluding endpoint, port, PID, path, SQL, secret, and capability in `crates/duckle-db-runner/src/events.rs`
- [ ] T015 [P] Add profile defaults/serialization/migration/bounds tests in `crates/duckle-db-runner/src/resources.rs` and `apps/desktop/src/app_settings.rs`
- [ ] T016 [P] Add bootstrap, effective-profile readiness, credential, mismatch, and argv/environment/file secret tests in `crates/duckle-db-runner/tests/local_process_provider.rs`

---

## Phase 3: User Story 4 - Elastic ready/on-demand worker pool (Priority: P1)

**Independent Test**: Exercise concurrent demand, startup failure, release, growth, and scale-in; verify base 3, exclusive lease, on-demand decisions, and 5-second/5-minute policy.

- [ ] T017 [US4] Implement WorkerPoolControl states, atomic acquire, lease ownership, and single-use release in `crates/duckle-db-runner/src/worker_pool.rs`
- [ ] T018 [US4] Implement event-time demand tracking and sliding five-minute peak ledger in `crates/duckle-db-runner/src/demand.rs`
- [ ] T019 [US4] Implement 5-second target `max(base_capacity, ceil(peak_5m * 1.20))`, start de-duplication, floor, and restart state in `crates/duckle-db-runner/src/autoscaler.rs`
- [ ] T020 [US4] Implement warm provisioning/readiness and ready-only scale-in without interrupting or replacing leased workers in `crates/duckle-db-runner/src/worker_pool.rs`
- [ ] T021 [US4] Implement immediate controller-owned on-demand assignment with no ready worker, excluded from warm capacity and terminated with run in `crates/duckle-db-runner/src/worker_pool.rs`
- [ ] T022 [US4] Emit correlated sanitized acquire, decision, provision, readiness, lease, release, failure, and scale events in `crates/duckle-db-runner/src/events.rs`
- [ ] T023 [P] [US4] Test exclusive leases, readiness backoff, cancellation during provision, no duplicate start, and shutdown/crash/release/apply/scale precedence in `crates/duckle-db-runner/tests/worker_pool.rs`
- [ ] T024 [P] [US4] Test base 3, ceiling arithmetic, five-second tick, five-minute expiry, restart, and ready-only scale-in in `crates/duckle-db-runner/tests/autoscaler.rs`
- [ ] T025 [US4] Add 100-run test: 100 on-demand assignments, demand counted once, then target 120 warm workers in `crates/duckle-db-runner/tests/worker_pool.rs`
- [ ] T026 [US4] Add second-wave test: 100 leases from 120 warm workers, 20 ready retained, no on-demand workers in `crates/duckle-db-runner/tests/worker_pool.rs`

---

## Phase 4: User Story 7 - Atomic per-run resource profile (Priority: P1)

**Independent Test**: Save versions during 1/2/4/8 active queries; verify drain, coalescing, latest-only atomic apply, apply failure, and base-capacity convergence.

- [ ] T027 [US7] Implement profile generations, permits 1..=8, cancellable waits, and drain-before-atomic apply in `crates/duckle-db-runner/src/run_session.rs`
- [ ] T028 [US7] Implement save coalescing, serialized worker-state precedence, ready apply, stale-starting publication prevention, and target-aware termination in `crates/duckle-db-runner/src/worker_pool.rs`
- [ ] T029 [US7] Preserve prior effective profile on apply failure and return sanitized configuration_apply_failed in `crates/duckle-db-runner/src/run_session.rs`
- [ ] T030 [US7] Connect profile base-capacity change to immediate autoscaling without terminating leased workers in `crates/duckle-db-runner/src/autoscaler.rs` and `crates/duckle-db-runner/src/worker_pool.rs`
- [ ] T031 [US7] Implement atomic settings_get_runner_resources and settings_set_runner_resources in `apps/desktop/src/lib.rs`
- [ ] T032 [US7] Add Runner resources UI with complete save, requested/effective diagnostics, and base default 3 in `frontend/src/workflow-ui/SettingsModal.tsx`
- [ ] T033 [P] [US7] Test profile drain, coalescing, apply failure, and permit limits in `crates/duckle-db-runner/tests/run_session.rs`
- [ ] T034 [P] [US7] Test complete profile persistence/IPC, legacy/invalid profile behavior, and base-capacity convergence in `apps/desktop/src/app_settings.rs` and `apps/desktop/src/lib.rs`
- [ ] T035 [US7] Test Runner resources form, 1..=8 range, and diagnostics in `frontend/src/workflow-ui/SettingsModal.test.tsx`

---

## Phase 5: User Story 1 - Isolated official database for every pipeline run (Priority: P1)

**Independent Test**: Compare parity results, events, preview, partial run, runtime, and external effects with the baseline.

- [ ] T036 [US1] Implement Quack-backed RunDatabase/session SQL-batch, setup, transfer, preview, and cancel adapter in `crates/duckle-db-runner/src/run_database.rs` and `crates/duckle-db-runner/src/run_session.rs`
- [ ] T037 [US1] Integrate controller acquisition and RunDatabase behind non-default compatibility selection, without changing production routing, in `crates/duckdb-engine/src/lib.rs`
- [ ] T038 [US1] Implement versioned SQL-remote/Quack/Parquet decision table with inputs, outputs, retries, cleanup, and sanitized exceptions in `crates/duckdb-engine/src/lib.rs` and `crates/duckdb-engine/src/context.rs`
- [ ] T039 [US1] Route preview and partial run through official runner with unchanged semantics in `crates/duckdb-engine/src/lib.rs`
- [ ] T040 [US1] Create one desktop controller per open workspace with per-run cancellation ownership in `apps/desktop/src/engine_manager.rs` and `apps/desktop/src/lib.rs`
- [ ] T041 [P] [US1] Add isolation, SQL batch, runtime/materialization, preview, and partial integration tests in `crates/duckdb-engine/tests/quack_runner_parity.rs`
- [ ] T042 [P] [US1] Add regression that each normal run emits one controller acquire, cannot directly spawn, and preserves compatibility routing before cutover in `crates/duckdb-engine/tests/quack_runner_parity.rs`

---

## Phase 6: User Story 2 - Catalog sharing and concurrency without affinity (Priority: P1)

**Independent Test**: Verify shared relations, server setup, batches, and 2/4/8 compatible requests without affinity classification or fallback.

- [ ] T043 [US2] Keep temporary state in planner batches and route server setup through RunSession in `crates/duckdb-engine/src/lib.rs` and `crates/duckle-db-runner/src/run_session.rs`
- [ ] T044 [US2] Add per-run server-setup deduplication and catalog visibility across stateless requests in `crates/duckle-db-runner/src/run_session.rs`
- [ ] T045 [US2] Implement and test affinity-free planning while retaining affinity compatibility until cutover in `crates/duckdb-engine/src/plan/mod.rs`, `crates/duckdb-engine/src/plan/graph.rs`, and `crates/duckdb-engine/src/plan/builders.rs`
- [ ] T046 [US2] Migrate affinity callers to normal run database behind compatibility selection while retaining `AffinitySession` until cutover in `crates/duckdb-engine/src/lib.rs` and `crates/duckdb-engine/src/affinity_session.rs`
- [ ] T047 [P] [US2] Replace affinity tests with catalog-sharing and 2/4/8 concurrency tests in `crates/duckdb-engine/src/plan/tests.rs` and `crates/duckdb-engine/tests/execution.rs`

---

## Phase 7: User Story 3 - Deterministic cancellation, crash, and cleanup (Priority: P1)

**Independent Test**: Cancel and force-kill scan, join, spill, transfer, and runtime work; verify sanitized outcome and cleanup within 10 seconds.

- [ ] T048 [US3] Wire cancellation to lease cancellation, process-scope termination, and no-new-stage dispatch in `crates/duckdb-engine/src/lib.rs` and `crates/duckle-db-runner/src/run_session.rs`
- [ ] T049 [US3] Classify transport loss as runner_crashed, poison lease, and prevent reuse in `crates/duckle-db-runner/src/worker_pool.rs` and `crates/duckdb-engine/src/lib.rs`
- [ ] T050 [US3] Replace desktop single-current-run state with concurrent run-to-session ownership in `apps/desktop/src/lib.rs`
- [ ] T051 [US3] Run sweeper and parent-death containment from desktop/headless startup in `apps/desktop/src/lib.rs` and `crates/duckle-runner/src/main.rs`
- [ ] T052 [P] [US3] Add cancellation/crash/orphan integration tests for scan, join, spill, transfer, and runtime in `crates/duckdb-engine/tests/quack_runner_lifecycle.rs`

---

## Phase 8: User Story 5 - Secret and capability protection (Priority: P1)

**Independent Test**: Use synthetic canaries and inspect spawn, files, IPC, logs, history, errors, export, profiler, cancellation, and access attempts.

- [ ] T053 [US5] Enforce opaque client handles and prohibit endpoint/capability exposure to runtime or user code in `crates/duckle-db-runner/src/local_process_provider.rs` and `crates/duckle-db-runner/src/run_session.rs`
- [ ] T054 [US5] Redact failures/events before history, logs, desktop events, headless output, and MCP responses in `crates/duckdb-engine/src/history.rs`, `crates/duckdb-engine/src/run_log.rs`, `apps/desktop/src/lib.rs`, `crates/duckle-runner/src/main.rs`, and `crates/duckle-mcp/src/tools.rs`
- [ ] T055 [US5] Verify no Tauri capability or CSP scope expands in `apps/desktop/capabilities/` and `apps/desktop/tauri.conf.json`
- [ ] T056 [P] [US5] Add secret-canary redaction tests for argv, env, files, IPC, events, history, errors, and logs in `crates/duckle-db-runner/tests/security_redaction.rs` and `crates/duckdb-engine/tests/quack_runner_lifecycle.rs`
- [ ] T057 [P] [US5] Add credential and external-runtime isolation tests in `crates/duckle-db-runner/tests/local_process_provider.rs`

---

## Phase 9: User Story 6 - Offline distribution and complete cutover (Priority: P1)

**Independent Test**: On clean offline builds with no system DuckDB CLI, run desktop, headless, scheduler, MCP, inspect, drift, branch/diff, artifacts, and data tools; establish the benchmark gate, then scan for retired references.

- [ ] T058 [US6] Pin DuckDB/Quack with version, checksum, license, provenance, and offline staging verification in `apps/desktop/src/engine_manager.rs` and `apps/desktop/build.rs`
- [ ] T059 [US6] Package and locate sidecar/extension pair for desktop, headless, and releases in `crates/duckle-runner/Cargo.toml`, `apps/desktop/build.rs`, and `.github/workflows/`
- [ ] T060 [US6] Route headless CLI/web through controller and remove web run_lock serialization without an admission queue in `crates/duckle-runner/src/main.rs` and `crates/duckle-runner/src/serve.rs`
- [ ] T061 [US6] Route scheduler and MCP through same controller/run database in `crates/scheduler/src/lib.rs` and `crates/duckle-mcp/src/tools.rs`
- [ ] T062 [US6] Implement CutoverEvidence manifest evaluation and production/test/compatibility/release-CI selection that keeps official runner non-productive until approval in `crates/duckle-db-runner/src/cutover.rs`, `crates/duckdb-engine/src/lib.rs`, `apps/desktop/src/lib.rs`, and `crates/duckle-runner/src/main.rs`
- [ ] T063 [US6] Add regression proving production/release-CI gate rejection, test/compatibility selection, and no post-cutover CLI fallback in `crates/duckdb-engine/tests/quack_runner_parity.rs` and `crates/duckle-runner/src/serve.rs`
- [ ] T064 [US6] Apply and verify effective memory, CPU, spill, and temporary-space profile before worker readiness in `crates/duckle-db-runner/src/local_process_provider.rs` and `crates/duckle-db-runner/src/run_session.rs`
- [ ] T065 [US6] Add bounded-spill, disk-full/quota, readiness-rejection, current/peak-metric, invalid-profile, and runner-unavailable integration coverage in `crates/duckle-db-runner/tests/run_session.rs`
- [ ] T066 [US6] Add selection and correctness coverage for SQL remote, Quack transfer, and Parquet decision table in `crates/duckdb-engine/tests/quack_runner_parity.rs`
- [ ] T067 [US6] Create frozen cutover manifest and reproducible benchmark harness with build/hardware/dataset/seed/warm-up/repetitions/thresholds, CLI baseline, and 1/2/4/8 crossover report in `crates/duckle-db-runner/src/cutover.rs`, `crates/duckdb-engine/benches/quack_transfer.rs`, and `specs/003-quack-sidecar-database-runner/quickstart.md`
- [ ] T068 [US6] Inventory and route inspect, drift, and branch/diff through WorkerPoolControl and RunDatabase in `crates/duckdb-engine/src/lib.rs` and `crates/duckdb-engine/tests/quack_runner_parity.rs`
- [ ] T069 [US6] Verify requested/effective profile consistency for desktop, headless, scheduler, and MCP in `apps/desktop/src/lib.rs`, `crates/duckle-runner/src/main.rs`, `crates/scheduler/src/lib.rs`, and `crates/duckle-mcp/src/tools.rs`
- [ ] T070 [US6] Verify permitted stage/attempt/duration/rows/bytes/transport/memory/spill/CPU telemetry, redaction, retention routing, and warm/on-demand distinction in `crates/duckle-db-runner/tests/autoscaler.rs` and `crates/duckdb-engine/tests/quack_runner_lifecycle.rs`
- [ ] T071 [US6] After CutoverEvidence has all applicable SC pass, named owner/approver, and resolved-or-explicitly-accepted findings, enable official runner and remove CLI, affinity, and compatibility selection in `crates/duckdb-engine/`, `apps/desktop/src/engine_manager.rs`, `crates/duckle-runner/`, `crates/duckle-mcp/`, and `.github/workflows/`
- [ ] T072 [US6] Remove `spikes/quack-sidecar-phase0-spike/` and retain historical documentation after official runner gates pass
- [ ] T073 [US6] Preserve readable-but-disabled SlothDB and xf.dbt with no-fallback diagnostics in `crates/duckdb-engine/src/lib.rs`, `apps/desktop/src/lib.rs`, and `frontend/src/`
- [ ] T074 [US6] Add clean offline package and mismatch smoke coverage after package staging completes in `.github/workflows/runner-package-smoke.yml`

---

## Phase 10: Polish and final gates

- [ ] T075 [P] Document pool states, profile application, autoscale formula, and diagnostics in `docs/architecture/adr-quack-sidecar-runner.md` and `specs/003-quack-sidecar-database-runner/_readme.md`
- [ ] T076 [P] Assert autoscale telemetry has capacity, demand, peak, target, reason, and outcome without sensitive fields in `crates/duckle-db-runner/tests/autoscaler.rs`
- [ ] T077 Record immutable CutoverEvidence results: parity, threshold-gap, benchmark, owner/approver, and finding resolution or explicit acceptance with motivation in `specs/003-quack-sidecar-database-runner/quickstart.md`
- [ ] T078 Run formatting, clippy, and workspace tests from `Cargo.toml`
- [ ] T079 Run frontend install, lint, and build from `frontend/package.json`
- [ ] T080 Scan and resolve final production references to CLI, affinity, and Phase 0 spike in `crates/`, `apps/`, `frontend/`, `.github/`, and `docs/`

---

## Dependencies and execution order

```text
Phase 1 -> Phase 2 -> US4 WorkerPoolControl
                         |- US7 Resource profile
                         |- US1 Per-run database -> US2 Catalog/concurrency
                         |                         -> US3 Lifecycle
                         `- US5 Security
All story gates -> US6 gate evidence (T062–T070) -> production cutover (T071–T074) -> Phase 10
```

- US4 is mandatory for all allocation; US7 depends on pool lifecycle.
- US1 depends on US4; US2 and US3 depend on US1.
- US5 begins with foundational security and must pass before US6.
- US6 completes security, package, benchmark, entry-point, and requirements-quality evidence before T071 removes CLI, affinity remnants, and the retained spike.

## Parallel opportunities

- T001/T002, T006/T007, and each non-overlapping `[P]` task can run in parallel.
- T023/T024 can proceed after pool implementation; T033-T035 after profile implementation.
- US2, US3, and non-overlapping US5 work can run in parallel after US1 is stable.
- T074 can be prepared with packaging but executed only after T058-T073.

## Implementation strategy

### MVP first

Complete Phase 1, Phase 2, US4, and the minimal US1 route. A run may not spawn directly: WorkerPoolControl must lease or decide an on-demand worker.

### Completion condition

Complete only after parity, 100-run pool, profile drain, redaction, cancellation/cleanup, offline package, and zero-production-reference gates pass.
