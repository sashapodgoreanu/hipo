//! Source-compatible adapter for the former dual-backend migration.
//!
//! Duckle now has one database runtime: the packaged Quack runner. These small
//! types remain temporarily so existing callers can compile while their method
//! names are cleaned up; no environment variable, manifest, entry-point class,
//! or approval state can change the selected runtime.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CutoverGate {
    Approved,
    Rejected { missing_or_failed: Vec<String> },
}

impl CutoverGate {
    pub fn rejection_ids(&self) -> &[String] {
        match self {
            Self::Approved => &[],
            Self::Rejected { missing_or_failed } => missing_or_failed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryPointClass {
    Production,
    ReleaseCi,
    Test,
    Compatibility,
}

impl EntryPointClass {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "production" => Some(Self::Production),
            "release-ci" | "release_ci" => Some(Self::ReleaseCi),
            "test" => Some(Self::Test),
            "compatibility" => Some(Self::Compatibility),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnerSelection {
    Official,
    /// Retained only so older exhaustive matches compile. `select_runner` never
    /// returns this value and no runtime configuration can request it.
    Compatibility,
}

/// Retained for source compatibility. Build and runtime environment variables
/// are deliberately ignored.
pub fn configured_entry_point_class() -> EntryPointClass {
    EntryPointClass::Production
}

/// Retained for source compatibility. The packaged runtime is always active.
pub fn packaged_cutover_gate() -> CutoverGate {
    CutoverGate::Approved
}

/// Retained for tests and older callers. JSON cannot enable or disable a route.
pub fn evaluate_cutover_json(_json: Option<&str>) -> CutoverGate {
    CutoverGate::Approved
}

/// There is no selector anymore: every caller receives the Quack runtime.
pub fn select_runner(
    _entry_point: EntryPointClass,
    _gate: &CutoverGate,
) -> RunnerSelection {
    RunnerSelection::Official
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_legacy_entry_point_maps_to_the_single_runner() {
        let rejected = CutoverGate::Rejected {
            missing_or_failed: vec!["historical".into()],
        };
        for entry_point in [
            EntryPointClass::Production,
            EntryPointClass::ReleaseCi,
            EntryPointClass::Test,
            EntryPointClass::Compatibility,
        ] {
            assert_eq!(select_runner(entry_point, &rejected), RunnerSelection::Official);
        }
    }

    #[test]
    fn runtime_configuration_cannot_reject_the_packaged_runner() {
        assert_eq!(configured_entry_point_class(), EntryPointClass::Production);
        assert_eq!(packaged_cutover_gate(), CutoverGate::Approved);
        assert_eq!(evaluate_cutover_json(None), CutoverGate::Approved);
        assert_eq!(evaluate_cutover_json(Some("not-json")), CutoverGate::Approved);
    }
}
