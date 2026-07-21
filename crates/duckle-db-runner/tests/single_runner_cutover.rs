use duckle_db_runner::cutover::{
    select_runner, CutoverGate, EntryPointClass, RunnerSelection,
};

#[test]
fn every_entry_point_uses_the_single_quack_runner() {
    let rejected = CutoverGate::Rejected {
        missing_or_failed: vec!["legacy_gate_is_not_a_runtime_selector".to_string()],
    };
    let approved = CutoverGate::Approved;

    for gate in [&rejected, &approved] {
        for entry_point in [
            EntryPointClass::Production,
            EntryPointClass::ReleaseCi,
            EntryPointClass::Test,
            EntryPointClass::Compatibility,
        ] {
            assert_eq!(
                select_runner(entry_point, gate),
                RunnerSelection::Official,
                "runtime routing must not vary by build class or cutover evidence"
            );
        }
    }
}
