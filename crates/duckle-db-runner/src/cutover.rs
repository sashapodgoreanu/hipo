//! Cutover evidence retained for release documentation.
//!
//! Runtime routing is intentionally no longer controlled by build classes,
//! environment variables, or evidence manifests. Duckle has one database
//! execution route: the packaged Quack runner. Evidence remains serializable so
//! owners can record parity, benchmark, package, and approval results without
//! introducing a second executable path.

use crate::bundle::{bundle_for, BundlePlatform, QuackBundleEntry};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::OnceLock;

pub const CUTOVER_MANIFEST_SCHEMA_VERSION: u32 = 1;
pub const REQUIRED_CUTOVER_CRITERIA: [&str; 11] = [
    "SC-001", "SC-002", "SC-003", "SC-004", "SC-005", "SC-006", "SC-007", "SC-008",
    "SC-009", "SC-010", "SC-011",
];

const NON_WAIVABLE_CRITERIA: [&str; 5] = ["SC-001", "SC-003", "SC-004", "SC-005", "SC-011"];

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
pub struct BundleEvidence {
    pub platform: String,
    pub duckdb_version: String,
    pub quack_version: String,
    pub quack_sha256: String,
    pub license: String,
    pub provenance: String,
}

impl BundleEvidence {
    fn matches(&self, expected: &QuackBundleEntry) -> bool {
        self.platform == expected.platform.repository_name()
            && self.duckdb_version == expected.duckdb_version
            && self.quack_version == expected.quack_version
            && self.quack_sha256.eq_ignore_ascii_case(expected.sha256)
            && self.license == expected.license
            && self.provenance == expected.provenance
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CutoverEvidence {
    #[serde(default)]
    pub schema_version: u32,
    pub release_id: String,
    pub technical_owner: String,
    pub release_approver: String,
    pub success_criteria: BTreeMap<String, EvidenceItem>,
    pub findings: BTreeMap<String, FindingDisposition>,
    pub bundle_evidence_id: Option<String>,
    #[serde(default)]
    pub benchmark_evidence_id: Option<String>,
    #[serde(default)]
    pub bundle: Option<BundleEvidence>,
}

impl CutoverEvidence {
    pub fn evaluate_required(&self) -> CutoverGate {
        self.evaluate(REQUIRED_CUTOVER_CRITERIA)
    }

    pub fn evaluate<'a, I>(&self, required_criteria: I) -> CutoverGate
    where
        I: IntoIterator<Item = &'a str>,
    {
        let expected_bundle = BundlePlatform::current().and_then(bundle_for);
        self.evaluate_with_bundle(required_criteria, expected_bundle)
    }

    fn evaluate_with_bundle<'a, I>(
        &self,
        required_criteria: I,
        expected_bundle: Option<&QuackBundleEntry>,
    ) -> CutoverGate
    where
        I: IntoIterator<Item = &'a str>,
    {
        let mut missing_or_failed = Vec::new();

        if self.schema_version != CUTOVER_MANIFEST_SCHEMA_VERSION {
            push_unique(&mut missing_or_failed, "schema_version");
        }
        require_text(&mut missing_or_failed, "release_id", &self.release_id);
        require_text(
            &mut missing_or_failed,
            "technical_owner",
            &self.technical_owner,
        );
        require_text(
            &mut missing_or_failed,
            "release_approver",
            &self.release_approver,
        );
        require_optional_text(
            &mut missing_or_failed,
            "bundle_evidence",
            self.bundle_evidence_id.as_deref(),
        );
        require_optional_text(
            &mut missing_or_failed,
            "benchmark_evidence",
            self.benchmark_evidence_id.as_deref(),
        );

        for criterion in required_criteria {
            match self.success_criteria.get(criterion) {
                Some(item) if item.evidence_id.trim().is_empty() => {
                    push_unique(&mut missing_or_failed, &format!("{criterion}:evidence"));
                }
                Some(EvidenceItem {
                    status: EvidenceStatus::Pass,
                    ..
                }) => {}
                Some(EvidenceItem {
                    status: EvidenceStatus::NotApplicable,
                    ..
                }) if !NON_WAIVABLE_CRITERIA.contains(&criterion) => {}
                _ => push_unique(&mut missing_or_failed, criterion),
            }
        }

        for (finding, disposition) in &self.findings {
            match disposition {
                FindingDisposition::Resolved => {}
                FindingDisposition::Accepted { motivation } if !motivation.trim().is_empty() => {}
                FindingDisposition::Accepted { .. } => push_unique(
                    &mut missing_or_failed,
                    &format!("finding:{finding}:motivation"),
                ),
                FindingDisposition::Open => {
                    push_unique(&mut missing_or_failed, &format!("finding:{finding}"));
                }
            }
        }

        match (self.bundle.as_ref(), expected_bundle) {
            (Some(actual), Some(expected)) if actual.matches(expected) => {}
            (Some(_), Some(_)) | (None, Some(_)) => {
                push_unique(&mut missing_or_failed, "bundle_identity");
            }
            (_, None) => push_unique(&mut missing_or_failed, "bundle_target"),
        }

        missing_or_failed.sort();
        if missing_or_failed.is_empty() {
            CutoverGate::Approved
        } else {
            CutoverGate::Rejected { missing_or_failed }
        }
    }
}

fn require_text(missing: &mut Vec<String>, id: &str, value: &str) {
    if value.trim().is_empty() {
        push_unique(missing, id);
    }
}

fn require_optional_text(missing: &mut Vec<String>, id: &str, value: Option<&str>) {
    if value.is_none_or(|value| value.trim().is_empty()) {
        push_unique(missing, id);
    }
}

fn push_unique(values: &mut Vec<String>, value: &str) {
    if !values.iter().any(|existing| existing == value) {
        values.push(value.to_string());
    }
}

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

/// Transitional source-compatibility type. It no longer controls execution.
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

/// Transitional source-compatibility type. `Compatibility` is never selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnerSelection {
    Official,
    Compatibility,
}

