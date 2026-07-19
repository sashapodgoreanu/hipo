//! T066 integration coverage for the versioned runner transport decision table.
//!
//! These tests are deliberately independent from a local DuckDB installation:
//! they verify the planner-visible policy contract that chooses SQL remote,
//! direct Quack transfer, or a reusable Parquet snapshot.

use duckle_duckdb_engine::{
    select_transport, RuntimeTransportCapabilities, TransportCleanupStrategy, TransportDecision,
    TransportDecisionError, TransportDecisionInput, TransportDecisionReason, TransportMechanism,
    TransportResourceLimits, TransportRetryStrategy, TRANSPORT_DECISION_POLICY_VERSION,
};

const MIB: u64 = 1024 * 1024;
const PARQUET_REUSE_THRESHOLD: u64 = 64 * MIB;
const STREAMING_MEMORY_CAP: u64 = 16 * MIB;

fn input(
    estimated_bytes: u64,
    consumer_count: u32,
    retry_required: bool,
    runtime: RuntimeTransportCapabilities,
) -> TransportDecisionInput {
    TransportDecisionInput {
        estimated_bytes,
        consumer_count,
        retry_required,
        relation_mutable: false,
        sidecar_available: true,
        materialization_cost_bytes: estimated_bytes / 4,
        runtime,
    }
}

fn all_capabilities() -> RuntimeTransportCapabilities {
    RuntimeTransportCapabilities {
        sql_remote: true,
        quack: true,
        parquet: true,
    }
}

fn assert_decision(
    actual: TransportDecision,
    mechanism: TransportMechanism,
    reason: TransportDecisionReason,
    retry: TransportRetryStrategy,
    cleanup: TransportCleanupStrategy,
    limits: TransportResourceLimits,
) {
    assert_eq!(actual.policy_version, TRANSPORT_DECISION_POLICY_VERSION);
    assert_eq!(actual.mechanism, mechanism);
    assert_eq!(actual.reason, reason);
    assert_eq!(actual.retry, retry);
    assert_eq!(actual.cleanup, cleanup);
    assert_eq!(actual.limits, limits);
}

#[test]
fn immutable_single_consumer_prefers_sql_remote_without_an_artifact() {
    let decision = select_transport(input(8 * MIB, 1, false, all_capabilities())).unwrap();

    assert_decision(
        decision,
        TransportMechanism::SqlRemote,
        TransportDecisionReason::SingleConsumerRemoteSql,
        TransportRetryStrategy::OrchestratorRetry,
        TransportCleanupStrategy::NoArtifact,
        TransportResourceLimits {
            maximum_memory_bytes: 8 * MIB,
            maximum_spill_bytes: 0,
        },
    );
}

#[test]
fn mutable_relation_cannot_use_sql_remote_and_uses_direct_quack() {
    let decision = select_transport(TransportDecisionInput {
        relation_mutable: true,
        ..input(32 * MIB, 1, false, all_capabilities())
    })
    .unwrap();

    assert_decision(
        decision,
        TransportMechanism::Quack,
        TransportDecisionReason::DirectQuackTransfer,
        TransportRetryStrategy::ReissueTransfer,
        TransportCleanupStrategy::DropEphemeralTransfer,
        TransportResourceLimits {
            maximum_memory_bytes: STREAMING_MEMORY_CAP,
            maximum_spill_bytes: 0,
        },
    );
}

#[test]
fn reusable_parquet_wins_at_the_exact_threshold_for_fanout_and_retry() {
    let decision = select_transport(TransportDecisionInput {
        materialization_cost_bytes: PARQUET_REUSE_THRESHOLD,
        ..input(PARQUET_REUSE_THRESHOLD, 2, true, all_capabilities())
    })
    .unwrap();

    assert_decision(
        decision,
        TransportMechanism::Parquet,
        TransportDecisionReason::ReusableParquetSnapshot,
        TransportRetryStrategy::ReuseSnapshot,
        TransportCleanupStrategy::RemoveSnapshotAfterConsumers,
        TransportResourceLimits {
            maximum_memory_bytes: STREAMING_MEMORY_CAP,
            maximum_spill_bytes: PARQUET_REUSE_THRESHOLD,
        },
    );
}

#[test]
fn parquet_is_not_selected_below_the_reuse_threshold() {
    let estimated = PARQUET_REUSE_THRESHOLD - 1;
    let decision = select_transport(input(estimated, 2, true, all_capabilities())).unwrap();

    assert_decision(
        decision,
        TransportMechanism::Quack,
        TransportDecisionReason::RuntimeCapabilityFallback,
        TransportRetryStrategy::ReissueTransfer,
        TransportCleanupStrategy::DropEphemeralTransfer,
        TransportResourceLimits {
            maximum_memory_bytes: STREAMING_MEMORY_CAP,
            maximum_spill_bytes: 0,
        },
    );
}

#[test]
fn expensive_parquet_materialization_falls_back_to_quack() {
    let decision = select_transport(TransportDecisionInput {
        materialization_cost_bytes: PARQUET_REUSE_THRESHOLD + 1,
        ..input(PARQUET_REUSE_THRESHOLD, 4, false, all_capabilities())
    })
    .unwrap();

    assert_eq!(decision.mechanism, TransportMechanism::Quack);
    assert_eq!(
        decision.reason,
        TransportDecisionReason::RuntimeCapabilityFallback
    );
    assert_eq!(decision.limits.maximum_spill_bytes, 0);
}

#[test]
fn parquet_only_runtime_uses_a_capability_fallback_snapshot() {
    let estimated = 4 * MIB;
    let decision = select_transport(input(
        estimated,
        1,
        false,
        RuntimeTransportCapabilities {
            sql_remote: false,
            quack: false,
            parquet: true,
        },
    ))
    .unwrap();

    assert_decision(
        decision,
        TransportMechanism::Parquet,
        TransportDecisionReason::RuntimeCapabilityFallback,
        TransportRetryStrategy::ReuseSnapshot,
        TransportCleanupStrategy::RemoveSnapshotAfterConsumers,
        TransportResourceLimits {
            maximum_memory_bytes: estimated,
            maximum_spill_bytes: estimated,
        },
    );
}

#[test]
fn missing_sidecar_fails_before_runtime_capability_selection() {
    let result = select_transport(TransportDecisionInput {
        sidecar_available: false,
        ..input(8 * MIB, 1, false, all_capabilities())
    });

    assert_eq!(result, Err(TransportDecisionError::RunnerUnavailable));
}

#[test]
fn runtime_without_any_supported_transport_fails_closed() {
    let result = select_transport(input(
        8 * MIB,
        1,
        false,
        RuntimeTransportCapabilities {
            sql_remote: false,
            quack: false,
            parquet: false,
        },
    ));

    assert_eq!(result, Err(TransportDecisionError::RuntimeUnsupported));
}

#[test]
fn identical_inputs_produce_identical_versioned_decisions() {
    let request = input(PARQUET_REUSE_THRESHOLD, 3, true, all_capabilities());
    let first = select_transport(request).unwrap();
    let second = select_transport(request).unwrap();

    assert_eq!(first, second);
    assert_eq!(first.policy_version, TRANSPORT_DECISION_POLICY_VERSION);
}
