//! T063 regressions for the headless web/serve entry points.

use duckle_db_runner::cutover::{CutoverGate, EntryPointClass};
use duckle_duckdb_engine::{DuckdbEngine, ExecutionRoute, PipelineDoc};
use std::path::PathBuf;

#[test]
fn serve_run_paths_use_the_workspace_controller_without_global_serialization() {
    let source = include_str!("../src/serve.rs");

    assert!(
        !source.contains("run_lock"),
        "serve must not reintroduce a global run serialization lock"
    );
    assert!(
        source.matches("runner_controller::engine_for_workspace").count() >= 4,
        "manual, streamed, scheduled, and preview execution must resolve through the workspace controller"
    );
}

#[test]
fn approved_web_execution_fails_closed_without_a_controller() {
    let engine = DuckdbEngine::new(PathBuf::from("definitely-missing-duckdb-cli"))
        .with_runner_selection(EntryPointClass::Production, &CutoverGate::Approved);
    assert_eq!(engine.execution_route(), ExecutionRoute::OfficialRunner);

    let result = engine.execute_pipeline(&PipelineDoc {
        nodes: Vec::new(),
        edges: Vec::new(),
    });
    assert_eq!(result.status, "error");
    assert_eq!(
        result.error.as_deref(),
        Some("runner_unavailable"),
        "the post-cutover web path must not silently fall back to the DuckDB CLI"
    );
}
