//! Sanitized lifecycle telemetry for the runner control plane.
//!
//! The structures deliberately have no free-form detail field. That makes it
//! mechanically harder for a provider to leak an endpoint, PID, path, SQL or
//! capability into a desktop event, history record, or headless response.

use crate::model::{
    EventId, RunId, RunnerFailureReason, SanitizedMetrics, WorkerId, WorkerKind, WorkerLeaseId,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunnerEventKind {
    AcquireRequested,
    AllocationDecided,
    ProvisionStarted,
    WorkerReady,
    LeaseGranted,
    LeaseReleased,
    WorkerFailed,
    ScaleEvaluated,
    ProfileApplyStarted,
    ProfileApplied,
    CleanupCompleted,
    CutoverGateRejected,
    RequestTelemetry,
}

/// Point in the request lifecycle represented by one resource sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetrySamplePhase {
    RequestStarted,
    Periodic,
    RequestFinished,
}

/// Retention destination for an event. Request samples join the normal run
/// history; control-plane lifecycle and autoscaling events remain ephemeral.
/// There is intentionally no separate runner telemetry archive.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetryRetention {
    #[default]
    Ephemeral,
    RunHistory,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunnerEvent {
    pub event_id: EventId,
    pub at_millis: u64,
    pub kind: RunnerEventKind,
    pub run_id: Option<RunId>,
    pub worker_id: Option<WorkerId>,
    pub lease_id: Option<WorkerLeaseId>,
    pub worker_kind: Option<WorkerKind>,
    pub reason: Option<RunnerFailureReason>,
    pub metrics: SanitizedMetrics,
    pub scale: Option<ScaleTelemetry>,
    /// A deterministic fingerprint of the planner-owned stage identifier. The
    /// raw stage string is not copied into provider telemetry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telemetry_phase: Option<TelemetrySamplePhase>,
    #[serde(default)]
    pub retention: TelemetryRetention,
    /// Opaque identifier of an evidence item, never its raw benchmark or log.
    pub evidence_id: Option<String>,
}

impl RunnerEvent {
    pub fn lifecycle(
        at_millis: u64,
        kind: RunnerEventKind,
        run_id: Option<RunId>,
        worker_id: Option<WorkerId>,
        lease_id: Option<WorkerLeaseId>,
        worker_kind: Option<WorkerKind>,
    ) -> Self {
        Self {
            event_id: EventId::new(),
            at_millis,
            kind,
            run_id,
            worker_id,
            lease_id,
            worker_kind,
            reason: None,
            metrics: SanitizedMetrics::empty(),
            scale: None,
            stage_fingerprint: None,
            attempt: None,
            telemetry_phase: None,
            retention: TelemetryRetention::Ephemeral,
            evidence_id: None,
        }
    }

    /// Build a stage/request sample suitable for the normal run-history route.
    /// The stage identifier is fingerprinted so telemetry can correlate samples
    /// without retaining a user-supplied identifier or SQL fragment.
    #[allow(clippy::too_many_arguments)]
    pub fn request_telemetry(
        at_millis: u64,
        phase: TelemetrySamplePhase,
        run_id: RunId,
        worker_id: WorkerId,
        lease_id: WorkerLeaseId,
        worker_kind: WorkerKind,
        stage_id: &str,
        attempt: u32,
        metrics: SanitizedMetrics,
    ) -> Self {
        Self {
            event_id: EventId::new(),
            at_millis,
            kind: RunnerEventKind::RequestTelemetry,
            run_id: Some(run_id),
            worker_id: Some(worker_id),
            lease_id: Some(lease_id),
            worker_kind: Some(worker_kind),
            reason: None,
            metrics,
            scale: None,
            stage_fingerprint: Some(stage_fingerprint(stage_id)),
            attempt: Some(attempt),
            telemetry_phase: Some(phase),
            retention: TelemetryRetention::RunHistory,
            evidence_id: None,
        }
    }

