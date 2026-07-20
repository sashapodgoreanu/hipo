//! T070 lifecycle telemetry contract for the official runner.
//!
//! The sidecar does not own a second event archive. Request samples are routed
//! to the normal run-history stream, while lifecycle/autoscale events remain
//! ephemeral. Stage correlation uses a fingerprint rather than raw user text.

use duckle_db_runner::events::{
    RunnerEvent, RunnerEventKind, TelemetryRetention, TelemetrySamplePhase,
};
use duckle_db_runner::model::{
    RunId, SanitizedMetrics, TransportKind, WorkerId, WorkerKind, WorkerLeaseId,
};

fn complete_metrics() -> SanitizedMetrics {
    SanitizedMetrics {
        memory_current_bytes: Some(64 * 1024 * 1024),
        memory_peak_bytes: Some(96 * 1024 * 1024),
        spill_current_bytes: Some(16 * 1024 * 1024),
        spill_peak_bytes: Some(32 * 1024 * 1024),
        cpu_ms: Some(275),
        rows: Some(4_200),
        transfer_bytes: Some(8 * 1024 * 1024),
        duration_ms: Some(10_000),
        transport_kind: Some(TransportKind::Quack),
    }
}

#[test]
fn request_samples_cover_all_permitted_metrics_and_route_only_to_run_history() {
    let run_id = RunId::new();
    let worker_id = WorkerId::new();
    let lease_id = WorkerLeaseId::new();
    let raw_stage = "stage-with-password=TOP_SECRET_CANARY";

    let start = RunnerEvent::request_telemetry(
        1_000,
        TelemetrySamplePhase::RequestStarted,
        run_id,
        worker_id,
        lease_id,
        WorkerKind::Warm,
        raw_stage,
        3,
        SanitizedMetrics::empty(),
    );
    let periodic = RunnerEvent::request_telemetry(
        6_000,
        TelemetrySamplePhase::Periodic,
        run_id,
        worker_id,
        lease_id,
        WorkerKind::Warm,
        raw_stage,
        3,
        complete_metrics(),
    );
    let finished = RunnerEvent::request_telemetry(
        11_000,
        TelemetrySamplePhase::RequestFinished,
        run_id,
        worker_id,
        lease_id,
        WorkerKind::Warm,
        raw_stage,
        3,
        complete_metrics(),
    );
    let control = RunnerEvent::lifecycle(
        11_001,
        RunnerEventKind::LeaseReleased,
        Some(run_id),
        Some(worker_id),
        Some(lease_id),
        Some(WorkerKind::Warm),
    );

    assert_eq!(periodic.at_millis - start.at_millis, 5_000);
    assert_eq!(finished.at_millis - periodic.at_millis, 5_000);
    assert_eq!(start.telemetry_phase, Some(TelemetrySamplePhase::RequestStarted));
    assert_eq!(periodic.telemetry_phase, Some(TelemetrySamplePhase::Periodic));
    assert_eq!(
        finished.telemetry_phase,
        Some(TelemetrySamplePhase::RequestFinished)
    );
    assert_eq!(finished.attempt, Some(3));
    assert_eq!(finished.worker_kind, Some(WorkerKind::Warm));
    assert_eq!(finished.retention, TelemetryRetention::RunHistory);
    assert_eq!(control.retention, TelemetryRetention::Ephemeral);

    let events = [&control, &start, &periodic, &finished];
    let retained = events
        .into_iter()
        .filter(|event| event.retained_for_history())
        .count();
    assert_eq!(retained, 3, "no separate lifecycle/autoscale archive is retained");

    let json = serde_json::to_value(&finished).unwrap();
    let object = json.as_object().unwrap();
    for key in [
        "runId",
        "workerId",
        "leaseId",
        "workerKind",
        "stageFingerprint",
        "attempt",
        "telemetryPhase",
        "retention",
        "metrics",
    ] {
        assert!(object.contains_key(key), "missing request telemetry field {key}");
    }
    let metrics = object["metrics"].as_object().unwrap();
    for key in [
        "memoryCurrentBytes",
        "memoryPeakBytes",
        "spillCurrentBytes",
        "spillPeakBytes",
        "cpuMs",
        "rows",
        "transferBytes",
        "durationMs",
        "transportKind",
    ] {
        assert!(metrics.contains_key(key), "missing sanitized metric {key}");
    }
    assert_eq!(metrics["transportKind"], "quack");

    let serialized = serde_json::to_string(&events).unwrap();
    assert!(!serialized.contains(raw_stage));
    for forbidden in [
        "TOP_SECRET_CANARY",
        "password=",
        "127.0.0.1",
        "quack:",
        "TOKEN",
        "capability",
        "sql",
        "pid",
    ] {
        assert!(
            !serialized.contains(forbidden),
            "telemetry leaked forbidden content {forbidden}"
        );
    }
}

#[test]
fn warm_and_on_demand_request_samples_remain_explicitly_distinct() {
    let run_id = RunId::new();
    let lease_id = WorkerLeaseId::new();
    let warm = RunnerEvent::request_telemetry(
        1,
        TelemetrySamplePhase::RequestStarted,
        run_id,
        WorkerId::new(),
        lease_id,
        WorkerKind::Warm,
        "stage-a",
        1,
        SanitizedMetrics::empty(),
    );
    let on_demand = RunnerEvent::request_telemetry(
        2,
        TelemetrySamplePhase::RequestStarted,
        RunId::new(),
        WorkerId::new(),
        WorkerLeaseId::new(),
        WorkerKind::OnDemand,
        "stage-b",
        1,
        SanitizedMetrics::empty(),
    );

    assert_eq!(warm.worker_kind, Some(WorkerKind::Warm));
    assert_eq!(on_demand.worker_kind, Some(WorkerKind::OnDemand));
    assert!(warm.retained_for_history());
    assert!(on_demand.retained_for_history());
}
