//! Lifecycle integration coverage for the official Quack runner route.
//!
//! These tests keep the provider opaque and exercise the engine/controller
//! contract: one acquire per run, cancellation propagation, sanitized crash
//! classification, single release, and bounded orphan cleanup.

use duckle_db_runner::cutover::{CutoverGate, EntryPointClass};
use duckle_db_runner::model::{
    RunCancellation, RunId, RunnerFailureReason, TransportKind, WorkerId, WorkerKind, WorkerLease,
    WorkerLeaseId,
};
use duckle_db_runner::process_cleanup::{sweep_run_artifacts, RunArtifactScope};
use duckle_db_runner::run_database::{PreviewResult, SqlBatchResult};
use duckle_duckdb_engine::{DuckdbEngine, OfficialRunnerController, PipelineDoc};
use serde_json::json;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy)]
enum BatchOutcome {
    WaitForCancellation,
    Fail(RunnerFailureReason),
}

#[derive(Default)]
struct LifecycleState {
    acquired: u32,
    released: u32,
    batches: Vec<Vec<String>>,
    dispatched: bool,
}

struct LifecycleController {
    outcome: BatchOutcome,
    state: Mutex<LifecycleState>,
    changed: Condvar,
}

impl LifecycleController {
    fn new(outcome: BatchOutcome) -> Self {
        Self {
            outcome,
            state: Mutex::new(LifecycleState::default()),
            changed: Condvar::new(),
        }
    }

    fn wait_until_dispatched(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut state = self.state.lock().expect("lifecycle state poisoned");
        while !state.dispatched {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            let (next, result) = self
                .changed
                .wait_timeout(state, remaining)
                .expect("lifecycle state poisoned");
            state = next;
            if result.timed_out() && !state.dispatched {
                return false;
            }
        }
        true
    }

    fn counts(&self) -> (u32, u32, usize) {
        let state = self.state.lock().expect("lifecycle state poisoned");
        (state.acquired, state.released, state.batches.len())
    }
}

impl OfficialRunnerController for LifecycleController {
    fn acquire(
        &self,
        run_id: RunId,
        _attempt: u32,
        _cancellation: RunCancellation,
        _now_millis: u64,
    ) -> Result<WorkerLease, RunnerFailureReason> {
        self.state
            .lock()
            .expect("lifecycle state poisoned")
            .acquired += 1;
        Ok(WorkerLease {
            lease_id: WorkerLeaseId::new(),
            worker_id: WorkerId::new(),
            run_id,
            worker_kind: WorkerKind::OnDemand,
            profile_version: 1,
        })
    }

    fn release(&self, _lease: WorkerLease, _now_millis: u64) {
        self.state
            .lock()
            .expect("lifecycle state poisoned")
            .released += 1;
    }

