//! T068 integration coverage for inspect, schema drift, and branch/data diff.

use duckle_db_runner::cutover::{CutoverGate, EntryPointClass};
use duckle_db_runner::model::{
    RunCancellation, RunId, RunnerFailureReason, TransportKind, WorkerId, WorkerKind, WorkerLease,
    WorkerLeaseId,
};
use duckle_db_runner::run_database::{PreviewResult, SqlBatchResult};
use duckle_duckdb_engine::drift::{schema_drift, RunDatabaseDataTools};
use duckle_duckdb_engine::{DuckdbEngine, OfficialRunnerController, PipelineDoc};
use duckle_metadata::{Column, DataType};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Calls {
    acquired: u32,
    released: u32,
    batches: Vec<Vec<String>>,
    previews: Vec<String>,
}

struct RecordingController {
    calls: Arc<Mutex<Calls>>,
}

impl OfficialRunnerController for RecordingController {
    fn acquire(
        &self,
        run_id: RunId,
        _attempt: u32,
        _cancellation: RunCancellation,
        _now_millis: u64,
    ) -> Result<WorkerLease, RunnerFailureReason> {
        self.calls.lock().unwrap().acquired += 1;
        Ok(WorkerLease {
            lease_id: WorkerLeaseId::new(),
            worker_id: WorkerId::new(),
            run_id,
            worker_kind: WorkerKind::OnDemand,
            profile_version: 1,
        })
    }

    fn release(&self, _lease: WorkerLease, _now_millis: u64) {
        self.calls.lock().unwrap().released += 1;
    }

    fn execute_batch(
        &self,
        _lease: &WorkerLease,
        statements: Vec<String>,
        _cancellation: RunCancellation,
    ) -> Result<SqlBatchResult, RunnerFailureReason> {
        self.calls.lock().unwrap().batches.push(statements);
        Ok(SqlBatchResult {
            rows: 0,
            transport: TransportKind::Quack,
        })
    }

    fn preview_relation(
        &self,
        _lease: &WorkerLease,
        sql: &str,
        _limit: u32,
        _cancellation: RunCancellation,
    ) -> Result<PreviewResult, RunnerFailureReason> {
        self.calls.lock().unwrap().previews.push(sql.to_string());
        if sql.trim_start().starts_with("DESCRIBE") {
            let mut row = BTreeMap::new();
            row.insert("column_name".into(), Value::String("id".into()));
            row.insert("column_type".into(), Value::String("BIGINT".into()));
            row.insert("null".into(), Value::String("YES".into()));
            Ok(PreviewResult {
                columns: vec![
                    "column_name".into(),
                    "column_type".into(),
                    "null".into(),
                ],
                rows: vec![row],
                truncated: false,
                transport: TransportKind::Quack,
            })
        } else {
            let mut row = BTreeMap::new();
            row.insert("id".into(), Value::from(1));
            Ok(PreviewResult {
                columns: vec!["id".into()],
                rows: vec![row],
                truncated: false,
                transport: TransportKind::Quack,
            })
        }
    }
}

fn official_engine(calls: &Arc<Mutex<Calls>>) -> DuckdbEngine {
    DuckdbEngine::new(PathBuf::from("definitely-missing-duckdb-cli"))
        .with_official_runner_controller(Arc::new(RecordingController {
            calls: calls.clone(),
        }))
        .with_runner_selection(
            EntryPointClass::Test,
            &CutoverGate::Rejected {
                missing_or_failed: vec!["SC-001".into()],
            },
        )
}

#[test]
fn inspect_uses_one_controller_lease_for_setup_schema_and_sample() {
    let calls = Arc::new(Mutex::new(Calls::default()));
    let engine = official_engine(&calls);

    let inspection = engine
        .inspect_via_run_database(
            "duckdb",
            json!({
                "database": "ignored.duckdb",
                "table": "orders"
            }),
        )
        .unwrap();

    assert_eq!(inspection.schema.len(), 1);
    assert_eq!(inspection.schema[0].name, "id");
    assert_eq!(inspection.sample_rows, vec![json!({ "id": 1 })]);

    let calls = calls.lock().unwrap();
    assert_eq!(calls.acquired, 1);
    assert_eq!(calls.released, 1);
    assert_eq!(calls.batches.len(), 1, "ATTACH/setup must use the same lease");
    assert_eq!(calls.previews.len(), 2, "DESCRIBE and sample are separate queries");
}

#[test]
fn schema_drift_reuses_runner_routed_inspection() {
    let calls = Arc::new(Mutex::new(Calls::default()));
    let engine = official_engine(&calls);
    let mut doc: PipelineDoc = serde_json::from_value(json!({
        "nodes": [{
            "id": "source",
            "position": { "x": 0, "y": 0 },
            "data": {
                "label": "source",
                "componentId": "src.csv",
                "properties": { "path": "ignored.csv", "hasHeader": true }
            }
        }],
        "edges": []
    }))
    .unwrap();
    doc.nodes[0].data.schema = Some(vec![Column {
        name: "id".into(),
        data_type: DataType::Int64,
        nullable: true,
        primary_key: None,
        format: None,
    }]);

    let report = schema_drift(&engine, &doc);

    assert_eq!(report["summary"]["sourcesChecked"], json!(1));
    assert_eq!(report["summary"]["unreadable"], json!(0));
    assert_eq!(report["hasBreaking"], json!(false));
    let calls = calls.lock().unwrap();
    assert_eq!(calls.acquired, 1);
    assert_eq!(calls.released, 1);
    assert_eq!(calls.previews.len(), 2);
}

#[test]
fn branch_diff_setup_and_query_share_one_run_database_lease() {
    let calls = Arc::new(Mutex::new(Calls::default()));
    let engine = official_engine(&calls);

    let rows = engine
        .branch_diff_rows_via_run_database(
            vec![
                "ATTACH 'before.duckdb' AS before_branch (READ_ONLY)".into(),
                "ATTACH 'after.duckdb' AS after_branch (READ_ONLY)".into(),
            ],
            "SELECT id FROM after_branch.orders EXCEPT SELECT id FROM before_branch.orders",
            1_000,
        )
        .unwrap();

    assert_eq!(rows, vec![json!({ "id": 1 })]);
    let calls = calls.lock().unwrap();
    assert_eq!(calls.acquired, 1);
    assert_eq!(calls.released, 1);
    assert_eq!(calls.batches.len(), 1);
    assert_eq!(calls.batches[0].len(), 2);
    assert_eq!(calls.previews.len(), 1);
    assert!(calls.previews[0].contains("EXCEPT"));
}
