//! Controller-owned elastic worker pool.
//!
//! No caller can spawn a worker directly. `acquire` records demand exactly once
//! and atomically leases a ready warm worker or provisions a dedicated
//! on-demand worker. On-demand workers never enter the warm-capacity count.

use crate::autoscaler::{AutoscaleDecision, ElasticAutoscaler, ScaleAction};
use crate::demand::DemandWindow;
use crate::events::{RunnerEvent, RunnerEventKind, ScaleDirection, ScaleReason, ScaleTelemetry};
use crate::model::{
    AcquireRequest, RunCancellation, RunId, RunnerFailureReason, WorkerId, WorkerKind, WorkerLease,
    WorkerLeaseId, WorkerState,
};
use crate::resources::RunnerResourcesProfile;
use crate::run_database::{RunDatabase, SqlBatchResult};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use thiserror::Error;

/// Provider-facing request intentionally contains only opaque identifiers and
/// the effective requested profile. Endpoint, bootstrap transport and process
/// handles remain provider-private.
#[derive(Debug, Clone)]
pub struct WorkerProvisionRequest {
    pub worker_id: WorkerId,
    pub kind: WorkerKind,
    pub profile: RunnerResourcesProfile,
    pub cancellation: RunCancellation,
}

/// The local-process provider is the only implementation allowed to know how
/// a sidecar is bootstrapped. The pool only receives a sanitized outcome.
pub trait WorkerProvider: Send + Sync + 'static {
    fn provision(&self, request: WorkerProvisionRequest) -> Result<(), RunnerFailureReason>;

    /// Applies a complete profile to an idle warm worker before it is returned
    /// to `ready`. The default supports providers whose worker is recreated for
    /// every profile change; the local provider overrides it.
    fn apply_profile(
        &self,
        _worker_id: WorkerId,
        _profile: &RunnerResourcesProfile,
    ) -> Result<(), RunnerFailureReason> {
        Ok(())
    }

    /// Opens the provider-private database facade for one currently leased
    /// worker. The opaque facade may execute only planner-owned operations;
    /// it never exposes a connection, endpoint, or capability to the caller.
    fn open_database(
        &self,
        _worker_id: WorkerId,
        _cancellation: RunCancellation,
    ) -> Result<RunDatabase, RunnerFailureReason> {
        Err(RunnerFailureReason::RunnerUnavailable)
    }

    /// Must terminate the complete process scope and sweep its run-scoped
    /// artifacts. It must be idempotent because crash/cancel can race cleanup.
    fn terminate(&self, worker_id: WorkerId);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolCounts {
    pub starting_warm: u32,
    pub ready_warm: u32,
    pub leased_warm: u32,
    pub leased_on_demand: u32,
}

