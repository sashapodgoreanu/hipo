use duckle_db_runner::model::{
    RunCancellation, RunnerFailureReason, SanitizedMetrics,
};
use duckle_db_runner::resources::{
    AutomaticOrU16, ResourceLimit, RunnerResourcesProfile,
};
use duckle_db_runner::run_session::{ProfileApplier, RunSession, RunSessionError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Default)]
struct RecordingApplier {
    applied: Mutex<Vec<RunnerResourcesProfile>>,
    verified: Mutex<Vec<RunnerResourcesProfile>>,
    fail_apply: Mutex<Option<(u64, RunnerFailureReason)>>,
    fail_verify: Mutex<Option<(u64, RunnerFailureReason)>>,
}

impl ProfileApplier for RecordingApplier {
    fn apply_profile(&self, profile: &RunnerResourcesProfile) -> Result<(), RunnerFailureReason> {
        if let Some((version, reason)) = *self.fail_apply.lock().unwrap() {
            if version == profile.version {
                return Err(reason);
            }
        }
        self.applied.lock().unwrap().push(profile.clone());
        Ok(())
    }

    fn verify_profile(&self, profile: &RunnerResourcesProfile) -> Result<(), RunnerFailureReason> {
        self.verified.lock().unwrap().push(profile.clone());
        if let Some((version, reason)) = *self.fail_verify.lock().unwrap() {
            if version == profile.version {
                return Err(reason);
            }
        }
        Ok(())
    }
}

fn profile(version: u64, parallelism: u16) -> RunnerResourcesProfile {
    RunnerResourcesProfile {
        version,
        quack_parallelism: AutomaticOrU16::Value(parallelism),
        ..RunnerResourcesProfile::default()
    }
}

#[test]
fn active_queries_drain_before_the_latest_saved_profile_applies() {
    let applier = Arc::new(RecordingApplier::default());
    let session = RunSession::new(applier.clone(), profile(1, 2)).unwrap();
    let cancellation = RunCancellation::default();
    let first = session.begin_query(&cancellation).unwrap();
    let second = session.begin_query(&cancellation).unwrap();

    session.save_profile(profile(2, 4)).unwrap();
    session.save_profile(profile(3, 8)).unwrap();
    assert_eq!(session.profile_state().effective_version, 1);

    session.finish_query(first);
    assert_eq!(session.profile_state().effective_version, 1);
    session.finish_query(second);
    assert_eq!(session.profile_state().effective_version, 3);
    assert_eq!(
        applier
            .applied
            .lock()
            .unwrap()
            .iter()
            .map(|profile| profile.version)
            .collect::<Vec<_>>(),
        vec![3]
    );
}

#[test]
fn failed_apply_keeps_the_effective_profile_and_blocks_new_queries() {
    let applier = Arc::new(RecordingApplier::default());
    let session = RunSession::new(applier.clone(), profile(1, 1)).unwrap();
    *applier.fail_apply.lock().unwrap() = Some((
        2,
        RunnerFailureReason::ConfigurationApplyFailed,
    ));

    session.save_profile(profile(2, 2)).unwrap();
    assert_eq!(session.profile_state().effective_version, 1);
    assert_eq!(
        session.begin_query(&RunCancellation::default()),
        Err(RunSessionError::ConfigurationApplyFailed)
    );
}

