//! Explicit, serializable gate for enabling the official runner in production.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceStatus {
    Pass,
    Fail,
    NotApplicable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceItem {
    pub status: EvidenceStatus,
    /// A stable short identifier, not a log, command line or benchmark output.
    pub evidence_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingDisposition {
    Resolved,
    Accepted { motivation: String },
    Open,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CutoverEvidence {
    pub release_id: String,
    pub technical_owner: String,
    pub release_approver: String,
    pub success_criteria: BTreeMap<String, EvidenceItem>,
    pub findings: BTreeMap<String, FindingDisposition>,
    pub bundle_evidence_id: Option<String>,
}

impl CutoverEvidence {
    /// Only the criteria declared applicable to the migration gate are
    /// mandatory. The caller chooses the set so future intentionally-out-of-
    /// scope criteria can be marked NotApplicable without a hidden bypass.
    pub fn evaluate<'a, I>(&self, required_criteria: I) -> CutoverGate
    where
        I: IntoIterator<Item = &'a str>,
    {
        let mut missing_or_failed = Vec::new();
        if self.release_id.trim().is_empty() {
            missing_or_failed.push("release_id".into());
        }
        if self.technical_owner.trim().is_empty() {
            missing_or_failed.push("technical_owner".into());
        }
        if self.release_approver.trim().is_empty() {
            missing_or_failed.push("release_approver".into());
        }
        if self.bundle_evidence_id.as_deref().is_none_or(str::is_empty) {
            missing_or_failed.push("bundle".into());
        }
        for criterion in required_criteria {
            match self.success_criteria.get(criterion) {
                Some(EvidenceItem {
                    status: EvidenceStatus::Pass | EvidenceStatus::NotApplicable,
                    ..
                }) => {}
                _ => missing_or_failed.push(criterion.to_owned()),
            }
        }
        for (finding, disposition) in &self.findings {
            if matches!(disposition, FindingDisposition::Open) {
                missing_or_failed.push(format!("finding:{finding}"));
            }
        }

        if missing_or_failed.is_empty() {
            CutoverGate::Approved
        } else {
            CutoverGate::Rejected { missing_or_failed }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CutoverGate {
    Approved,
    Rejected { missing_or_failed: Vec<String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryPointClass {
    Production,
    ReleaseCi,
    Test,
    Compatibility,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnerSelection {
    Official,
    Compatibility,
}

/// The official runner is never selected by a production or release-CI entry
/// point until immutable evidence approves cutover. Test and compatibility
/// paths may exercise it before that gate.
pub fn select_runner(entry_point: EntryPointClass, gate: &CutoverGate) -> RunnerSelection {
    match (entry_point, gate) {
        (EntryPointClass::Production | EntryPointClass::ReleaseCi, CutoverGate::Approved) => {
            RunnerSelection::Official
        }
        (
            EntryPointClass::Production | EntryPointClass::ReleaseCi,
            CutoverGate::Rejected { .. },
        ) => RunnerSelection::Compatibility,
        (EntryPointClass::Test | EntryPointClass::Compatibility, _) => RunnerSelection::Official,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_stays_compatible_until_evidence_is_complete() {
        let evidence = CutoverEvidence {
            release_id: "r1".into(),
            technical_owner: "owner".into(),
            release_approver: "approver".into(),
            success_criteria: BTreeMap::new(),
            findings: BTreeMap::new(),
            bundle_evidence_id: None,
        };
        let gate = evidence.evaluate(["SC-001"]);
        assert!(matches!(gate, CutoverGate::Rejected { .. }));
        assert_eq!(
            select_runner(EntryPointClass::Production, &gate),
            RunnerSelection::Compatibility
        );
        assert_eq!(
            select_runner(EntryPointClass::Test, &gate),
            RunnerSelection::Official
        );
    }
}
