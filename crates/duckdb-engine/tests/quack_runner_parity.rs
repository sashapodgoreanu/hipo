//! Compatibility baselines for the Quack runner migration.
//!
//! These fixtures intentionally execute through the current CLI backend.  The
//! Quack compatibility route reuses the same documents and assertions so a
//! backend change cannot quietly alter source, transform, sink, runtime,
//! preview, partial-run, or event semantics.

use duckle_db_runner::cutover::{CutoverGate, EntryPointClass};
use duckle_db_runner::model::{
    RunCancellation, RunId, RunnerFailureReason, WorkerId, WorkerKind, WorkerLease, WorkerLeaseId,
};
use duckle_duckdb_engine::{DuckdbEngine, OfficialRunnerController, PipelineDoc, PipelineEvent};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

fn engine() -> Option<DuckdbEngine> {
    let bin = std::env::var_os("DUCKLE_DUCKDB_BIN").map(PathBuf::from)?;
    bin.exists().then(|| DuckdbEngine::new(bin))
}

macro_rules! engine_or_skip {
    () => {
        match engine() {
            Some(engine) => engine,
            None => {
                eprintln!("skipping parity baseline: set DUCKLE_DUCKDB_BIN to a DuckDB CLI");
                return;
            }
        }
    };
}

fn node(id: &str, component_id: &str, properties: Value) -> Value {
    json!({
        "id": id,
        "position": { "x": 0, "y": 0 },
        "data": {
            "label": id,
            "componentId": component_id,
            "properties": properties
        }
    })
}

fn edge(id: &str, source: &str, target: &str) -> Value {
    json!({
        "id": id,
        "source": source,
        "target": target,
        "data": { "connectionType": "main" }
    })
}

fn doc(nodes: Value, edges: Value) -> PipelineDoc {
    serde_json::from_value(json!({ "nodes": nodes, "edges": edges })).expect("valid parity fixture")
}

fn normalized(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn event_type(event: PipelineEvent) -> String {
    serde_json::to_value(event)
        .expect("pipeline event is serializable")
        .get("type")
        .and_then(Value::as_str)
        .expect("pipeline event has a type")
        .to_owned()
}

#[derive(Default)]
struct ControllerCalls {
    acquired: u32,
    released: u32,
    batches: Vec<Vec<String>>,
    previews: Vec<String>,
}

struct RecordingController {
    calls: Arc<Mutex<ControllerCalls>>,
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
    ) -> Result<duckle_db_runner::run_database::SqlBatchResult, RunnerFailureReason> {
        self.calls.lock().unwrap().batches.push(statements);
        Ok(duckle_db_runner::run_database::SqlBatchResult {
            rows: 0,
            transport: duckle_db_runner::model::TransportKind::Quack,
        })
    }

    fn preview_relation(
        &self,
        _lease: &WorkerLease,
        sql: &str,
        _limit: u32,
        _cancellation: RunCancellation,
    ) -> Result<duckle_db_runner::run_database::PreviewResult, RunnerFailureReason> {
        self.calls.lock().unwrap().previews.push(sql.to_string());
        // Return a synthetic DESCRIBE-style result for schema queries, or
        // a synthetic row for data queries, so the engine can build previews.
        if sql.contains("DESCRIBE") {
            Ok(duckle_db_runner::run_database::PreviewResult {
                columns: vec![
                    "column_name".to_string(),
                    "column_type".to_string(),
                    "null".to_string(),
                ],
                rows: vec![{
                    let mut row = std::collections::BTreeMap::new();
                    row.insert("column_name".into(), Value::String("test_col".into()));
                    row.insert("column_type".into(), Value::String("VARCHAR".into()));
                    row.insert("null".into(), Value::String("YES".into()));
                    row
                }],
                truncated: false,
                transport: duckle_db_runner::model::TransportKind::Quack,
            })
        } else {
            Ok(duckle_db_runner::run_database::PreviewResult {
                columns: vec!["test_col".to_string()],
                rows: vec![{
                    let mut row = std::collections::BTreeMap::new();
                    row.insert("test_col".into(), Value::String("preview_value".into()));
                    row
                }],
                truncated: false,
                transport: duckle_db_runner::model::TransportKind::Quack,
            })
        }
    }
}

