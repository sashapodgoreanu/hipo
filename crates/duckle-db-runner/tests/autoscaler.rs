use duckle_db_runner::autoscaler::{ElasticAutoscaler, ScaleAction, AUTOSCALE_INTERVAL_MILLIS};
use duckle_db_runner::demand::{DemandObservation, DEMAND_WINDOW_MILLIS};
use duckle_db_runner::events::{ScaleDirection, ScaleReason, ScaleTelemetry};

fn observation(active_runs: u32, peak_5m: u32) -> DemandObservation {
    DemandObservation {
        active_runs,
        peak_5m,
        newly_counted: false,
    }
}

#[test]
fn policy_has_base_three_five_second_ticks_and_twenty_percent_ceiling() {
    let mut autoscaler = ElasticAutoscaler::new(3);
    let first = autoscaler
        .evaluate_if_due(0, observation(100, 100), 3)
        .unwrap();
    assert_eq!(first.target_warm_capacity, 120);
    assert_eq!(first.action, ScaleAction::ScaleOut { workers: 117 });
    assert!(autoscaler
        .evaluate_if_due(AUTOSCALE_INTERVAL_MILLIS - 1, observation(100, 100), 120)
        .is_none());
    let steady = autoscaler
        .evaluate_if_due(AUTOSCALE_INTERVAL_MILLIS, observation(100, 100), 120)
        .unwrap();
    assert_eq!(steady.action, ScaleAction::Hold);
}

#[test]
fn scale_in_only_becomes_eligible_after_the_five_minute_peak_expires() {
    let mut autoscaler = ElasticAutoscaler::new(3);
    autoscaler.evaluate_now(0, observation(100, 100), 3);
    let still_warm = autoscaler.evaluate_now(DEMAND_WINDOW_MILLIS, observation(0, 100), 120);
    assert_eq!(still_warm.action, ScaleAction::Hold);
    let expired = autoscaler.evaluate_now(DEMAND_WINDOW_MILLIS + 1, observation(0, 0), 120);
    assert_eq!(expired.target_warm_capacity, 3);
    assert_eq!(expired.action, ScaleAction::ScaleIn { workers: 117 });
}

#[test]
fn ceiling_arithmetic_is_exact_for_small_and_large_peaks() {
    let autoscaler = ElasticAutoscaler::new(3);
    assert_eq!(autoscaler.target_for_peak(0), 3);
    assert_eq!(autoscaler.target_for_peak(1), 3);
    assert_eq!(autoscaler.target_for_peak(2), 3);
    assert_eq!(autoscaler.target_for_peak(3), 4);
    assert_eq!(autoscaler.target_for_peak(4), 5);
    assert_eq!(autoscaler.target_for_peak(99), 119);
    assert_eq!(autoscaler.target_for_peak(100), 120);
}

#[test]
fn restart_discards_the_previous_peak_and_returns_to_base_capacity() {
    let mut before_restart = ElasticAutoscaler::new(3);
    let burst = before_restart.evaluate_now(0, observation(100, 100), 3);
    assert_eq!(burst.target_warm_capacity, 120);

    let mut after_restart = ElasticAutoscaler::new(3);
    let restarted = after_restart
        .evaluate_if_due(0, observation(0, 0), 3)
        .unwrap();
    assert_eq!(restarted.target_warm_capacity, 3);
    assert_eq!(restarted.action, ScaleAction::Hold);
}

#[test]
fn changing_base_capacity_takes_effect_on_the_next_forced_decision() {
    let mut autoscaler = ElasticAutoscaler::new(3);
    autoscaler.evaluate_now(0, observation(0, 0), 3);
    autoscaler.set_base_capacity(7);

    let grown = autoscaler.evaluate_now(1, observation(0, 0), 3);
    assert_eq!(grown.base_capacity, 7);
    assert_eq!(grown.target_warm_capacity, 7);
    assert_eq!(grown.action, ScaleAction::ScaleOut { workers: 4 });
}

#[test]
fn autoscale_telemetry_has_only_safe_capacity_demand_peak_reason_and_outcome_fields() {
    let telemetry = ScaleTelemetry {
        reason: ScaleReason::PeriodicTick,
        direction: ScaleDirection::ScaleOut,
        outcome: ScaleDirection::ScaleOut,
        active_demand: 100,
        peak_5m: 100,
        base_capacity: 3,
        current_warm_capacity: 3,
        starting_warm: 117,
        ready_warm: 0,
        leased_warm: 3,
        target_warm_capacity: 120,
        provisioned: 117,
        terminated_ready: 0,
    };

    let json = serde_json::to_value(telemetry).unwrap();
    let object = json.as_object().unwrap();
    for key in [
        "reason",
        "outcome",
        "activeDemand",
        "peak5m",
        "baseCapacity",
        "currentWarmCapacity",
        "targetWarmCapacity",
    ] {
        assert!(object.contains_key(key), "missing telemetry field {key}");
    }
    for forbidden in ["endpoint", "port", "pid", "path", "token", "secret", "sql", "capability"] {
        assert!(!object.contains_key(forbidden), "telemetry leaked {forbidden}");
    }
}