#[test]
fn parallelism_permits_block_at_one_and_allow_eight() {
    let session = Arc::new(
        RunSession::new(Arc::new(RecordingApplier::default()), profile(1, 1)).unwrap(),
    );
    let first = session.begin_query(&RunCancellation::default()).unwrap();
    let waiting_session = session.clone();
    let (sender, receiver) = std::sync::mpsc::channel();
    thread::spawn(move || {
        sender
            .send(waiting_session.begin_query(&RunCancellation::default()))
            .unwrap();
    });
    assert!(receiver.recv_timeout(Duration::from_millis(50)).is_err());
    session.finish_query(first);
    let second = receiver.recv_timeout(Duration::from_secs(1)).unwrap().unwrap();
    session.finish_query(second);

    let eight = RunSession::new(
        Arc::new(RecordingApplier::default()),
        profile(1, 8),
    )
    .unwrap();
    let permits = (0..8)
        .map(|_| eight.begin_query(&RunCancellation::default()).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(eight.profile_state().active_queries, 8);
    for permit in permits {
        eight.finish_query(permit);
    }
}

#[test]
fn bounded_spill_is_applied_and_verified_as_one_complete_generation() {
    let applier = Arc::new(RecordingApplier::default());
    let session = RunSession::new(applier.clone(), profile(1, 2)).unwrap();
    let bounded = RunnerResourcesProfile {
        version: 2,
        memory: ResourceLimit::Bytes(256 * 1024 * 1024),
        cpu_threads: AutomaticOrU16::Value(2),
        spill: ResourceLimit::Bytes(64 * 1024 * 1024),
        quack_parallelism: AutomaticOrU16::Value(2),
        base_capacity: 3,
    };

    session.save_profile(bounded.clone()).unwrap();

    assert_eq!(session.profile_state().effective_version, 2);
    assert_eq!(applier.applied.lock().unwrap().as_slice(), &[bounded.clone()]);
    assert_eq!(
        applier.verified.lock().unwrap().last(),
        Some(&bounded),
        "spill, memory, CPU, and parallelism must be verified together"
    );
}

#[test]
fn disk_full_or_quota_failure_preserves_the_prior_effective_profile() {
    let applier = Arc::new(RecordingApplier::default());
    let session = RunSession::new(applier.clone(), profile(1, 2)).unwrap();
    *applier.fail_apply.lock().unwrap() = Some((
        2,
        RunnerFailureReason::WorkspaceCapacity,
    ));

    session
        .save_profile(RunnerResourcesProfile {
            version: 2,
            spill: ResourceLimit::Bytes(32 * 1024 * 1024),
            ..profile(2, 2)
        })
        .unwrap();

    let state = session.profile_state();
    assert_eq!(state.effective_version, 1);
    assert_eq!(
        state.apply_failure,
        Some(RunnerFailureReason::WorkspaceCapacity)
    );
    assert_eq!(
        session.begin_query(&RunCancellation::default()),
        Err(RunSessionError::ConfigurationApplyFailed)
    );
}

#[test]
fn readiness_rejection_prevents_session_publication() {
    let applier = Arc::new(RecordingApplier::default());
    *applier.fail_verify.lock().unwrap() = Some((
        1,
        RunnerFailureReason::RunnerVersionMismatch,
    ));

    assert!(matches!(
        RunSession::new(applier, profile(1, 2)),
        Err(RunSessionError::ConfigurationApplyFailed)
    ));
}

#[test]
fn invalid_profiles_are_rejected_before_apply_or_readiness() {
    let applier = Arc::new(RecordingApplier::default());
    let invalid_version = RunnerResourcesProfile {
        version: 0,
        ..RunnerResourcesProfile::default()
    };
    assert!(matches!(
        RunSession::new(applier.clone(), invalid_version),
        Err(RunSessionError::InvalidProfile)
    ));

    let invalid_spill = RunnerResourcesProfile {
        spill: ResourceLimit::Bytes(0),
        ..RunnerResourcesProfile::default()
    };
    assert!(matches!(
        RunSession::new(applier.clone(), invalid_spill),
        Err(RunSessionError::InvalidProfile)
    ));
    assert!(applier.applied.lock().unwrap().is_empty());
    assert!(applier.verified.lock().unwrap().is_empty());
}

#[test]
fn runner_unavailable_is_retained_as_a_sanitized_failure_reason() {
    let applier = Arc::new(RecordingApplier::default());
    let session = RunSession::new(applier.clone(), profile(1, 2)).unwrap();
    *applier.fail_apply.lock().unwrap() = Some((
        2,
        RunnerFailureReason::RunnerUnavailable,
    ));

    session.save_profile(profile(2, 2)).unwrap();

    assert_eq!(
        session.profile_state().apply_failure,
        Some(RunnerFailureReason::RunnerUnavailable)
    );
    assert_eq!(
        session.begin_query(&RunCancellation::default()),
        Err(RunSessionError::ConfigurationApplyFailed)
    );
}

#[test]
fn current_resource_samples_update_while_peaks_remain_monotonic() {
    let mut metrics = SanitizedMetrics::empty();
    metrics.observe_resource_sample(100, 20);
    metrics.observe_resource_sample(250, 80);
    metrics.observe_resource_sample(90, 10);

    assert_eq!(metrics.memory_current_bytes, Some(90));
    assert_eq!(metrics.spill_current_bytes, Some(10));
    assert_eq!(metrics.memory_peak_bytes, Some(250));
    assert_eq!(metrics.spill_peak_bytes, Some(80));
}