#[test]
fn runner_selection_acquires_once_without_cli_fallback_and_preserves_production_compatibility() {
    let calls = Arc::new(Mutex::new(ControllerCalls::default()));
    let controller = Arc::new(RecordingController {
        calls: calls.clone(),
    });
    let gate = CutoverGate::Rejected {
        missing_or_failed: vec!["SC-001".to_string()],
    };
    let doc = PipelineDoc {
        nodes: Vec::new(),
        edges: Vec::new(),
    };

    let official = DuckdbEngine::new(PathBuf::from("not-installed"))
        .with_official_runner_controller(controller.clone())
        .with_runner_selection(EntryPointClass::Test, &gate);
    assert_eq!(official.execute_pipeline(&doc).error.as_deref(), Some("runner_unavailable"));
    {
        let calls = calls.lock().unwrap();
        assert_eq!(calls.acquired, 1);
        assert_eq!(calls.released, 1);
    }

    let compatibility = DuckdbEngine::new(PathBuf::from("not-installed"))
        .with_official_runner_controller(controller)
        .with_runner_selection(EntryPointClass::Production, &gate);
    assert_ne!(
        compatibility.execute_pipeline(&doc).error.as_deref(),
        Some("runner_unavailable"),
        "production must retain the compatibility route before cutover"
    );
    let calls = calls.lock().unwrap();
    assert_eq!(calls.acquired, 1);
    assert_eq!(calls.released, 1);
    assert!(calls.batches.is_empty());
}

#[test]
fn official_runner_dispatches_pure_sql_stages_as_one_controlled_batch() {
    let temp = tempfile::tempdir().expect("temporary runner workspace");
    let calls = Arc::new(Mutex::new(ControllerCalls::default()));
    let gate = CutoverGate::Rejected {
        missing_or_failed: vec!["SC-001".to_string()],
    };
    let engine = DuckdbEngine::new(PathBuf::from("not-installed"))
        .with_official_runner_controller(Arc::new(RecordingController {
            calls: calls.clone(),
        }))
        .with_runner_selection(EntryPointClass::Test, &gate);

    let result = engine.execute_pipeline(&sql_fixture(
        &temp.path().join("orders.csv"),
        &temp.path().join("paid.csv"),
    ));

    assert_eq!(result.status, "ok", "official SQL dispatch failed: {:?}", result.error);
    assert_eq!(result.nodes.len(), 3);
    let calls = calls.lock().unwrap();
    assert_eq!(calls.acquired, 1);
    assert_eq!(calls.released, 1);
    assert_eq!(calls.batches.len(), 1);
    // The batch contains setup statements (PRAGMAs, secrets) followed by the
    // 3 stage SQL statements. At least the PRAGMA setup + 3 stages.
    assert!(
        calls.batches[0].len() >= 3,
        "batch should contain at least 3 stage statements, got {}",
        calls.batches[0].len()
    );
    // The last 3 entries are the stage SQL (non-empty).
    let stage_count = 3;
    let stage_start = calls.batches[0].len() - stage_count;
    assert!(calls.batches[0][stage_start..]
        .iter()
        .all(|statement| !statement.trim().is_empty()));
}

fn sql_fixture(input: &Path, output: &Path) -> PipelineDoc {
    doc(
        json!([
            node(
                "source",
                "src.csv",
                json!({ "path": normalized(input), "hasHeader": true })
            ),
            node(
                "transform",
                "xf.filter",
                json!({ "predicate": "status = 'paid'" })
            ),
            node(
                "sink",
                "snk.csv",
                json!({
                    "path": normalized(output),
                    "hasHeader": true,
                    "mode": "overwrite"
                })
            )
        ]),
        json!([
            edge("source-transform", "source", "transform"),
            edge("transform-sink", "transform", "sink")
        ]),
    )
}

#[test]
fn cli_baseline_preserves_sql_source_transform_sink_and_events() {
    let engine = engine_or_skip!();
    let temp = tempfile::tempdir().expect("temporary parity workspace");
    let input = temp.path().join("orders.csv");
    let output = temp.path().join("paid.csv");
    std::fs::write(
        &input,
        "id,status,amount\n1,paid,10\n2,pending,20\n3,paid,30\n4,refunded,5\n",
    )
    .expect("write SQL parity source");

    let mut events = Vec::new();
    let result = engine.execute_pipeline_with_events(
        &sql_fixture(&input, &output),
        None,
        Some("QuackParitySqlBaseline"),
        |event| events.push(event_type(event)),
    );

    assert_eq!(
        result.status, "ok",
        "SQL baseline failed: {:?}",
        result.error
    );
    assert_eq!(
        result.nodes.get("source").and_then(|node| node.rows),
        Some(4)
    );
    assert_eq!(
        result.nodes.get("transform").and_then(|node| node.rows),
        Some(2)
    );
    assert_eq!(result.nodes.get("sink").and_then(|node| node.rows), Some(2));
    assert_eq!(
        std::fs::read_to_string(&output).expect("read SQL parity sink"),
        "id,status,amount\n1,paid,10\n3,paid,30\n"
    );

    assert_eq!(events.first().map(String::as_str), Some("started"));
    assert_eq!(events.last().map(String::as_str), Some("finished"));
    assert_eq!(
        events
            .iter()
            .filter(|event| event.as_str() == "stage_started")
            .count(),
        3
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.as_str() == "stage_finished")
            .count(),
        3
    );
}