impl PoolCounts {
    pub fn warm_capacity(self) -> u32 {
        self.starting_warm + self.ready_warm + self.leased_warm
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolSnapshot {
    pub counts: PoolCounts,
    pub target_warm_capacity: u32,
    pub active_demand: u32,
    pub peak_5m: u32,
    pub profile_version: u64,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PoolError {
    #[error("invalid runner resource profile")]
    InvalidProfile,
    #[error("run was cancelled before a worker lease was granted")]
    Cancelled,
    #[error("run already has a worker request or lease")]
    DuplicateRun,
    #[error("worker provisioning failed: {0:?}")]
    Provision(RunnerFailureReason),
    #[error("worker lease is not owned by this controller")]
    UnknownLease,
    #[error("worker pool is shutting down")]
    ShuttingDown,
}

#[derive(Debug)]
struct WorkerRecord {
    kind: WorkerKind,
    state: WorkerState,
    profile_version: u64,
    cancellation: RunCancellation,
}

#[derive(Debug)]
struct PoolState {
    desired_profile: RunnerResourcesProfile,
    autoscaler: ElasticAutoscaler,
    demand: DemandWindow,
    target_warm_capacity: u32,
    workers: HashMap<WorkerId, WorkerRecord>,
    ready: VecDeque<WorkerId>,
    leases: HashMap<WorkerLeaseId, WorkerLease>,
    run_leases: HashMap<RunId, WorkerLeaseId>,
    pending_runs: HashMap<RunId, WorkerId>,
    events: Vec<RunnerEvent>,
    shutting_down: bool,
}

/// The one controller for the currently open workspace instance.
#[derive(Clone)]
pub struct WorkerPoolControl {
    provider: Arc<dyn WorkerProvider>,
    state: Arc<Mutex<PoolState>>,
    changed: Arc<Condvar>,
}

impl WorkerPoolControl {
    pub fn new(
        provider: Arc<dyn WorkerProvider>,
        profile: RunnerResourcesProfile,
        now_millis: u64,
    ) -> Result<Self, PoolError> {
        profile.validate().map_err(|_| PoolError::InvalidProfile)?;
        let controller = Self {
            provider,
            state: Arc::new(Mutex::new(PoolState {
                target_warm_capacity: profile.base_capacity,
                autoscaler: ElasticAutoscaler::new(profile.base_capacity),
                desired_profile: profile,
                demand: DemandWindow::default(),
                workers: HashMap::new(),
                ready: VecDeque::new(),
                leases: HashMap::new(),
                run_leases: HashMap::new(),
                pending_runs: HashMap::new(),
                events: Vec::new(),
                shutting_down: false,
            })),
            changed: Arc::new(Condvar::new()),
        };
        controller.reconcile(now_millis, ScaleReason::Startup);
        Ok(controller)
    }

    /// Controller-only allocation. This never waits for a warm pool resize: a
    /// missing ready worker causes direct, dedicated provisioning for this run.
    pub fn acquire(
        &self,
        request: AcquireRequest,
        now_millis: u64,
    ) -> Result<WorkerLease, PoolError> {
        request
            .profile
            .validate()
            .map_err(|_| PoolError::InvalidProfile)?;
        if request.cancellation.is_cancelled() {
            return Err(PoolError::Cancelled);
        }

        let (on_demand, provision_profile) = {
            let mut state = self.lock_state();
            if state.shutting_down {
                return Err(PoolError::ShuttingDown);
            }
            if state.run_leases.contains_key(&request.run_id)
                || state.pending_runs.contains_key(&request.run_id)
            {
                return Err(PoolError::DuplicateRun);
            }
            if request.profile.version != state.desired_profile.version {
                return Err(PoolError::InvalidProfile);
            }
            state.demand.acquire(request.run_id, now_millis);
            state.events.push(RunnerEvent::lifecycle(
                now_millis,
                RunnerEventKind::AcquireRequested,
                Some(request.run_id),
                None,
                None,
                None,
            ));

            if let Some(worker_id) = pop_current_ready(&mut state) {
                let lease = grant_lease(&mut state, worker_id, request.run_id, now_millis);
                state.events.push(RunnerEvent::lifecycle(
                    now_millis,
                    RunnerEventKind::AllocationDecided,
                    Some(request.run_id),
                    Some(worker_id),
                    Some(lease.lease_id),
                    Some(WorkerKind::Warm),
                ));
                state.events.push(RunnerEvent::lifecycle(
                    now_millis,
                    RunnerEventKind::LeaseGranted,
                    Some(request.run_id),
                    Some(worker_id),
                    Some(lease.lease_id),
                    Some(WorkerKind::Warm),
                ));
                self.changed.notify_all();
                return Ok(lease);
            }

            let worker_id = WorkerId::new();
            let provision_profile = state.desired_profile.clone();
            state.workers.insert(
                worker_id,
                WorkerRecord {
                    kind: WorkerKind::OnDemand,
                    state: WorkerState::Starting,
                    profile_version: provision_profile.version,
                    cancellation: request.cancellation.clone(),
                },
            );
            state.pending_runs.insert(request.run_id, worker_id);
            state.events.push(RunnerEvent::lifecycle(
                now_millis,
                RunnerEventKind::AllocationDecided,
                Some(request.run_id),
                Some(worker_id),
                None,
                Some(WorkerKind::OnDemand),
            ));
            state.events.push(RunnerEvent::lifecycle(
                now_millis,
                RunnerEventKind::ProvisionStarted,
                Some(request.run_id),
                Some(worker_id),
                None,
                Some(WorkerKind::OnDemand),
            ));
            (worker_id, provision_profile)
        };

        let provision = self.provider.provision(WorkerProvisionRequest {
            worker_id: on_demand,
            kind: WorkerKind::OnDemand,
            profile: provision_profile.clone(),
            cancellation: request.cancellation.clone(),
        });
        if let Err(reason) = provision {
            // Cancellation/shutdown owns the terminal outcome even if process
            // loss is observed at the same time. Provider diagnostics must not
            // turn an intentional termination into runner_crashed.
            let cancelled = request.cancellation.is_cancelled() || {
                let state = self.lock_state();
                state.shutting_down || !state.pending_runs.contains_key(&request.run_id)
            };
            let terminal_reason = if cancelled {
                RunnerFailureReason::Cancelled
            } else {
                reason
            };
            self.fail_pending_acquire(request.run_id, on_demand, terminal_reason, now_millis);
            self.provider.terminate(on_demand);
            return if cancelled {
                Err(PoolError::Cancelled)
            } else {
                Err(PoolError::Provision(reason))
            };
        }

        // A save while this worker was bootstrapping is applied before the
        // first lease, never after a query begins with an obsolete profile.
        let latest_profile = self.lock_state().desired_profile.clone();
        if latest_profile.version != provision_profile.version {
            if let Err(reason) = self.provider.apply_profile(on_demand, &latest_profile) {
                self.fail_pending_acquire(request.run_id, on_demand, reason, now_millis);
                self.provider.terminate(on_demand);
                return Err(PoolError::Provision(reason));
            }
        }

        let result = {
            let mut state = self.lock_state();
            let cancelled = request.cancellation.is_cancelled()
                || state.shutting_down
                || !matches!(state.pending_runs.get(&request.run_id), Some(worker_id) if *worker_id == on_demand);
            if cancelled {
                state.pending_runs.remove(&request.run_id);
                state.workers.remove(&on_demand);
                state.demand.terminal(request.run_id, now_millis);
                state.events.push(
                    RunnerEvent::lifecycle(
                        now_millis,
                        RunnerEventKind::WorkerFailed,
                        Some(request.run_id),
                        Some(on_demand),
                        None,
                        Some(WorkerKind::OnDemand),
                    )
                    .failure(RunnerFailureReason::Cancelled),
                );
                Err(PoolError::Cancelled)
            } else {
                let effective_version = state.desired_profile.version;
                let record = state
                    .workers
                    .get_mut(&on_demand)
                    .expect("pending worker exists");
                record.state = WorkerState::Leased;
                record.profile_version = effective_version;
                state.pending_runs.remove(&request.run_id);
                let lease = grant_lease(&mut state, on_demand, request.run_id, now_millis);
                state.events.push(RunnerEvent::lifecycle(
                    now_millis,
                    RunnerEventKind::WorkerReady,
                    Some(request.run_id),
                    Some(on_demand),
                    Some(lease.lease_id),
                    Some(WorkerKind::OnDemand),
                ));
                state.events.push(RunnerEvent::lifecycle(
                    now_millis,
                    RunnerEventKind::LeaseGranted,
                    Some(request.run_id),
                    Some(on_demand),
                    Some(lease.lease_id),
                    Some(WorkerKind::OnDemand),
                ));
                Ok(lease)
            }
        };
        self.changed.notify_all();
        if result.is_err() {
            self.provider.terminate(on_demand);
        }
        result
    }

    /// Acquires using the profile that is current at the controller boundary.
    /// Entry points must not cache or construct a profile themselves: a save
    /// can replace the desired generation while a run is being prepared.
    pub fn acquire_for_current_profile(
        &self,
        run_id: RunId,
        attempt: u32,
        cancellation: RunCancellation,
        now_millis: u64,
    ) -> Result<WorkerLease, PoolError> {
        let profile = self.lock_state().desired_profile.clone();
        self.acquire(
            AcquireRequest {
                run_id,
                attempt,
                profile,
                cancellation,
            },
            now_millis,
        )
    }

    /// Dispatches a planned SQL batch through the database associated with an
    /// active exclusive lease. A stale or foreign lease cannot open a worker
    /// database, and no caller receives the underlying transport details.
    pub fn execute_database_batch(
        &self,
        lease: &WorkerLease,
        statements: Vec<String>,
        cancellation: RunCancellation,
    ) -> Result<SqlBatchResult, RunnerFailureReason> {
        let owned = self
            .lock_state()
            .leases
            .get(&lease.lease_id)
            .is_some_and(|stored| stored == lease);
        if !owned {
            return Err(RunnerFailureReason::RunnerUnavailable);
        }
        let database = self.provider.open_database(lease.worker_id, cancellation)?;
        database.execute_batch(statements)
    }

    /// Runs a preview query against the database of a currently leased worker.
    /// The caller receives column names, rows, and a truncation flag; no
    /// connection, endpoint, or credential is exposed.
    pub fn preview_database_relation(
        &self,
        lease: &WorkerLease,
        sql: &str,
        limit: u32,
        cancellation: RunCancellation,
    ) -> Result<crate::run_database::PreviewResult, RunnerFailureReason> {
        let owned = self
            .lock_state()
            .leases
            .get(&lease.lease_id)
            .is_some_and(|stored| stored == lease);
        if !owned {
            return Err(RunnerFailureReason::RunnerUnavailable);
        }
        let database = self.provider.open_database(lease.worker_id, cancellation)?;
        database.preview(sql, limit)
    }

    /// Releases an exclusive single-use worker. The worker is never returned
    /// to ready; only the normal autoscaler may create a replacement.
    pub fn release(&self, lease: WorkerLease, now_millis: u64) -> Result<(), PoolError> {
        let worker_id = {
            let mut state = self.lock_state();
            let Some(stored) = state.leases.remove(&lease.lease_id) else {
                return Err(PoolError::UnknownLease);
            };
            if stored != lease {
                state.leases.insert(stored.lease_id, stored);
                return Err(PoolError::UnknownLease);
            }
            state.run_leases.remove(&lease.run_id);
            state.demand.terminal(lease.run_id, now_millis);
            if let Some(record) = state.workers.get_mut(&lease.worker_id) {
                record.state = WorkerState::Terminating;
            }
            state.events.push(RunnerEvent::lifecycle(
                now_millis,
                RunnerEventKind::LeaseReleased,
                Some(lease.run_id),
                Some(lease.worker_id),
                Some(lease.lease_id),
                Some(lease.worker_kind),
            ));
            lease.worker_id
        };
        self.provider.terminate(worker_id);
        let mut state = self.lock_state();
        state.workers.remove(&worker_id);
        state.events.push(RunnerEvent::lifecycle(
            now_millis,
            RunnerEventKind::CleanupCompleted,
            Some(lease.run_id),
            Some(worker_id),
            Some(lease.lease_id),
            Some(lease.worker_kind),
        ));
        drop(state);
        self.changed.notify_all();
        Ok(())
    }

    /// Marks a pending bootstrap as cancelled or releases a leased run.
    pub fn cancel_run(&self, run_id: RunId, now_millis: u64) -> Result<(), PoolError> {
        let pending = {
            let mut state = self.lock_state();
            if let Some(lease_id) = state.run_leases.get(&run_id).copied() {
                let lease = state
                    .leases
                    .get(&lease_id)
                    .cloned()
                    .expect("lease index is valid");
                drop(state);
                return self.release(lease, now_millis);
            }
            let Some(worker_id) = state.pending_runs.remove(&run_id) else {
                return Ok(());
            };
            if let Some(record) = state.workers.get_mut(&worker_id) {
                record.cancellation.cancel();
                record.state = WorkerState::Terminating;
            }
            state.demand.terminal(run_id, now_millis);
            Some(worker_id)
        };
        if let Some(worker_id) = pending {
            self.provider.terminate(worker_id);
        }
        self.changed.notify_all();
        Ok(())
    }

    /// Performs the regular five-second evaluation. A shorter call leaves
    /// target/capacity untouched and cannot produce an extra provision batch.
    pub fn autoscale_tick(&self, now_millis: u64) {
        self.reconcile(now_millis, ScaleReason::PeriodicTick);
    }

    /// Makes a complete profile immediately desired. Idle warm workers apply
    /// it before they become leaseable; leased workers are intentionally left
    /// to their `RunSession` drain barrier.
    pub fn set_desired_profile(
        &self,
        profile: RunnerResourcesProfile,
        now_millis: u64,
    ) -> Result<(), PoolError> {
        profile.validate().map_err(|_| PoolError::InvalidProfile)?;
        let ready_to_apply = {
            let mut state = self.lock_state();
            if profile.version <= state.desired_profile.version {
                return Err(PoolError::InvalidProfile);
            }
            state.desired_profile = profile.clone();
            state.autoscaler.set_base_capacity(profile.base_capacity);
            let mut ready_to_apply = Vec::new();
            for worker_id in state.ready.drain(..).collect::<Vec<_>>() {
                if let Some(record) = state.workers.get_mut(&worker_id) {
                    record.state = WorkerState::Starting;
                    ready_to_apply.push(worker_id);
                }
            }
            state.events.push(RunnerEvent::lifecycle(
                now_millis,
                RunnerEventKind::ProfileApplyStarted,
                None,
                None,
                None,
                None,
            ));
            ready_to_apply
        };
        // Establish the new target before any asynchronous apply completes, so
        // those completions can make a target-aware publish/terminate choice.
        self.reconcile(now_millis, ScaleReason::ProfileChanged);
        for worker_id in ready_to_apply {
            let controller = self.clone();
            let profile = profile.clone();
            thread::spawn(move || {
                controller.finish_ready_profile_apply(worker_id, profile, now_millis)
            });
        }
        Ok(())
    }

    pub fn snapshot(&self, now_millis: u64) -> PoolSnapshot {
        let mut state = self.lock_state();
        let observation = state.demand.observe(now_millis, false);
        PoolSnapshot {
            counts: counts(&state),
            target_warm_capacity: state.target_warm_capacity,
            active_demand: observation.active_runs,
            peak_5m: observation.peak_5m,
            profile_version: state.desired_profile.version,
        }
    }

    pub fn events(&self) -> Vec<RunnerEvent> {
        self.lock_state().events.clone()
    }

    /// Test and integration helper; production callers should use events.
    pub fn wait_for_ready_count(&self, expected: u32, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut state = self.lock_state();
        loop {
            if counts(&state).ready_warm == expected {
                return true;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            let (next, result) = self
                .changed
                .wait_timeout(state, remaining)
                .expect("pool state poisoned");
            state = next;
            if result.timed_out() && counts(&state).ready_warm != expected {
                return false;
            }
        }
    }

    pub fn shutdown(&self, now_millis: u64) {
        let workers = {
            let mut state = self.lock_state();
            state.shutting_down = true;
            state
                .workers
                .iter_mut()
                .map(|(worker_id, record)| {
                    record.cancellation.cancel();
                    record.state = WorkerState::Terminating;
                    *worker_id
                })
                .collect::<Vec<_>>()
        };
        for worker_id in workers {
            self.provider.terminate(worker_id);
        }
        let mut state = self.lock_state();
        state.workers.clear();
        state.ready.clear();
        state.leases.clear();
        state.run_leases.clear();
        state.pending_runs.clear();
        state.events.push(RunnerEvent::lifecycle(
            now_millis,
            RunnerEventKind::CleanupCompleted,
            None,
            None,
            None,
            None,
        ));
        drop(state);
        self.changed.notify_all();
    }

    fn fail_pending_acquire(
        &self,
        run_id: RunId,
        worker_id: WorkerId,
        reason: RunnerFailureReason,
        now_millis: u64,
    ) {
        let mut state = self.lock_state();
        state.pending_runs.remove(&run_id);
        state.workers.remove(&worker_id);
        state.demand.terminal(run_id, now_millis);
        state.events.push(
            RunnerEvent::lifecycle(
                now_millis,
                RunnerEventKind::WorkerFailed,
                Some(run_id),
                Some(worker_id),
                None,
                Some(WorkerKind::OnDemand),
            )
            .failure(reason),
        );
        drop(state);
        self.changed.notify_all();
    }

    fn reconcile(&self, now_millis: u64, reason: ScaleReason) {
        let (to_start, to_terminate) = {
            let mut state = self.lock_state();
            if state.shutting_down {
                return;
            }
            let observation = state.demand.observe(now_millis, false);
            let current = counts(&state);
            let decision = if matches!(reason, ScaleReason::Startup | ScaleReason::ProfileChanged) {
                state
                    .autoscaler
                    .evaluate_now(now_millis, observation, current.warm_capacity())
            } else {
                let Some(decision) = state.autoscaler.evaluate_if_due(
                    now_millis,
                    observation,
                    current.warm_capacity(),
                ) else {
                    return;
                };
                decision
            };
            state.target_warm_capacity = decision.target_warm_capacity;
            let profile = state.desired_profile.clone();
            let mut to_start = Vec::new();
            let mut to_terminate = Vec::new();
            match decision.action {
                ScaleAction::ScaleOut { workers } => {
                    for _ in 0..workers {
                        let worker_id = WorkerId::new();
                        let cancellation = RunCancellation::default();
                        state.workers.insert(
                            worker_id,
                            WorkerRecord {
                                kind: WorkerKind::Warm,
                                state: WorkerState::Starting,
                                profile_version: profile.version,
                                cancellation: cancellation.clone(),
                            },
                        );
                        state.events.push(RunnerEvent::lifecycle(
                            now_millis,
                            RunnerEventKind::ProvisionStarted,
                            None,
                            Some(worker_id),
                            None,
                            Some(WorkerKind::Warm),
                        ));
                        to_start.push((worker_id, profile.clone(), cancellation));
                    }
                }
                ScaleAction::ScaleIn { workers } => {
                    // Scale-in never interrupts or replaces a leased worker. It
                    // only terminates ready capacity; starting workers are left
                    // to finish and are considered at the next regular tick.
                    for _ in 0..workers {
                        let Some(worker_id) = state.ready.pop_back() else {
                            break;
                        };
                        if let Some(record) = state.workers.get_mut(&worker_id) {
                            record.state = WorkerState::Terminating;
                            to_terminate.push(worker_id);
                        }
                    }
                }
                ScaleAction::Hold => {}
            }
            let after = counts(&state);
            state.events.push(scale_event(
                now_millis,
                decision,
                reason,
                after,
                to_start.len() as u32,
                to_terminate.len() as u32,
            ));
            (to_start, to_terminate)
        };
        for (worker_id, profile, cancellation) in to_start {
            let controller = self.clone();
            thread::spawn(move || {
                controller.finish_warm_start(worker_id, profile, cancellation, now_millis)
            });
        }
        for worker_id in to_terminate {
            let controller = self.clone();
            thread::spawn(move || controller.finish_termination(worker_id, now_millis));
        }
        self.changed.notify_all();
    }

    fn finish_warm_start(
        &self,
        worker_id: WorkerId,
        profile: RunnerResourcesProfile,
        cancellation: RunCancellation,
        now_millis: u64,
    ) {
        let result = self.provider.provision(WorkerProvisionRequest {
            worker_id,
            kind: WorkerKind::Warm,
            profile: profile.clone(),
            cancellation,
        });
        if let Err(reason) = result {
            let mut state = self.lock_state();
            let Some(record) = state.workers.get_mut(&worker_id) else {
                return;
            };
            record.state = WorkerState::Failed;
            state.events.push(
                RunnerEvent::lifecycle(
                    now_millis,
                    RunnerEventKind::WorkerFailed,
                    None,
                    Some(worker_id),
                    None,
                    Some(WorkerKind::Warm),
                )
                .failure(reason),
            );
            drop(state);
            self.finish_termination(worker_id, now_millis);
            return;
        }
        self.converge_starting_worker(
            worker_id,
            profile.version,
            RunnerEventKind::WorkerReady,
            now_millis,
        );
    }

    fn finish_ready_profile_apply(
        &self,
        worker_id: WorkerId,
        profile: RunnerResourcesProfile,
        now_millis: u64,
    ) {
        match self.provider.apply_profile(worker_id, &profile) {
            Ok(()) => self.converge_starting_worker(
                worker_id,
                profile.version,
                RunnerEventKind::ProfileApplied,
                now_millis,
            ),
            Err(reason) => self.fail_profile_apply(worker_id, reason, now_millis),
        }
    }

    /// Coalesces any saves which arrived while a warm worker was provisioning
    /// or applying a previous generation. Only the latest desired generation
    /// can become ready. Excess capacity terminates instead of doing obsolete
    /// work, while leased workers remain untouched outside this state machine.
    fn converge_starting_worker(
        &self,
        worker_id: WorkerId,
        mut applied_version: u64,
        published_event: RunnerEventKind,
        now_millis: u64,
    ) {
        loop {
            let next_profile = {
                let mut state = self.lock_state();
                let current_warm_capacity = counts(&state).warm_capacity();
                let desired_profile = state.desired_profile.clone();
                let should_terminate =
                    state.shutting_down || current_warm_capacity > state.target_warm_capacity;
                let Some(record) = state.workers.get_mut(&worker_id) else {
                    return;
                };
                if record.state != WorkerState::Starting || should_terminate {
                    record.state = WorkerState::Terminating;
                    None
                } else if applied_version == desired_profile.version {
                    record.profile_version = desired_profile.version;
                    record.state = WorkerState::Ready;
                    state.ready.push_back(worker_id);
                    state.events.push(RunnerEvent::lifecycle(
                        now_millis,
                        published_event.clone(),
                        None,
                        Some(worker_id),
                        None,
                        Some(WorkerKind::Warm),
                    ));
                    drop(state);
                    self.changed.notify_all();
                    return;
                } else {
                    Some(desired_profile)
                }
            };
            let Some(profile) = next_profile else {
                self.finish_termination(worker_id, now_millis);
                return;
            };
            match self.provider.apply_profile(worker_id, &profile) {
                Ok(()) => applied_version = profile.version,
                Err(reason) => {
                    self.fail_profile_apply(worker_id, reason, now_millis);
                    return;
                }
            }
        }
    }

    fn fail_profile_apply(
        &self,
        worker_id: WorkerId,
        reason: RunnerFailureReason,
        now_millis: u64,
    ) {
        let mut state = self.lock_state();
        let Some(record) = state.workers.get_mut(&worker_id) else {
            return;
        };
        record.state = WorkerState::Failed;
        state.events.push(
            RunnerEvent::lifecycle(
                now_millis,
                RunnerEventKind::WorkerFailed,
                None,
                Some(worker_id),
                None,
                Some(WorkerKind::Warm),
            )
            .failure(RunnerFailureReason::ConfigurationApplyFailed.max_reason(reason)),
        );
        drop(state);
        self.finish_termination(worker_id, now_millis);
        // A provider that cannot safely mutate an already-started worker (the
        // local Quack sidecar is one) reports ConfigurationApplyFailed. The
        // terminated warm slot must be replaced with a fresh worker carrying
        // the new complete profile; otherwise a profile save can permanently
        // shrink capacity below its target.
        self.reconcile(now_millis, ScaleReason::ProfileChanged);
    }

    fn finish_termination(&self, worker_id: WorkerId, now_millis: u64) {
        self.provider.terminate(worker_id);
        let mut state = self.lock_state();
        state.workers.remove(&worker_id);
        state.ready.retain(|candidate| *candidate != worker_id);
        state.events.push(RunnerEvent::lifecycle(
            now_millis,
            RunnerEventKind::CleanupCompleted,
            None,
            Some(worker_id),
            None,
            Some(WorkerKind::Warm),
        ));
        drop(state);
        self.changed.notify_all();
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, PoolState> {
        self.state.lock().expect("worker pool state poisoned")
    }
}

fn pop_current_ready(state: &mut PoolState) -> Option<WorkerId> {
    while let Some(worker_id) = state.ready.pop_front() {
        if state.workers.get(&worker_id).is_some_and(|record| {
            record.state == WorkerState::Ready
                && record.profile_version == state.desired_profile.version
        }) {
            return Some(worker_id);
        }
    }
    None
}

fn grant_lease(
    state: &mut PoolState,
    worker_id: WorkerId,
    run_id: RunId,
    _now_millis: u64,
) -> WorkerLease {
    let record = state.workers.get_mut(&worker_id).expect("known worker");
    record.state = WorkerState::Leased;
    let lease = WorkerLease {
        lease_id: WorkerLeaseId::new(),
        worker_id,
        run_id,
        worker_kind: record.kind,
        profile_version: record.profile_version,
    };
    state.run_leases.insert(run_id, lease.lease_id);
    state.leases.insert(lease.lease_id, lease.clone());
    lease
}

fn counts(state: &PoolState) -> PoolCounts {
    state.workers.values().fold(
        PoolCounts {
            starting_warm: 0,
            ready_warm: 0,
            leased_warm: 0,
            leased_on_demand: 0,
        },
        |mut counts, record| {
            match (record.kind, record.state) {
                (WorkerKind::Warm, WorkerState::Starting) => counts.starting_warm += 1,
                (WorkerKind::Warm, WorkerState::Ready) => counts.ready_warm += 1,
                (WorkerKind::Warm, WorkerState::Leased) => counts.leased_warm += 1,
                (WorkerKind::OnDemand, WorkerState::Leased) => counts.leased_on_demand += 1,
                _ => {}
            }
            counts
        },
    )
}

fn scale_event(
    now_millis: u64,
    decision: AutoscaleDecision,
    reason: ScaleReason,
    counts: PoolCounts,
    provisioned: u32,
    terminated_ready: u32,
) -> RunnerEvent {
    let mut event = RunnerEvent::lifecycle(
        now_millis,
        RunnerEventKind::ScaleEvaluated,
        None,
        None,
        None,
        None,
    );
    event.scale = Some(ScaleTelemetry {
        reason,
        direction: match decision.action {
            ScaleAction::Hold => ScaleDirection::Hold,
            ScaleAction::ScaleOut { .. } => ScaleDirection::ScaleOut,
            ScaleAction::ScaleIn { .. } => ScaleDirection::ScaleIn,
        },
        outcome: match decision.action {
            ScaleAction::Hold => ScaleDirection::Hold,
            ScaleAction::ScaleOut { .. } => ScaleDirection::ScaleOut,
            ScaleAction::ScaleIn { .. } => ScaleDirection::ScaleIn,
        },
        active_demand: decision.active_demand,
        peak_5m: decision.peak_5m,
        base_capacity: decision.base_capacity,
        current_warm_capacity: decision.current_warm_capacity,
        starting_warm: counts.starting_warm,
        ready_warm: counts.ready_warm,
        leased_warm: counts.leased_warm,
        target_warm_capacity: decision.target_warm_capacity,
        provisioned,
        terminated_ready,
    });
    event
}

trait SanitizedApplyReason {
    fn max_reason(self, supplied: RunnerFailureReason) -> RunnerFailureReason;
}

impl SanitizedApplyReason for RunnerFailureReason {
    fn max_reason(self, _supplied: RunnerFailureReason) -> RunnerFailureReason {
        // A provider may know a detailed failure, but public profile-apply
        // callers must always receive the stable reason code.
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeProvider {
        provisioned: Mutex<Vec<WorkerProvisionRequest>>,
        terminated: Mutex<Vec<WorkerId>>,
        fail_profile_apply: AtomicBool,
    }

    impl WorkerProvider for FakeProvider {
        fn provision(&self, request: WorkerProvisionRequest) -> Result<(), RunnerFailureReason> {
            if request.cancellation.is_cancelled() {
                return Err(RunnerFailureReason::Cancelled);
            }
            self.provisioned.lock().unwrap().push(request);
            Ok(())
        }

        fn terminate(&self, worker_id: WorkerId) {
            self.terminated.lock().unwrap().push(worker_id);
        }

        fn apply_profile(
            &self,
            _worker_id: WorkerId,
            _profile: &RunnerResourcesProfile,
        ) -> Result<(), RunnerFailureReason> {
            if self.fail_profile_apply.load(Ordering::Acquire) {
                Err(RunnerFailureReason::ConfigurationApplyFailed)
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn starts_base_three_and_leases_ready_workers_exclusively() {
        let provider = Arc::new(FakeProvider::default());
        let pool = WorkerPoolControl::new(provider, RunnerResourcesProfile::default(), 0).unwrap();
        assert!(pool.wait_for_ready_count(3, Duration::from_secs(2)));
        let first = pool
            .acquire(
                AcquireRequest::new(RunId::new(), 1, RunnerResourcesProfile::default()),
                1,
            )
            .unwrap();
        let second = pool
            .acquire(
                AcquireRequest::new(RunId::new(), 1, RunnerResourcesProfile::default()),
                1,
            )
            .unwrap();
        assert_eq!(first.worker_kind, WorkerKind::Warm);
        assert_eq!(second.worker_kind, WorkerKind::Warm);
        assert_ne!(first.worker_id, second.worker_id);
    }

    #[test]
    fn missing_ready_worker_is_provisioned_on_demand_and_not_warm_capacity() {
        let provider = Arc::new(FakeProvider::default());
        let pool = WorkerPoolControl::new(provider, RunnerResourcesProfile::default(), 0).unwrap();
        assert!(pool.wait_for_ready_count(3, Duration::from_secs(2)));
        for _ in 0..3 {
            pool.acquire(
                AcquireRequest::new(RunId::new(), 1, RunnerResourcesProfile::default()),
                1,
            )
            .unwrap();
        }
        let direct = pool
            .acquire(
                AcquireRequest::new(RunId::new(), 1, RunnerResourcesProfile::default()),
                1,
            )
            .unwrap();
        assert_eq!(direct.worker_kind, WorkerKind::OnDemand);
        let snapshot = pool.snapshot(1);
        assert_eq!(snapshot.counts.warm_capacity(), 3);
        assert_eq!(snapshot.counts.leased_on_demand, 1);
    }

    #[test]
    fn failed_idle_profile_apply_replaces_workers_to_restore_target_capacity() {
        let provider = Arc::new(FakeProvider::default());
        let pool = WorkerPoolControl::new(provider.clone(), RunnerResourcesProfile::default(), 0).unwrap();
        assert!(pool.wait_for_ready_count(3, Duration::from_secs(2)));
        provider.fail_profile_apply.store(true, Ordering::Release);
        let profile = RunnerResourcesProfile {
            version: 2,
            ..RunnerResourcesProfile::default()
        };

        pool.set_desired_profile(profile, 1).unwrap();

        assert!(pool.wait_for_ready_count(3, Duration::from_secs(2)));
        assert!(provider.terminated.lock().unwrap().len() >= 3);
        assert!(provider.provisioned.lock().unwrap().len() >= 6);
    }

    #[test]
    fn a_peak_of_one_hundred_targets_one_hundred_twenty_warm_workers() {
        let provider = Arc::new(FakeProvider::default());
        let pool = WorkerPoolControl::new(provider, RunnerResourcesProfile::default(), 0).unwrap();
        assert!(pool.wait_for_ready_count(3, Duration::from_secs(2)));
        let mut leases = Vec::new();
        for _ in 0..100 {
            leases.push(
                pool.acquire(
                    AcquireRequest::new(RunId::new(), 1, RunnerResourcesProfile::default()),
                    1,
                )
                .unwrap(),
            );
        }
        pool.autoscale_tick(5_000);
        assert!(pool.wait_for_ready_count(117, Duration::from_secs(3)));
        let snapshot = pool.snapshot(5_000);
        assert_eq!(snapshot.peak_5m, 100);
        assert_eq!(snapshot.target_warm_capacity, 120);
        assert_eq!(snapshot.counts.warm_capacity(), 120);
        let scale = pool
            .events()
            .into_iter()
            .rev()
            .find(|event| event.kind == RunnerEventKind::ScaleEvaluated)
            .and_then(|event| event.scale)
            .expect("autoscale event");
        assert_eq!(scale.direction, ScaleDirection::ScaleOut);
        assert_eq!(scale.reason, ScaleReason::PeriodicTick);
        assert_eq!(scale.outcome, ScaleDirection::ScaleOut);
        assert_eq!(scale.current_warm_capacity, 3);
        assert_eq!(scale.target_warm_capacity, 120);
        drop(leases);
    }

    #[test]
    fn rejects_profile_changes_without_a_new_generation() {
        let provider = Arc::new(FakeProvider::default());
        let pool = WorkerPoolControl::new(provider, RunnerResourcesProfile::default(), 0).unwrap();
        let mut changed_without_new_version = RunnerResourcesProfile::default();
        changed_without_new_version.base_capacity = 4;

        assert_eq!(
            pool.set_desired_profile(changed_without_new_version, 1),
            Err(PoolError::InvalidProfile)
        );
        assert_eq!(pool.snapshot(1).profile_version, 1);
    }
}
