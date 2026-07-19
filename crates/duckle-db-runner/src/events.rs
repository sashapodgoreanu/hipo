//! Sanitized lifecycle telemetry for the runner control plane.
//!
//! The structures deliberately have no free-form detail field. That makes it
//! mechanically harder for a provider to leak an endpoint, PID, path, SQL or
//! capability into a desktop event, history record, or headless response.

use crate::model::{
    EventId, RunId, RunnerFailureReason, SanitizedMetrics, WorkerId, WorkerKind, WorkerLeaseId,
};
use serde::{Deserialize, Serialize};

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
            evidence_id: None,
        }
    }

    pub fn failure(mut self, reason: RunnerFailureReason) -> Self {
        self.reason = Some(reason);
        self
    }
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
