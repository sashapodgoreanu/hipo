//! T073 regressions for legacy features retained only for workspace readability.

use duckle_duckdb_engine::{compile_pipeline_sql, plan, PipelineDoc};
use serde_json::json;

fn dbt_pipeline(disabled: bool) -> PipelineDoc {
    serde_json::from_value(json!({
        "nodes": [{
            "id": "legacy-dbt",
            "position": { "x": 0, "y": 0 },
            "data": {
                "label": "Legacy dbt",
                "componentId": "xf.dbt",
                "disabled": disabled,
                "properties": {
                    "dbtBin": "definitely-missing-dbt",
                    "projectPath": "legacy-project"
                }
            }
        }],
        "edges": []
    }))
    .expect("valid persisted legacy pipeline")
}

#[test]
fn disabled_xf_dbt_remains_readable_and_is_skipped_by_planning() {
    let pipeline = dbt_pipeline(true);
    assert_eq!(
        pipeline.nodes[0].data.component_id.as_deref(),
        Some("xf.dbt"),
        "loading a persisted pipeline must preserve its legacy component id"
    );

    let compiled = plan::compile(&pipeline).expect("disabled legacy node remains readable");
    assert!(compiled.stages.is_empty());
}

#[test]
fn enabled_xf_dbt_fails_with_an_explicit_no_fallback_diagnostic() {
    let pipeline = dbt_pipeline(false);
    let error = plan::compile(&pipeline).expect_err("xf.dbt must remain disabled");
    let message = error.to_string();

    assert!(message.contains("component_disabled"), "{message}");
    assert!(message.contains("xf.dbt"), "{message}");
    assert!(message.contains("sidecar runner migration"), "{message}");
    assert!(!message.contains("definitely-missing-dbt"), "planner attempted a dbt fallback: {message}");

    let exported = compile_pipeline_sql(&pipeline)
        .expect_err("SQL export must use the same disabled-component gate")
        .to_string();
    assert!(exported.contains("component_disabled"), "{exported}");
    assert!(!exported.contains("definitely-missing-dbt"), "{exported}");
}