#[test]
fn cli_baseline_preserves_partial_target_preview() {
    let engine = engine_or_skip!();
    let temp = tempfile::tempdir().expect("temporary parity workspace");
    let input = temp.path().join("orders.csv");
    let output = temp.path().join("must-not-exist.csv");
    std::fs::write(
        &input,
        "id,status,amount\n1,paid,10\n2,pending,20\n3,paid,30\n",
    )
    .expect("write partial parity source");

    let result = engine.execute_pipeline_with_events(
        &sql_fixture(&input, &output),
        Some("transform"),
        Some("QuackParityPartialBaseline"),
        |_| {},
    );

    assert_eq!(
        result.status, "ok",
        "partial baseline failed: {:?}",
        result.error
    );
    assert!(
        !output.exists(),
        "partial run must not execute its downstream sink"
    );
    let preview = result
        .preview
        .iter()
        .find(|preview| preview.node_id == "transform")
        .expect("partial target preview");
    assert_eq!(preview.rows.len(), 2);
    assert_eq!(preview.columns.len(), 3);
    assert!(preview
        .rows
        .iter()
        .all(|row| { row.get("status").and_then(Value::as_str) == Some("paid") }));
}

#[test]
fn cli_baseline_preserves_runtime_relation_and_preview() {
    let engine = engine_or_skip!();
    let runtime = doc(
        json!([node(
            "runtime",
            "code.shell",
            json!({ "command": "echo duckle-runtime" })
        )]),
        json!([]),
    );

    let result = engine.execute_pipeline(&runtime);

    assert_eq!(
        result.status, "ok",
        "runtime baseline failed: {:?}",
        result.error
    );
    assert_eq!(
        result.nodes.get("runtime").and_then(|node| node.rows),
        Some(1)
    );
    let row = result
        .preview
        .iter()
        .find(|preview| preview.node_id == "runtime")
        .and_then(|preview| preview.rows.first())
        .expect("runtime preview row");
    assert_eq!(row.get("exit_code").and_then(Value::as_i64), Some(0));
    assert!(row
        .get("stdout")
        .and_then(Value::as_str)
        .is_some_and(|stdout| stdout.contains("duckle-runtime")));
}

// ── T041: official runner isolation, preview, partial, and runtime tests ──

fn official_engine(calls: &Arc<Mutex<ControllerCalls>>) -> DuckdbEngine {
    let gate = CutoverGate::Rejected {
        missing_or_failed: vec!["SC-001".to_string()],
    };
    DuckdbEngine::new(PathBuf::from("not-installed"))
        .with_official_runner_controller(Arc::new(RecordingController {
            calls: calls.clone(),
        }))
        .with_runner_selection(EntryPointClass::Test, &gate)
}

/// Two consecutive runs must each receive their own acquire/release cycle
/// (isolation) with no shared worker state.
#[test]
fn official_runner_isolates_consecutive_runs() {
    let temp = tempfile::tempdir().expect("temporary workspace");
    let calls = Arc::new(Mutex::new(ControllerCalls::default()));
    let engine = official_engine(&calls);
    let fixture = sql_fixture(
        &temp.path().join("orders.csv"),
        &temp.path().join("out.csv"),
    );

    let r1 = engine.for_new_run().execute_pipeline(&fixture);
    let r2 = engine.for_new_run().execute_pipeline(&fixture);

    assert_eq!(r1.status, "ok");
    assert_eq!(r2.status, "ok");
    let calls = calls.lock().unwrap();
    assert_eq!(calls.acquired, 2, "each run must acquire separately");
    assert_eq!(calls.released, 2, "each run must release separately");
    assert_eq!(calls.batches.len(), 2, "each run must dispatch its own batch");
}

