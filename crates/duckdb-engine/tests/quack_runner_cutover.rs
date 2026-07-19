//! T063 cutover-routing regressions.
//!
//! These tests deliberately do not require a DuckDB CLI or a staged sidecar.
//! They exercise the public routing contract directly so release selection
//! cannot regress into a silent backend fallback.

use duckle_db_runner::cutover::{CutoverGate, EntryPointClass};
use duckle_duckdb_engine::{DuckdbEngine, ExecutionRoute, PipelineDoc};
use std::path::PathBuf;

fn empty_pipeline() -> PipelineDoc {
    PipelineDoc {
        nodes: Vec::new(),
        edges: Vec::new(),
    }
}

#[test]
fn rejected_gate_keeps_production_and_release_ci_on_compatibility() {
    let gate = CutoverGate::Rejected {
        missing_or_failed: vec!["SC-010".to_string()],
    };
    let base = DuckdbEngine::new(PathBuf::from("definitely-missing-duckdb-cli"));

    assert_eq!(
        base.with_runner_selection(EntryPointClass::Production, &gate)
            .execution_route(),
        ExecutionRoute::CliCompatibility
    );
    assert_eq!(
        base.with_runner_selection(EntryPointClass::ReleaseCi, &gate)
            .execution_route(),
        ExecutionRoute::CliCompatibility
    );
}

#[test]
fn test_and_explicit_compatibility_can_exercise_official_runner_pre_gate() {
    let gate = CutoverGate::Rejected {
        missing_or_failed: vec!["SC-001".to_string()],
    };
    let base = DuckdbEngine::new(PathBuf::from("definitely-missing-duckdb-cli"));

    assert_eq!(
        base.with_runner_selection(EntryPointClass::Test, &gate)
            .execution_route(),
        ExecutionRoute::OfficialRunner
    );
    assert_eq!(
        base.with_runner_selection(EntryPointClass::Compatibility, &gate)
            .execution_route(),
        ExecutionRoute::OfficialRunner
    );
}

#[test]
fn approved_production_and_release_ci_never_fall_back_to_cli() {
    let base = DuckdbEngine::new(PathBuf::from("definitely-missing-duckdb-cli"));

    for entry_point in [EntryPointClass::Production, EntryPointClass::ReleaseCi] {
        let engine = base.with_runner_selection(entry_point, &CutoverGate::Approved);
        assert_eq!(engine.execution_route(), ExecutionRoute::OfficialRunner);

        let result = engine.execute_pipeline(&empty_pipeline());
        assert_eq!(result.status, "error");
        assert_eq!(
            result.error.as_deref(),
            Some("runner_unavailable"),
            "an approved official route must fail closed when its controller is unavailable instead of spawning the CLI"
        );
    }
}
