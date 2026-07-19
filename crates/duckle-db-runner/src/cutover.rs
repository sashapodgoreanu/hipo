//! Explicit, serializable gate for enabling the official runner in production.
//!
//! Production binaries accept only evidence embedded at compile time through
//! `DUCKLE_CUTOVER_EVIDENCE_JSON`. A runtime environment variable or workspace
//! file cannot silently enable the official route after a release is built.

use crate::bundle::{bundle_for, BundlePlatform, QuackBundleEntry};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::OnceLock;

pub const CUTOVER_MANIFEST_SCHEMA_VERSION: u32 = 1;
pub const REQUIRED_CUTOVER_CRITERIA: [&str; 11] = [
    "SC-001", "SC-002", "SC-003", "SC-004", "SC-005", "SC-006", "SC-007", "SC-008",
    "SC-009", "SC-010", "SC-011",
];

/// Criteria for which `not_applicable` would amount to a safety, compatibility,
/// containment, redaction, or offline-package waiver. Those criteria must pass.
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
    /// Evaluate this manifest for the current packaged platform against the
    /// mandatory Feature 003 cutover criteria.
    pub fn evaluate_required(&self) -> CutoverGate {
        self.evaluate(REQUIRED_CUTOVER_CRITERIA)
    }

    /// Only the criteria declared applicable to the migration gate are
    /// mandatory. The caller chooses the set so future intentionally-out-of-
    /// scope criteria can be marked NotApplicable without a hidden bypass.
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
                FindingDisposition::Accepted { .. } => {
                    push_unique(&mut missing_or_failed, &format!("finding:{finding}:motivation"));
                }
                FindingDisposition::Open => {
                    push_unique(&mut missing_or_failed, &format!("finding:{finding}"));
                }
            }
        }

        match (self.bundle.as_ref(), expected_bundle) {
            (Some(actual), Some(expected)) if actual.matches(expected) => {}
            (Some(_), Some(_)) => push_unique(&mut missing_or_failed, "bundle_identity"),
            (None, Some(_)) => push_unique(&mut missing_or_failed, "bundle_identity"),
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
    Compatibility,
}

/// Entry-point class selected while compiling the binary. Unknown values fail
/// closed to Production. Runtime environment changes cannot reclassify a built
/// release as Test or Compatibility.
pub fn configured_entry_point_class() -> EntryPointClass {
    option_env!("DUCKLE_ENTRY_POINT_CLASS")
        .and_then(EntryPointClass::parse)
        .unwrap_or(EntryPointClass::Production)
}

/// Parse and evaluate the immutable manifest compiled into this binary. A
/// missing or malformed manifest produces stable diagnostic IDs only.
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

/// The packaged gate is authoritative for production and release-CI builds.
/// Before approval they stay on compatibility. After approval, an older caller
/// cannot silently force the product back to CLI by passing a stale local
/// rejection. Test and explicit compatibility entry points may exercise the
/// official runner before the production gate.
pub fn select_runner(entry_point: EntryPointClass, gate: &CutoverGate) -> RunnerSelection {
    match entry_point {
        EntryPointClass::Test | EntryPointClass::Compatibility => RunnerSelection::Official,
        EntryPointClass::Production | EntryPointClass::ReleaseCi => {
            let approved = matches!(gate, CutoverGate::Approved)
                || matches!(packaged_cutover_gate(), CutoverGate::Approved);
            if approved {
                RunnerSelection::Official
            } else {
                RunnerSelection::Compatibility
            }
        }
    }
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
    fn production_stays_compatible_without_packaged_evidence() {
        let gate = evaluate_cutover_json(None);
        assert!(matches!(gate, CutoverGate::Rejected { .. }));
        assert_eq!(
            select_runner(EntryPointClass::Production, &gate),
            RunnerSelection::Compatibility
        );
        assert_eq!(
            select_runner(EntryPointClass::ReleaseCi, &gate),
            RunnerSelection::Compatibility
        );
        assert_eq!(
            select_runner(EntryPointClass::Test, &gate),
            RunnerSelection::Official
        );
    }

    #[test]
    fn complete_matching_evidence_approves_production() {
        let gate = approved_evidence().evaluate_required();
        assert_eq!(gate, CutoverGate::Approved);
        assert_eq!(
            select_runner(EntryPointClass::Production, &gate),
            RunnerSelection::Official
        );
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

    #[test]
    fn entry_point_parser_fails_closed_for_unknown_values() {
        assert_eq!(EntryPointClass::parse("release-ci"), Some(EntryPointClass::ReleaseCi));
        assert_eq!(EntryPointClass::parse("compatibility"), Some(EntryPointClass::Compatibility));
        assert_eq!(EntryPointClass::parse("unknown"), None);
    }
}
