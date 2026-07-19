use duckle_db_runner::bootstrap::BootstrapError;
use duckle_db_runner::events::{RunnerEvent, RunnerEventKind};
use duckle_db_runner::model::{
    RunId, RunnerFailureReason, SanitizedMetrics, TransportKind, WorkerId, WorkerKind, WorkerLease,
    WorkerLeaseId,
};
use serde_json::Value;
use std::io;

const FORBIDDEN_KEYS: [&str; 8] = [
    "endpoint",
    "port",
    "pid",
    "path",
    "token",
    "secret",
    "sql",
    "capability",
];

fn assert_allowlisted_shape(value: &Value) {
    match value {
        Value::Object(object) => {
            for (key, nested) in object {
                assert!(
                    !FORBIDDEN_KEYS.contains(&key.as_str()),
                    "public runner payload leaked forbidden key {key}: {value}"
                );
                assert_allowlisted_shape(nested);
            }
        }
        Value::Array(values) => {
            for nested in values {
                assert_allowlisted_shape(nested);
            }
        }
        _ => {}
    }
}

#[test]
fn public_runner_events_and_leases_expose_only_opaque_allowlisted_data() {
    let run_id = RunId::new();
    let worker_id = WorkerId::new();
    let lease_id = WorkerLeaseId::new();

    let mut event = RunnerEvent::lifecycle(
        5_000,
        RunnerEventKind::WorkerFailed,
        Some(run_id),
        Some(worker_id),
        Some(lease_id),
        Some(WorkerKind::OnDemand),
    )
    .failure(RunnerFailureReason::RunnerCrashed);
    event.metrics = SanitizedMetrics {
        memory_current_bytes: Some(1024),
        memory_peak_bytes: Some(2048),
        spill_current_bytes: Some(4096),
        spill_peak_bytes: Some(8192),
        cpu_ms: Some(17),
        rows: Some(3),
        transfer_bytes: Some(128),
        duration_ms: Some(25),
        transport_kind: Some(TransportKind::Quack),
    };
    event.evidence_id = Some("SC-001".to_string());

    let lease = WorkerLease {
        lease_id,
        worker_id,
        run_id,
        worker_kind: WorkerKind::OnDemand,
        profile_version: 7,
    };

    for value in [
        serde_json::to_value(event).expect("runner event serializes"),
        serde_json::to_value(lease).expect("worker lease serializes"),
    ] {
        assert_allowlisted_shape(&value);
        let serialized = value.to_string();
        for forbidden_value in [
            "127.0.0.1",
            "localhost",
            "quack:",
            "Bearer ",
            "DUCKLE_TOKEN",
            "TOP_SECRET_CANARY",
        ] {
            assert!(
                !serialized.contains(forbidden_value),
                "public runner payload leaked {forbidden_value}: {serialized}"
            );
        }
    }
}

#[test]
fn public_bootstrap_failures_do_not_echo_provider_canaries() {
    let canary = "TOP_SECRET_CANARY";
    let error = BootstrapError::Io(io::Error::other(format!(
        "provider endpoint quack:127.0.0.1:9494 token={canary}"
    )));

    let public = error.to_string();
    assert_eq!(public, "runner bootstrap transport failed");
    assert!(!public.contains(canary));
    assert!(!public.contains("127.0.0.1"));
    assert!(!public.contains("quack:"));
}