    pub fn attempt(mut self, attempt: u32) -> Self {
        self.attempt = Some(attempt);
        self
    }

    pub fn failure(mut self, reason: RunnerFailureReason) -> Self {
        self.reason = Some(reason);
        self
    }

    pub fn retained_for_history(&self) -> bool {
        self.retention == TelemetryRetention::RunHistory
    }
}

fn stage_fingerprint(stage_id: &str) -> String {
    format!("{:x}", Sha256::digest(stage_id.as_bytes()))
}

/// The complete allowed autoscaler telemetry set. All fields are counts or
/// durations; resource locations and provider implementation details remain
/// private to the provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScaleTelemetry {
    pub reason: ScaleReason,
    /// `direction` is retained for callers that already consume the initial
    /// control-plane DTO. `outcome` makes the outcome explicit in the
    /// versioned telemetry contract.
    pub direction: ScaleDirection,
    pub outcome: ScaleDirection,
    pub active_demand: u32,
    pub peak_5m: u32,
    pub base_capacity: u32,
    pub current_warm_capacity: u32,
    pub starting_warm: u32,
    pub ready_warm: u32,
    pub leased_warm: u32,
    pub target_warm_capacity: u32,
    pub provisioned: u32,
    pub terminated_ready: u32,
}

/// Explicit outcome of an autoscaler evaluation. This is intentionally
/// independent from any provider-specific process detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScaleDirection {
    Hold,
    ScaleOut,
    ScaleIn,
}

/// Safe cause of a controller evaluation. It records only orchestration
/// state, never provider diagnostics or process details.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScaleReason {
    Startup,
    PeriodicTick,
    ProfileChanged,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TransportKind;

    #[test]
    fn serialized_events_have_only_the_allowlisted_shape() {
        let event = RunnerEvent::lifecycle(
            5_000,
            RunnerEventKind::ScaleEvaluated,
            None,
            None,
            None,
            None,
        );
        let json = serde_json::to_value(&event).unwrap();
        assert_no_forbidden_key(&json);
    }

    #[test]
    fn request_telemetry_fingerprints_stage_and_routes_to_run_history() {
        let stage = "customer-secret-stage";
        let mut metrics = SanitizedMetrics::empty();
        metrics.observe_resource_sample(10, 20);
        metrics.cpu_ms = Some(30);
        metrics.rows = Some(40);
        metrics.transfer_bytes = Some(50);
        metrics.duration_ms = Some(60);
        metrics.transport_kind = Some(TransportKind::Quack);
        let event = RunnerEvent::request_telemetry(
            5_000,
            TelemetrySamplePhase::RequestFinished,
            RunId::new(),
            WorkerId::new(),
            WorkerLeaseId::new(),
            WorkerKind::OnDemand,
            stage,
            2,
            metrics,
        );

        assert!(event.retained_for_history());
        assert_eq!(event.attempt, Some(2));
        assert_eq!(event.worker_kind, Some(WorkerKind::OnDemand));
        assert_ne!(event.stage_fingerprint.as_deref(), Some(stage));
        let serialized = serde_json::to_string(&event).unwrap();
        assert!(!serialized.contains(stage));
        assert_no_forbidden_key(&serde_json::from_str(&serialized).unwrap());
    }

    fn assert_no_forbidden_key(value: &serde_json::Value) {
        const FORBIDDEN: [&str; 8] = [
            "endpoint",
            "port",
            "pid",
            "path",
            "token",
            "secret",
            "sql",
            "capability",
        ];
        match value {
            serde_json::Value::Object(object) => {
                for (key, nested) in object {
                    assert!(
                        !FORBIDDEN.contains(&key.as_str()),
                        "event leaked forbidden field {key}"
                    );
                    assert_no_forbidden_key(nested);
                }
            }
            serde_json::Value::Array(values) => {
                for nested in values {
                    assert_no_forbidden_key(nested);
                }
            }
            _ => {}
        }
    }
}