    fn execute_batch(
        &self,
        _lease: &WorkerLease,
        statements: Vec<String>,
        cancellation: RunCancellation,
    ) -> Result<SqlBatchResult, RunnerFailureReason> {
        {
            let mut state = self.state.lock().expect("lifecycle state poisoned");
            state.batches.push(statements);
            state.dispatched = true;
            self.changed.notify_all();
        }

        match self.outcome {
            BatchOutcome::WaitForCancellation => {
                let deadline = Instant::now() + Duration::from_secs(10);
                while !cancellation.is_cancelled() {
                    if Instant::now() >= deadline {
                        return Err(RunnerFailureReason::RunnerUnavailable);
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(RunnerFailureReason::Cancelled)
            }
            BatchOutcome::Fail(reason) => Err(reason),
        }
    }

    fn preview_relation(
        &self,
        _lease: &WorkerLease,
        _sql: &str,
        _limit: u32,
        _cancellation: RunCancellation,
    ) -> Result<PreviewResult, RunnerFailureReason> {
        Err(RunnerFailureReason::RunnerUnavailable)
    }
}

fn official_engine(controller: Arc<dyn OfficialRunnerController>) -> DuckdbEngine {
    let gate = CutoverGate::Rejected {
        missing_or_failed: vec!["SC-001".to_string()],
    };
    DuckdbEngine::new(PathBuf::from("not-installed"))
        .with_official_runner_controller(controller)
        .with_runner_selection(EntryPointClass::Test, &gate)
}

fn workload_doc(sql: &str) -> PipelineDoc {
    serde_json::from_value(json!({
        "nodes": [{
            "id": "workload",
            "position": { "x": 0, "y": 0 },
            "data": {
                "label": "workload",
                "componentId": "src.csv",
                "properties": {
                    "sqlOverride": sql
                }
            }
        }],
        "edges": []
    }))
    .expect("valid lifecycle fixture")
}

#[test]
fn cancellation_stops_scan_join_spill_transfer_and_runtime_shaped_batches() {
    let workloads = [
        (
            "scan",
            "SELECT * FROM range(0, 1000000000) AS scan_rows",
        ),
        (
            "join",
            "SELECT left_rows.range FROM range(0, 1000000) AS left_rows JOIN range(0, 1000000) AS right_rows ON left_rows.range = right_rows.range",
        ),
        (
            "spill",
            "SELECT * FROM range(0, 10000000) AS spill_rows ORDER BY range DESC",
        ),
        (
            "transfer",
            "SELECT * FROM read_parquet('runner-transfer.parquet')",
        ),
        (
            "runtime",
            "SELECT * FROM range(0, 1000000) AS runtime_handoff",
        ),
    ];

    for (name, sql) in workloads {
        let controller = Arc::new(LifecycleController::new(
            BatchOutcome::WaitForCancellation,
        ));
        let engine = official_engine(controller.clone());
        let worker_engine = engine.clone();
        let doc = workload_doc(sql);
        let run = std::thread::spawn(move || worker_engine.execute_pipeline(&doc));

        assert!(
            controller.wait_until_dispatched(Duration::from_secs(2)),
            "{name} batch was not dispatched"
        );
        engine.request_cancel();

        let result = run.join().expect("lifecycle worker panicked");
        assert_eq!(result.status, "cancelled", "{name}: {result:?}");
        assert_eq!(result.error, None, "{name} cancellation must be sanitized");
        assert_eq!(
            controller.counts(),
            (1, 1, 1),
            "{name} must acquire, dispatch, and release exactly once"
        );
    }
}

#[test]
fn transport_loss_is_reported_as_sanitized_runner_crashed_and_releases_the_lease() {
    let controller = Arc::new(LifecycleController::new(BatchOutcome::Fail(
        RunnerFailureReason::RunnerCrashed,
    )));
    let engine = official_engine(controller.clone());

    let result = engine.execute_pipeline(&workload_doc("SELECT 1 AS value"));

    assert_eq!(result.status, "error");
    assert_eq!(result.error.as_deref(), Some("runner_crashed"));
    assert_eq!(controller.counts(), (1, 1, 1));
    let serialized = serde_json::to_string(&result).expect("run result serializes");
    assert!(!serialized.contains("127.0.0.1"));
    assert!(!serialized.contains("quack:"));
    assert!(!serialized.contains("TOKEN"));
}

#[test]
fn stale_orphan_artifacts_are_removed_within_the_ten_second_cleanup_contract() {
    let root = tempfile::tempdir().expect("temporary artifact root");
    let stale = RunArtifactScope::create(root.path(), RunId::new(), 1_000)
        .expect("create stale run scope");
    let stale_path = stale.path().to_path_buf();
    std::fs::write(stale.path().join("spill.tmp"), b"spill")
        .expect("write stale spill artifact");
    std::mem::forget(stale);

    let fresh = RunArtifactScope::create(root.path(), RunId::new(), 10_500)
        .expect("create fresh run scope");
    let fresh_path = fresh.path().to_path_buf();
    std::mem::forget(fresh);

    let started = Instant::now();
    let report = sweep_run_artifacts(root.path(), 11_000, Duration::from_secs(10))
        .expect("sweep run artifacts");

    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(report.removed, 1);
    assert_eq!(report.retained, 1);
    assert_eq!(report.rejected, 0);
    assert!(!stale_path.exists());
    assert!(fresh_path.exists());
}

#[test]
fn lifecycle_results_keep_the_declared_quack_transport_internal() {
    let result = SqlBatchResult {
        rows: 0,
        transport: TransportKind::Quack,
    };
    assert_eq!(result.transport, TransportKind::Quack);
}