/// Kept temporarily so existing callers compile while T071 removes the old
/// selector API. There is no environment- or build-class configuration anymore.
pub fn configured_entry_point_class() -> EntryPointClass {
    EntryPointClass::Production
}

/// Evidence remains available to release tooling and documentation, but its
/// result does not switch the runtime backend.
pub fn packaged_cutover_gate() -> CutoverGate {
    static GATE: OnceLock<CutoverGate> = OnceLock::new();
    GATE.get_or_init(|| evaluate_cutover_json(option_env!("DUCKLE_CUTOVER_EVIDENCE_JSON")))
        .clone()
}

pub fn evaluate_cutover_json(json: Option<&str>) -> CutoverGate {
    let Some(json) = json.filter(|value| !value.trim().is_empty()) else {
        return CutoverGate::Rejected {
            missing_or_failed: vec!["cutover_manifest".to_string()],
        };
    };

    match serde_json::from_str::<CutoverEvidence>(json) {
        Ok(evidence) => evidence.evaluate_required(),
        Err(_) => CutoverGate::Rejected {
            missing_or_failed: vec!["cutover_manifest_parse".to_string()],
        },
    }
}

/// Duckle now has one database runtime. Arguments are accepted only until all
/// callers are migrated away from the old selector API.
pub fn select_runner(
    _entry_point: EntryPointClass,
    _gate: &CutoverGate,
) -> RunnerSelection {
    RunnerSelection::Official
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approved_evidence() -> CutoverEvidence {
        let expected = bundle_for(BundlePlatform::current().unwrap()).unwrap();
        let success_criteria = REQUIRED_CUTOVER_CRITERIA
            .into_iter()
            .map(|criterion| {
                (
                    criterion.to_string(),
                    EvidenceItem {
                        status: EvidenceStatus::Pass,
                        evidence_id: format!("evidence-{criterion}"),
                    },
                )
            })
            .collect();

        CutoverEvidence {
            schema_version: CUTOVER_MANIFEST_SCHEMA_VERSION,
            release_id: "r1".into(),
            technical_owner: "owner".into(),
            release_approver: "approver".into(),
            success_criteria,
            findings: BTreeMap::from([("quality-1".into(), FindingDisposition::Resolved)]),
            bundle_evidence_id: Some("bundle-1".into()),
            benchmark_evidence_id: Some("benchmark-1".into()),
            bundle: Some(BundleEvidence {
                platform: expected.platform.repository_name().into(),
                duckdb_version: expected.duckdb_version.into(),
                quack_version: expected.quack_version.into(),
                quack_sha256: expected.sha256.into(),
                license: expected.license.into(),
                provenance: expected.provenance.into(),
            }),
        }
    }

    #[test]
    fn every_legacy_entry_point_maps_to_the_single_runner() {
        let rejected = evaluate_cutover_json(None);
        for entry_point in [
            EntryPointClass::Production,
            EntryPointClass::ReleaseCi,
            EntryPointClass::Test,
            EntryPointClass::Compatibility,
        ] {
            assert_eq!(
                select_runner(entry_point, &rejected),
                RunnerSelection::Official
            );
        }
    }

    #[test]
    fn complete_matching_evidence_is_still_recordable() {
        assert_eq!(approved_evidence().evaluate_required(), CutoverGate::Approved);
    }

    #[test]
    fn safety_criteria_cannot_be_not_applicable() {
        let mut evidence = approved_evidence();
        evidence.success_criteria.get_mut("SC-004").unwrap().status =
            EvidenceStatus::NotApplicable;
        assert!(evidence
            .evaluate_required()
            .rejection_ids()
            .iter()
            .any(|id| id == "SC-004"));
    }

    #[test]
    fn accepted_finding_requires_a_motivation() {
        let mut evidence = approved_evidence();
        evidence.findings.insert(
            "quality-1".into(),
            FindingDisposition::Accepted {
                motivation: " ".into(),
            },
        );
        assert!(evidence
            .evaluate_required()
            .rejection_ids()
            .iter()
            .any(|id| id == "finding:quality-1:motivation"));
    }
}
