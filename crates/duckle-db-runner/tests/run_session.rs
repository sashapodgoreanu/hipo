use duckle_db_runner::model::{RunCancellation, RunnerFailureReason};
use duckle_db_runner::resources::{AutomaticOrU16, RunnerResourcesProfile};
use duckle_db_runner::run_session::{ProfileApplier, RunSession, RunSessionError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Default)]
struct RecordingApplier {
    applied: Mutex<Vec<u64>>,
    fail_version: Mutex<Option<u64>>,
}

impl ProfileApplier for RecordingApplier {
    fn apply_profile(&self, profile: &RunnerResourcesProfile) -> Result<(), RunnerFailureReason> {
        if *self.fail_version.lock().unwrap() == Some(profile.version) {
            return Err(RunnerFailureReason::ConfigurationApplyFailed);
        }
        self.applied.lock().unwrap().push(profile.version);
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
    assert_eq!(*applier.applied.lock().unwrap(), vec![3]);
}

#[test]
fn failed_apply_keeps_the_effective_profile_and_blocks_new_queries() {
    let applier = Arc::new(RecordingApplier::default());
    let session = RunSession::new(applier.clone(), profile(1, 1)).unwrap();
    *applier.fail_version.lock().unwrap() = Some(2);

    session.save_profile(profile(2, 2)).unwrap();
    assert_eq!(session.profile_state().effective_version, 1);
    assert_eq!(
        session.begin_query(&RunCancellation::default()),
        Err(RunSessionError::ConfigurationApplyFailed)
    );
}

#[test]
fn parallelism_permits_block_at_one_and_allow_eight() {
    let session = Arc::new(RunSession::new(Arc::new(RecordingApplier::default()), profile(1, 1)).unwrap());
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

    let eight = RunSession::new(Arc::new(RecordingApplier::default()), profile(1, 8)).unwrap();
    let permits = (0..8)
        .map(|_| eight.begin_query(&RunCancellation::default()).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(eight.profile_state().active_queries, 8);
    for permit in permits {
        eight.finish_query(permit);
    }
}