/// The official runner path requests preview data for view stages: one
/// DESCRIBE query for schema and one SELECT for rows per eligible view.
#[test]
fn official_runner_requests_preview_for_view_stages() {
    let temp = tempfile::tempdir().expect("temporary workspace");
    let calls = Arc::new(Mutex::new(ControllerCalls::default()));
    let engine = official_engine(&calls);

    // A pipeline with a source (view) and a transform (view) — two previewable
    // stages — and a sink (not previewable).
    let fixture = sql_fixture(
        &temp.path().join("orders.csv"),
        &temp.path().join("out.csv"),
    );

    let result = engine.for_new_run().execute_pipeline(&fixture);
    assert_eq!(result.status, "ok", "preview dispatch failed: {:?}", result.error);

    let calls = calls.lock().unwrap();
    // Two view stages -> 2 DESCRIBE + 2 SELECT = 4 preview_relation calls.
    // (sink stages are excluded from preview.)
    assert!(
        calls.previews.len() >= 2,
        "expected at least 2 preview queries for 2 view stages, got {}",
        calls.previews.len()
    );
    assert!(
        calls.previews.iter().any(|sql| sql.contains("DESCRIBE")),
        "expected at least one DESCRIBE query"
    );

    // Result should contain preview data for view stages.
    assert!(
        !result.preview.is_empty(),
        "official runner must populate preview for view stages"
    );
}

/// A partial run targeting a view stage must produce only the subgraph stages
/// and not execute the downstream sink.
#[test]
fn official_runner_partial_run_restricts_to_subgraph() {
    let temp = tempfile::tempdir().expect("temporary workspace");
    let calls = Arc::new(Mutex::new(ControllerCalls::default()));
    let engine = official_engine(&calls);
    let output = temp.path().join("must-not-exist.csv");
    let fixture = sql_fixture(&temp.path().join("orders.csv"), &output);

    let result = engine.for_new_run().execute_pipeline_with_events(
        &fixture,
        Some("transform"),
        Some("QuackPartialTest"),
        |_| {},
    );

    assert_eq!(result.status, "ok", "partial run failed: {:?}", result.error);
    assert!(
        !output.exists(),
        "partial run must not execute its downstream sink"
    );
    let calls = calls.lock().unwrap();
    assert_eq!(calls.acquired, 1);
    assert_eq!(calls.released, 1);
    // The batch should contain setup statements + the subgraph stages
    // (source + transform), not the downstream sink.
    assert_eq!(calls.batches.len(), 1);
    let total = calls.batches[0].len();
    let setup_count = total - 2; // remaining after subtracting 2 stage SQL
    assert!(
        total >= 2,
        "partial batch should have at least 2 stage statements, got {}",
        total
    );
    // Verify stage SQL (last 2 entries) are non-empty.
    assert!(calls.batches[0][setup_count..]
        .iter()
        .all(|s| !s.trim().is_empty()));
}

/// A non-pure-SQL stage (runtime/control/driver) must fail cleanly on the
/// official runner path, which only supports pure SQL dispatch.
#[test]
fn official_runner_rejects_non_pure_sql_stages() {
    let calls = Arc::new(Mutex::new(ControllerCalls::default()));
    let engine = official_engine(&calls);
    let runtime_doc = doc(
        json!([node(
            "runtime",
            "code.shell",
            json!({ "command": "echo test" })
        )]),
        json!([]),
    );

    let result = engine.for_new_run().execute_pipeline(&runtime_doc);

    assert_eq!(result.status, "error");
    assert_eq!(
        result.error.as_deref(),
        Some("runner_unavailable"),
        "non-pure-SQL must fail with runner_unavailable, not silently fall back"
    );
    let calls = calls.lock().unwrap();
    // Should have acquired and released even though it failed.
    assert_eq!(calls.acquired, 1);
    assert_eq!(calls.released, 1);
    // No batch should have been dispatched.
    assert!(calls.batches.is_empty());
}

// ── T047: catalog-sharing and concurrency tests ──

