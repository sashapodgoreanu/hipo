//! Per-run profile barrier and query permits.
//!
//! The sidecar is exclusive to one pipeline run, but compatible statements can
//! be concurrent. This module gives those statements a generation-safe permit:
//! a settings save is visible immediately as *desired*, active statements keep
//! their old effective generation, and the next statement waits for one atomic
//! latest-only apply after the drain.

use crate::model::{RunCancellation, RunnerFailureReason};
use crate::run_database::{PreviewResult, RunDatabase, SqlBatchResult, TransferResult};
use crate::resources::{AutomaticOrU16, RunnerResourcesProfile};
use std::collections::HashSet;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use thiserror::Error;

pub trait ProfileApplier: Send + Sync + 'static {
    /// Must atomically apply every effective field before returning success.
    fn apply_profile(&self, profile: &RunnerResourcesProfile) -> Result<(), RunnerFailureReason>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueryPermit {
    pub profile_version: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionProfileState {
    pub requested_version: u64,
    pub effective_version: u64,
    pub active_queries: u16,
    pub maximum_parallel_queries: u16,
    pub apply_pending: bool,
    pub apply_failure: Option<RunnerFailureReason>,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RunSessionError {
    #[error("invalid runner resource profile")]
    InvalidProfile,
    #[error("profile version is older than the desired generation")]
    StaleProfile,
    #[error("query was cancelled while waiting for its runner profile")]
    Cancelled,
    #[error("latest runner configuration could not be applied")]
    ConfigurationApplyFailed,
    #[error("server setup identity is invalid")]
    InvalidSetupIdentity,
    #[error("run database request failed: {0:?}")]
    Database(RunnerFailureReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetupOutcome {
    Applied(SqlBatchResult),
    AlreadyApplied,
}

#[derive(Debug)]
struct State {
    requested: RunnerResourcesProfile,
    effective: RunnerResourcesProfile,
    active_queries: u16,
    applying: bool,
    failure: Option<RunnerFailureReason>,
    completed_server_setups: HashSet<String>,
    pending_server_setups: HashSet<String>,
}

/// Owns only local session state and a provider-private applier. It does not
/// expose a raw DuckDB connection, Quack URI, process or capability.
pub struct RunSession {
    applier: Arc<dyn ProfileApplier>,
    state: Mutex<State>,
    changed: Condvar,
}

impl RunSession {
    pub fn new(
        applier: Arc<dyn ProfileApplier>,
        initial_profile: RunnerResourcesProfile,
    ) -> Result<Self, RunSessionError> {
        initial_profile
            .validate()
            .map_err(|_| RunSessionError::InvalidProfile)?;
        Ok(Self {
            applier,
            state: Mutex::new(State {
                requested: initial_profile.clone(),
                effective: initial_profile,
                active_queries: 0,
                applying: false,
                failure: None,
                completed_server_setups: HashSet::new(),
                pending_server_setups: HashSet::new(),
            }),
            changed: Condvar::new(),
        })
    }

    /// Atomically records a complete desired generation. A concurrent save
    /// simply replaces it; only the last value seen after active work drains is
    /// ever sent to the sidecar.
    pub fn save_profile(&self, profile: RunnerResourcesProfile) -> Result<(), RunSessionError> {
        profile
            .validate()
            .map_err(|_| RunSessionError::InvalidProfile)?;
        let should_apply = {
            let mut state = self.lock();
            if profile.version <= state.requested.version {
                return Err(RunSessionError::StaleProfile);
            }
            state.requested = profile;
            state.failure = None;
            if state.active_queries == 0
                && !state.applying
                && state.requested.version != state.effective.version
            {
                state.applying = true;
                true
            } else {
                self.changed.notify_all();
                false
            }
        };
        if should_apply {
            self.apply_latest();
        }
        Ok(())
    }

    /// Waits for both the safe profile generation and an available parallelism
    /// permit. The short timed wait makes cancellation deterministic without
    /// requiring a provider-specific command channel.
    pub fn begin_query(
        &self,
        cancellation: &RunCancellation,
    ) -> Result<QueryPermit, RunSessionError> {
        loop {
            let mut state = self.lock();
            if cancellation.is_cancelled() {
                return Err(RunSessionError::Cancelled);
            }
            if state.failure.is_some() && state.requested.version != state.effective.version {
                return Err(RunSessionError::ConfigurationApplyFailed);
            }
            let current = state.requested.version == state.effective.version && !state.applying;
            let maximum = effective_parallelism(&state.effective);
            if current && state.active_queries < maximum {
                state.active_queries += 1;
                return Ok(QueryPermit {
                    profile_version: state.effective.version,
                });
            }
            let (next, _) = self
                .changed
                .wait_timeout(state, Duration::from_millis(20))
                .expect("run session state poisoned");
            drop(next);
        }
    }

    /// Completes a statement. If it was the last statement under an obsolete
    /// profile, this thread performs the one atomic latest-only apply before it
    /// wakes future queries.
    pub fn finish_query(&self, _permit: QueryPermit) {
        let should_apply = {
            let mut state = self.lock();
            state.active_queries = state.active_queries.saturating_sub(1);
            let apply = state.active_queries == 0
                && !state.applying
                && state.requested.version != state.effective.version;
            if apply {
                state.applying = true;
            }
            self.changed.notify_all();
            apply
        };
        if should_apply {
            self.apply_latest();
        }
    }

    /// Executes a planned SQL batch while holding one profile-safe permit.
    /// The database remains opaque: callers never receive its connection or
    /// any Quack capability.
    pub fn execute_batch(
        &self,
        database: &RunDatabase,
        statements: Vec<String>,
    ) -> Result<SqlBatchResult, RunSessionError> {
        let permit = self.begin_query(database.cancellation())?;
        let result = database.execute_batch(statements).map_err(RunSessionError::Database);
        self.finish_query(permit);
        result
    }

    pub fn setup(
        &self,
        database: &RunDatabase,
        statements: Vec<String>,
    ) -> Result<SqlBatchResult, RunSessionError> {
        let permit = self.begin_query(database.cancellation())?;
        let result = database.setup(statements).map_err(RunSessionError::Database);
        self.finish_query(permit);
        result
    }

    /// Applies a planner-owned server setup at most once for the given
    /// resource identity. Concurrent callers wait for the first complete
    /// batch, keeping the resulting catalog state private to this run.
    pub fn setup_once(
        &self,
        database: &RunDatabase,
        resource_identity: &str,
        statements: Vec<String>,
    ) -> Result<SetupOutcome, RunSessionError> {
        if resource_identity.trim().is_empty() {
            return Err(RunSessionError::InvalidSetupIdentity);
        }

        loop {
            let mut state = self.lock();
            if database.cancellation().is_cancelled() {
                return Err(RunSessionError::Cancelled);
            }
            if state.completed_server_setups.contains(resource_identity) {
                return Ok(SetupOutcome::AlreadyApplied);
            }
            if state.pending_server_setups.insert(resource_identity.to_string()) {
                break;
            }
            let (next, _) = self
                .changed
                .wait_timeout(state, Duration::from_millis(20))
                .expect("run session state poisoned");
            drop(next);
        }

        let result = self.setup(database, statements).map(SetupOutcome::Applied);
        let mut state = self.lock();
        state.pending_server_setups.remove(resource_identity);
        if result.is_ok() {
            state.completed_server_setups.insert(resource_identity.to_string());
        }
        self.changed.notify_all();
        result
    }

    pub fn preview(
        &self,
        database: &RunDatabase,
        sql: &str,
        limit: u32,
    ) -> Result<PreviewResult, RunSessionError> {
        let permit = self.begin_query(database.cancellation())?;
        let result = database.preview(sql, limit).map_err(RunSessionError::Database);
        self.finish_query(permit);
        result
    }

    pub fn transfer(
        &self,
        database: &RunDatabase,
        sql: &str,
        transport: crate::model::TransportKind,
    ) -> Result<TransferResult, RunSessionError> {
        let permit = self.begin_query(database.cancellation())?;
        let result = database
            .transfer(sql, transport)
            .map_err(RunSessionError::Database);
        self.finish_query(permit);
        result
    }

    pub fn profile_state(&self) -> SessionProfileState {
        let state = self.lock();
        SessionProfileState {
            requested_version: state.requested.version,
            effective_version: state.effective.version,
            active_queries: state.active_queries,
            maximum_parallel_queries: effective_parallelism(&state.effective),
            apply_pending: state.applying || state.requested.version != state.effective.version,
            apply_failure: state.failure,
        }
    }

    fn apply_latest(&self) {
        loop {
            let profile = { self.lock().requested.clone() };
            let result = self.applier.apply_profile(&profile);
            let mut state = self.lock();
            match result {
                Ok(()) => {
                    // A newer save that raced the apply is never exposed as an
                    // effective partial profile. Loop once more for it.
                    state.effective = profile;
                    state.failure = None;
                    if state.requested.version == state.effective.version {
                        state.applying = false;
                        self.changed.notify_all();
                        return;
                    }
                }
                Err(reason) => {
                    // The old `effective` profile intentionally remains
                    // untouched. New queries receive only the stable reason.
                    state.failure = Some(reason);
                    state.applying = false;
                    self.changed.notify_all();
                    return;
                }
            }
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().expect("run session state poisoned")
    }
}

fn effective_parallelism(profile: &RunnerResourcesProfile) -> u16 {
    match profile.quack_parallelism {
        AutomaticOrU16::Automatic => RunnerResourcesProfile::MAX_QUACK_PARALLELISM,
        AutomaticOrU16::Value(value) => value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TransportKind;
    use crate::run_database::RunDatabaseTransport;
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingApplier {
        applied: Mutex<Vec<u64>>,
        fail_version: Mutex<Option<u64>>,
    }

    impl ProfileApplier for RecordingApplier {
        fn apply_profile(
            &self,
            profile: &RunnerResourcesProfile,
        ) -> Result<(), RunnerFailureReason> {
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
    fn active_queries_keep_old_profile_and_saves_coalesce_to_latest() {
        let applier = Arc::new(RecordingApplier::default());
        let session = RunSession::new(applier.clone(), profile(1, 2)).unwrap();
        let cancellation = RunCancellation::default();
        let one = session.begin_query(&cancellation).unwrap();
        let two = session.begin_query(&cancellation).unwrap();
        session.save_profile(profile(2, 4)).unwrap();
        session.save_profile(profile(3, 8)).unwrap();
        assert_eq!(session.profile_state().effective_version, 1);
        session.finish_query(one);
        assert_eq!(session.profile_state().effective_version, 1);
        session.finish_query(two);
        let state = session.profile_state();
        assert_eq!(state.effective_version, 3);
        assert_eq!(state.maximum_parallel_queries, 8);
        assert_eq!(*applier.applied.lock().unwrap(), vec![3]);
    }

    #[test]
    fn apply_failure_preserves_prior_effective_profile_and_rejects_new_queries() {
        let applier = Arc::new(RecordingApplier::default());
        let session = RunSession::new(applier.clone(), profile(1, 1)).unwrap();
        *applier.fail_version.lock().unwrap() = Some(2);
        session.save_profile(profile(2, 2)).unwrap();
        assert_eq!(session.profile_state().effective_version, 1);
        assert_eq!(
            session.begin_query(&RunCancellation::default()),
            Err(RunSessionError::ConfigurationApplyFailed)
        );
        *applier.fail_version.lock().unwrap() = None;
        session.save_profile(profile(3, 2)).unwrap();
        assert_eq!(session.profile_state().effective_version, 3);
    }

    #[test]
    fn rejects_a_profile_that_reuses_the_requested_generation() {
        let session = RunSession::new(Arc::new(RecordingApplier::default()), profile(1, 1)).unwrap();

        assert_eq!(
            session.save_profile(profile(1, 8)),
            Err(RunSessionError::StaleProfile)
        );
        assert_eq!(session.profile_state().requested_version, 1);
    }

    #[derive(Default)]
    struct RecordingTransport {
        batches: Mutex<Vec<Vec<String>>>,
    }

    impl RunDatabaseTransport for RecordingTransport {
        fn execute_batch(
            &self,
            statements: &[String],
            _cancellation: &RunCancellation,
        ) -> Result<SqlBatchResult, RunnerFailureReason> {
            self.batches.lock().unwrap().push(statements.to_vec());
            Ok(SqlBatchResult {
                rows: 1,
                transport: TransportKind::Quack,
            })
        }

        fn preview(
            &self,
            _sql: &str,
            _limit: u32,
            _cancellation: &RunCancellation,
        ) -> Result<PreviewResult, RunnerFailureReason> {
            Err(RunnerFailureReason::RunnerUnavailable)
        }

        fn transfer(
            &self,
            _sql: &str,
            _transport: TransportKind,
            _cancellation: &RunCancellation,
        ) -> Result<TransferResult, RunnerFailureReason> {
            Err(RunnerFailureReason::RunnerUnavailable)
        }
    }

    #[test]
    fn server_setup_is_batched_once_per_resource_identity() {
        let transport = Arc::new(RecordingTransport::default());
        let database = RunDatabase::new(transport.clone(), RunCancellation::default());
        let session = RunSession::new(Arc::new(RecordingApplier::default()), profile(1, 1)).unwrap();
        let statements = vec!["SET threads = 2".to_string(), "CREATE TABLE source AS SELECT 1".to_string()];

        assert!(matches!(
            session.setup_once(&database, "src.orders", statements.clone()),
            Ok(SetupOutcome::Applied(SqlBatchResult { rows: 1, transport: TransportKind::Quack }))
        ));
        assert_eq!(
            session.setup_once(&database, "src.orders", statements),
            Ok(SetupOutcome::AlreadyApplied)
        );
        assert_eq!(
            *transport.batches.lock().unwrap(),
            vec![vec![
                "SET threads = 2".to_string(),
                "CREATE TABLE source AS SELECT 1".to_string(),
            ]]
        );
    }
}
