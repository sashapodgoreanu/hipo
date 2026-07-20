use duckle_db_runner::cutover::{evaluate_cutover_json, CutoverGate};

const CUTOVER_TEMPLATE: &str = include_str!(
    "../../../specs/003-quack-sidecar-database-runner/cutover-evidence.template.json"
);

#[test]
fn cutover_template_is_parseable_but_cannot_approve_production() {
    let gate = evaluate_cutover_json(Some(CUTOVER_TEMPLATE));
    let CutoverGate::Rejected { missing_or_failed } = gate else {
        panic!("the unfilled cutover template must never approve production");
    };

    assert!(!missing_or_failed
        .iter()
        .any(|item| item == "cutover_manifest_parse"));
    for required_rejection in [
        "release_id",
        "technical_owner",
        "release_approver",
        "benchmark_evidence",
        "bundle_evidence",
        "bundle_identity",
        "SC-001",
        "SC-009",
        "SC-010",
        "SC-011",
        "finding:benchmark-thresholds",
    ] {
        assert!(
            missing_or_failed
                .iter()
                .any(|item| item == required_rejection),
            "missing expected fail-closed diagnostic {required_rejection}: {missing_or_failed:?}"
        );
    }
}