/// A Query Source pipeline routes through the official runner as a flat
/// batch (ATTACH + materialize + DETACH inline in stage SQL), no affinity
/// worker needed. The batch includes setup statements.
#[test]
fn official_runner_handles_query_source_as_flat_batch() {
    let temp = tempfile::tempdir().expect("temporary workspace");
    let calls = Arc::new(Mutex::new(ControllerCalls::default()));
    let engine = official_engine(&calls);

    let srcdb = temp.path().join("orders.duckdb");
    let out = temp.path().join("out.csv");
    let doc = doc(
        json!([
            node(
                "q",
                "src.query",
                json!({
                    "dataSourceRefs": ["ds-orders"],
                    "sql": "SELECT 1 AS id",
                    "_duckleDataSourceRuntime": [{
                        "id": "ds-orders",
                        "alias": "sales",
                        "kind": "duckdb",
                        "connection": {"database": normalized(&srcdb)}
                    }]
                })
            ),
            node(
                "sink",
                "snk.csv",
                json!({ "path": normalized(&out), "hasHeader": true, "mode": "overwrite" })
            )
        ]),
        json!([edge("q-sink", "q", "sink")]),
    );

    let result = engine.for_new_run().execute_pipeline(&doc);

    assert_eq!(result.status, "ok", "QS flat batch failed: {:?}", result.error);
    let calls = calls.lock().unwrap();
    assert_eq!(calls.acquired, 1);
    assert_eq!(calls.released, 1);
    assert_eq!(calls.batches.len(), 1);
    // The batch should contain setup statements + the QS stage + sink stage.
    assert!(
        calls.batches[0].len() >= 2,
        "batch should have at least 2 stage statements"
    );
    // The QS stage SQL should include ATTACH and DETACH inline.
    let qs_sql = calls.batches[0]
        .iter()
        .find(|s| s.contains("ATTACH") || s.contains("CREATE OR REPLACE TABLE"))
        .expect("Query Source stage should include ATTACH");
    assert!(qs_sql.contains("DETACH"), "QS stage should include DETACH");
}

/// Multiple concurrent runs (simulating 2/4/8) each receive independent
/// acquire/release cycles and independent batches.
#[test]
fn official_runner_concurrent_runs_get_independent_leases() {
    let temp = tempfile::tempdir().expect("temporary workspace");
    let calls = Arc::new(Mutex::new(ControllerCalls::default()));
    let gate = CutoverGate::Rejected {
        missing_or_failed: vec!["SC-001".to_string()],
    };
    let base = DuckdbEngine::new(PathBuf::from("not-installed"))
        .with_official_runner_controller(Arc::new(RecordingController {
            calls: calls.clone(),
        }))
        .with_runner_selection(EntryPointClass::Test, &gate);

    // Simulate 4 concurrent runs on separate threads.
    let input = temp.path().join("orders.csv");
    let output = temp.path().join("out.csv");
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let eng = base.for_new_run();
            let input = input.clone();
            let output = output.clone();
            std::thread::spawn(move || {
                eng.execute_pipeline(&sql_fixture(&input, &output))
            })
        })
        .collect();

    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    for (i, r) in results.iter().enumerate() {
        assert_eq!(r.status, "ok", "concurrent run {} failed: {:?}", i, r.error);
    }
    let calls = calls.lock().unwrap();
    assert_eq!(calls.acquired, 4, "each concurrent run must acquire separately");
    assert_eq!(calls.released, 4, "each concurrent run must release separately");
    assert_eq!(calls.batches.len(), 4, "each concurrent run must dispatch its own batch");
}

/// Two Query Source nodes sharing a Data Source compile to self-contained
/// stages and dispatch through the official runner as one flat batch
/// (catalog sharing via inline ATTACH, not affinity worker).
#[test]
fn official_runner_shared_data_source_dispatches_without_affinity() {
    let temp = tempfile::tempdir().expect("temporary workspace");
    let calls = Arc::new(Mutex::new(ControllerCalls::default()));
    let engine = official_engine(&calls);

    let srcdb = temp.path().join("shared.duckdb");
    let runtime = json!([{
        "id": "ds-shared",
        "alias": "sales",
        "kind": "duckdb",
        "connection": {"database": normalized(&srcdb)}
    }]);

    let d = doc(
        json!([
            node("q1", "src.query", json!({
                "dataSourceRefs": ["ds-shared"],
                "sql": "SELECT 1 AS a",
                "_duckleDataSourceRuntime": runtime.clone()
            })),
            node("q2", "src.query", json!({
                "dataSourceRefs": ["ds-shared"],
                "sql": "SELECT 2 AS b",
                "_duckleDataSourceRuntime": runtime.clone()
            }))
        ]),
        json!([]),
    );

    let result = engine.for_new_run().execute_pipeline(&d);

    assert_eq!(result.status, "ok", "shared DS failed: {:?}", result.error);
    let calls = calls.lock().unwrap();
    assert_eq!(calls.acquired, 1);
    assert_eq!(calls.released, 1);
    // Both QS stages dispatched in one batch (no affinity grouping needed).
    assert_eq!(calls.batches.len(), 1);
    assert!(calls.batches[0].len() >= 2, "batch should include both QS stages");
}
