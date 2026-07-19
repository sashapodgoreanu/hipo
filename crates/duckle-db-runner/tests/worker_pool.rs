use duckle_db_runner::model::{
    AcquireRequest, RunCancellation, RunId, RunnerFailureReason, WorkerId, WorkerKind,
};
use duckle_db_runner::resources::RunnerResourcesProfile;
use duckle_db_runner::worker_pool::{
    PoolError, WorkerPoolControl, WorkerProvider, WorkerProvisionRequest,
};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Default)]
struct WarmGate {
    state: Mutex<WarmGateState>,
    changed: Condvar,
}

#[derive(Default)]
struct WarmGateState {
    blocked: bool,
    warm_started: u32,
}

#[derive(Default)]
struct OnDemandGate {
    started: Mutex<bool>,
    changed: Condvar,
}

impl OnDemandGate {
    fn wait_for_start(&self) {
        let mut started = self.started.lock().unwrap();
        while !*started {
            started = self.changed.wait(started).unwrap();
        }
    }

    fn wait_for_cancellation(&self, cancellation: &RunCancellation) {
        let mut started = self.started.lock().unwrap();
        *started = true;
        self.changed.notify_all();
        while !cancellation.is_cancelled() {
            let (next, _) = self
                .changed
                .wait_timeout(started, Duration::from_millis(10))
                .unwrap();
            started = next;
        }
    }
}

impl WarmGate {
    fn block(&self) {
        self.state.lock().unwrap().blocked = true;
    }

    fn wait_for_warm_starts(&self, expected: u32) {
        let mut state = self.state.lock().unwrap();
        while state.warm_started < expected {
            state = self.changed.wait(state).unwrap();
        }
    }

    fn open(&self) {
        self.state.lock().unwrap().blocked = false;
        self.changed.notify_all();
    }
}

struct BlockingProvider {
    gate: Arc<WarmGate>,
    on_demand_gate: Option<Arc<OnDemandGate>>,
    provisioned: Mutex<Vec<WorkerKind>>,
    applied: Mutex<Vec<(WorkerId, u64)>>,
    terminated: Mutex<Vec<WorkerId>>,
}

impl BlockingProvider {
    fn new(gate: Arc<WarmGate>) -> Self {
        Self {
            gate,
            on_demand_gate: None,
            provisioned: Mutex::new(Vec::new()),
            applied: Mutex::new(Vec::new()),
            terminated: Mutex::new(Vec::new()),
        }
    }

    fn with_on_demand_gate(gate: Arc<WarmGate>, on_demand_gate: Arc<OnDemandGate>) -> Self {
        Self {
            gate,
            on_demand_gate: Some(on_demand_gate),
            provisioned: Mutex::new(Vec::new()),
            applied: Mutex::new(Vec::new()),
            terminated: Mutex::new(Vec::new()),
        }
    }
}

impl WorkerProvider for BlockingProvider {
    fn provision(&self, request: WorkerProvisionRequest) -> Result<(), RunnerFailureReason> {
        self.provisioned.lock().unwrap().push(request.kind);
        if request.kind == WorkerKind::Warm {
            let mut state = self.gate.state.lock().unwrap();
            state.warm_started += 1;
            self.gate.changed.notify_all();
            while state.blocked && !request.cancellation.is_cancelled() {
                state = self.gate.changed.wait(state).unwrap();
            }
        }
        if request.kind == WorkerKind::OnDemand {
            if let Some(gate) = &self.on_demand_gate {
                gate.wait_for_cancellation(&request.cancellation);
            }
        }
        if request.cancellation.is_cancelled() {
            return Err(RunnerFailureReason::Cancelled);
        }
        Ok(())
    }

    fn apply_profile(
        &self,
        worker_id: WorkerId,
        profile: &RunnerResourcesProfile,
    ) -> Result<(), RunnerFailureReason> {
        self.applied
            .lock()
            .unwrap()
            .push((worker_id, profile.version));
        Ok(())
    }

    fn terminate(&self, worker_id: WorkerId) {
        self.terminated.lock().unwrap().push(worker_id);
    }
}

#[test]
fn one_hundred_concurrent_runs_get_direct_workers_then_grow_warm_capacity() {
    let gate = Arc::new(WarmGate::default());
    gate.block();
    let provider = Arc::new(BlockingProvider::new(gate.clone()));
    let profile = RunnerResourcesProfile::default();
    let pool = WorkerPoolControl::new(provider.clone(), profile.clone(), 0).unwrap();
    gate.wait_for_warm_starts(3);

    let (sender, receiver) = std::sync::mpsc::channel();
    thread::scope(|scope| {
        for _ in 0..100 {
            let pool = pool.clone();
            let profile = profile.clone();
            let sender = sender.clone();
            scope.spawn(move || {
                let result = pool.acquire(AcquireRequest::new(RunId::new(), 1, profile), 1);
                sender.send(result).unwrap();
            });
        }
    });
    drop(sender);
    let first_wave = receiver
        .into_iter()
        .map(|result| result.unwrap())
        .collect::<Vec<_>>();
    assert_eq!(first_wave.len(), 100);
    assert!(first_wave
        .iter()
        .all(|lease| lease.worker_kind == WorkerKind::OnDemand));

    pool.autoscale_tick(5_000);
    gate.wait_for_warm_starts(120);
    gate.open();
    assert!(pool.wait_for_ready_count(120, Duration::from_secs(5)));
    let after_burst = pool.snapshot(5_000);
    assert_eq!(after_burst.peak_5m, 100);
    assert_eq!(after_burst.target_warm_capacity, 120);
    assert_eq!(after_burst.counts.warm_capacity(), 120);
    assert_eq!(after_burst.counts.leased_on_demand, 100);

    for lease in first_wave {
        pool.release(lease, 6_000).unwrap();
    }
    let after_direct_cleanup = pool.snapshot(6_000);
    assert_eq!(after_direct_cleanup.counts.leased_on_demand, 0);
    assert_eq!(after_direct_cleanup.counts.warm_capacity(), 120);

    let second_wave = (0..100)
        .map(|_| {
            pool.acquire(AcquireRequest::new(RunId::new(), 2, profile.clone()), 7_000)
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert!(second_wave
        .iter()
        .all(|lease| lease.worker_kind == WorkerKind::Warm));
    let after_second_wave = pool.snapshot(7_000);
    assert_eq!(after_second_wave.counts.ready_warm, 20);
    assert_eq!(after_second_wave.counts.leased_on_demand, 0);
}

#[test]
fn cancellation_during_on_demand_bootstrap_terminates_the_worker() {
    let gate = Arc::new(WarmGate::default());
    let on_demand_gate = Arc::new(OnDemandGate::default());
    let provider = Arc::new(BlockingProvider::with_on_demand_gate(
        gate,
        on_demand_gate.clone(),
    ));
    let pool =
        WorkerPoolControl::new(provider.clone(), RunnerResourcesProfile::default(), 0).unwrap();
    assert!(pool.wait_for_ready_count(3, Duration::from_secs(2)));
    for _ in 0..3 {
        let _ = pool
            .acquire(
                AcquireRequest::new(RunId::new(), 1, RunnerResourcesProfile::default()),
                1,
            )
            .unwrap();
    }

    let run_id = RunId::new();
    let cancellation = RunCancellation::default();
    let mut request = AcquireRequest::new(run_id, 1, RunnerResourcesProfile::default());
    request.cancellation = cancellation;
    let acquire_pool = pool.clone();
    let (sender, receiver) = std::sync::mpsc::channel();
    thread::spawn(move || sender.send(acquire_pool.acquire(request, 2)).unwrap());
    on_demand_gate.wait_for_start();

    pool.cancel_run(run_id, 3).unwrap();
    assert!(receiver
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .is_err());
    assert!(!provider.terminated.lock().unwrap().is_empty());
}

struct RetryingProvider {
    attempts: AtomicU32,
    failures_remaining: AtomicU32,
    changed: Condvar,
    lock: Mutex<()>,
    terminated: Mutex<Vec<WorkerId>>,
}

impl RetryingProvider {
    fn fail_first(count: u32) -> Self {
        Self {
            attempts: AtomicU32::new(0),
            failures_remaining: AtomicU32::new(count),
            changed: Condvar::new(),
            lock: Mutex::new(()),
            terminated: Mutex::new(Vec::new()),
        }
    }

    fn wait_for_attempts(&self, expected: u32) {
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut lock = self.lock.lock().unwrap();
        while self.attempts.load(Ordering::Acquire) < expected {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "timed out waiting for {expected} starts"
            );
            let (next, result) = self.changed.wait_timeout(lock, remaining).unwrap();
            lock = next;
            assert!(
                !result.timed_out(),
                "timed out waiting for {expected} starts"
            );
        }
    }
}

impl WorkerProvider for RetryingProvider {
    fn provision(&self, _request: WorkerProvisionRequest) -> Result<(), RunnerFailureReason> {
        self.attempts.fetch_add(1, Ordering::AcqRel);
        self.changed.notify_all();
        if self
            .failures_remaining
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                (remaining > 0).then_some(remaining - 1)
            })
            .is_ok()
        {
            Err(RunnerFailureReason::RunnerUnavailable)
        } else {
            Ok(())
        }
    }

    fn terminate(&self, worker_id: WorkerId) {
        self.terminated.lock().unwrap().push(worker_id);
    }
}

#[test]
fn failed_readiness_retries_only_on_the_next_five_second_evaluation() {
    let provider = Arc::new(RetryingProvider::fail_first(3));
    let pool =
        WorkerPoolControl::new(provider.clone(), RunnerResourcesProfile::default(), 0).unwrap();
    provider.wait_for_attempts(3);
    let deadline = Instant::now() + Duration::from_secs(2);
    while pool.snapshot(1).counts.warm_capacity() != 0 {
        assert!(
            Instant::now() < deadline,
            "failed workers did not leave warm capacity"
        );
        thread::yield_now();
    }

    pool.autoscale_tick(4_999);
    assert_eq!(provider.attempts.load(Ordering::Acquire), 3);
    pool.autoscale_tick(5_000);
    provider.wait_for_attempts(6);
    assert!(pool.wait_for_ready_count(3, Duration::from_secs(2)));
}

#[test]
fn starting_capacity_prevents_duplicate_warm_starts() {
    let gate = Arc::new(WarmGate::default());
    gate.block();
    let provider = Arc::new(BlockingProvider::new(gate.clone()));
    let pool =
        WorkerPoolControl::new(provider.clone(), RunnerResourcesProfile::default(), 0).unwrap();
    gate.wait_for_warm_starts(3);

    pool.autoscale_tick(1_000);
    pool.autoscale_tick(5_000);
    pool.autoscale_tick(10_000);
    assert_eq!(provider.provisioned.lock().unwrap().len(), 3);

    gate.open();
    assert!(pool.wait_for_ready_count(3, Duration::from_secs(2)));
}

#[test]
fn starting_workers_apply_only_the_latest_generation_before_publication() {
    let gate = Arc::new(WarmGate::default());
    gate.block();
    let provider = Arc::new(BlockingProvider::new(gate.clone()));
    let pool =
        WorkerPoolControl::new(provider.clone(), RunnerResourcesProfile::default(), 0).unwrap();
    gate.wait_for_warm_starts(3);

    let mut newer = RunnerResourcesProfile::default();
    newer.version = 2;
    newer.cpu_threads = duckle_db_runner::resources::AutomaticOrU16::Value(2);
    pool.set_desired_profile(newer, 1).unwrap();
    assert_eq!(pool.snapshot(1).counts.ready_warm, 0);
    gate.open();

    assert!(pool.wait_for_ready_count(3, Duration::from_secs(2)));
    let applied = provider.applied.lock().unwrap();
    assert_eq!(applied.len(), 3);
    assert!(applied.iter().all(|(_, version)| *version == 2));
    assert!(provider.terminated.lock().unwrap().is_empty());
    assert_eq!(pool.snapshot(2).profile_version, 2);
}

#[test]
fn leases_are_exclusive_single_use_and_duplicate_runs_are_rejected() {
    let provider = Arc::new(RetryingProvider::fail_first(0));
    let profile = RunnerResourcesProfile::default();
    let pool = WorkerPoolControl::new(provider, profile.clone(), 0).unwrap();
    assert!(pool.wait_for_ready_count(3, Duration::from_secs(2)));
    let run_id = RunId::new();
    let lease = pool
        .acquire(AcquireRequest::new(run_id, 1, profile.clone()), 1)
        .unwrap();

    assert!(pool
        .acquire(AcquireRequest::new(run_id, 2, profile), 2)
        .is_err());
    pool.release(lease.clone(), 3).unwrap();
    assert!(pool.release(lease, 4).is_err());
}

#[test]
fn shutdown_wins_over_starting_publication_and_future_acquire() {
    let gate = Arc::new(WarmGate::default());
    gate.block();
    let provider = Arc::new(BlockingProvider::new(gate.clone()));
    let pool =
        WorkerPoolControl::new(provider.clone(), RunnerResourcesProfile::default(), 0).unwrap();
    gate.wait_for_warm_starts(3);

    pool.shutdown(1);
    gate.open();
    let deadline = Instant::now() + Duration::from_secs(2);
    while provider.terminated.lock().unwrap().len() < 3 {
        assert!(
            Instant::now() < deadline,
            "shutdown did not terminate every starting worker"
        );
        thread::yield_now();
    }
    assert_eq!(pool.snapshot(2).counts.warm_capacity(), 0);
    assert!(pool
        .acquire(
            AcquireRequest::new(RunId::new(), 1, RunnerResourcesProfile::default()),
            2,
        )
        .is_err());
}

#[test]
fn scale_in_terminates_ready_workers_without_interrupting_a_lease() {
    let provider = Arc::new(RetryingProvider::fail_first(0));
    let profile = RunnerResourcesProfile::default();
    let pool = WorkerPoolControl::new(provider.clone(), profile.clone(), 0).unwrap();
    assert!(pool.wait_for_ready_count(3, Duration::from_secs(2)));
    let lease = pool
        .acquire(AcquireRequest::new(RunId::new(), 1, profile), 1)
        .unwrap();

    let mut smaller = RunnerResourcesProfile::default();
    smaller.version = 2;
    smaller.base_capacity = 1;
    pool.set_desired_profile(smaller, 2).unwrap();
    assert!(pool.wait_for_ready_count(1, Duration::from_secs(2)));

    let snapshot = pool.snapshot(5_003);
    assert_eq!(snapshot.counts.leased_warm, 1);
    assert_eq!(snapshot.counts.ready_warm, 1);
    assert!(!provider
        .terminated
        .lock()
        .unwrap()
        .contains(&lease.worker_id));
    pool.release(lease, 5_004).unwrap();
}

#[derive(Default)]
struct ApplyGate {
    state: Mutex<ApplyGateState>,
    changed: Condvar,
}

#[derive(Default)]
struct ApplyGateState {
    blocked: bool,
    calls: Vec<(WorkerId, u64)>,
}

impl ApplyGate {
    fn block(&self) {
        self.state.lock().unwrap().blocked = true;
    }

    fn wait_for_calls(&self, expected: usize) {
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut state = self.state.lock().unwrap();
        while state.calls.len() < expected {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(!remaining.is_zero(), "timed out waiting for profile apply");
            let (next, result) = self.changed.wait_timeout(state, remaining).unwrap();
            state = next;
            assert!(!result.timed_out(), "timed out waiting for profile apply");
        }
    }

    fn open(&self) {
        self.state.lock().unwrap().blocked = false;
        self.changed.notify_all();
    }
}

struct ApplyGateProvider {
    gate: Arc<ApplyGate>,
    terminated: Mutex<Vec<WorkerId>>,
}

impl WorkerProvider for ApplyGateProvider {
    fn provision(&self, _request: WorkerProvisionRequest) -> Result<(), RunnerFailureReason> {
        Ok(())
    }

    fn apply_profile(
        &self,
        worker_id: WorkerId,
        profile: &RunnerResourcesProfile,
    ) -> Result<(), RunnerFailureReason> {
        let mut state = self.gate.state.lock().unwrap();
        state.calls.push((worker_id, profile.version));
        self.gate.changed.notify_all();
        while state.blocked {
            state = self.gate.changed.wait(state).unwrap();
        }
        Ok(())
    }

    fn terminate(&self, worker_id: WorkerId) {
        self.terminated.lock().unwrap().push(worker_id);
    }
}

#[test]
fn concurrent_profile_saves_coalesce_to_the_latest_generation() {
    let gate = Arc::new(ApplyGate::default());
    let provider = Arc::new(ApplyGateProvider {
        gate: gate.clone(),
        terminated: Mutex::new(Vec::new()),
    });
    let pool =
        WorkerPoolControl::new(provider.clone(), RunnerResourcesProfile::default(), 0).unwrap();
    assert!(pool.wait_for_ready_count(3, Duration::from_secs(2)));
    gate.block();

    let mut second = RunnerResourcesProfile::default();
    second.version = 2;
    second.quack_parallelism = duckle_db_runner::resources::AutomaticOrU16::Value(2);
    pool.set_desired_profile(second, 1).unwrap();
    gate.wait_for_calls(3);
    assert_eq!(pool.snapshot(1).counts.ready_warm, 0);

    let mut third = RunnerResourcesProfile::default();
    third.version = 3;
    third.quack_parallelism = duckle_db_runner::resources::AutomaticOrU16::Value(4);
    pool.set_desired_profile(third, 2).unwrap();
    gate.open();

    gate.wait_for_calls(6);
    assert!(pool.wait_for_ready_count(3, Duration::from_secs(2)));
    let calls = gate.state.lock().unwrap().calls.clone();
    assert_eq!(calls.iter().filter(|(_, version)| *version == 2).count(), 3);
    assert_eq!(calls.iter().filter(|(_, version)| *version == 3).count(), 3);
    assert_eq!(pool.snapshot(3).profile_version, 3);
    assert!(provider.terminated.lock().unwrap().is_empty());
}

struct CrashOnCancelledProvision {
    gate: Arc<OnDemandGate>,
    terminated: Mutex<Vec<WorkerId>>,
}

impl WorkerProvider for CrashOnCancelledProvision {
    fn provision(&self, request: WorkerProvisionRequest) -> Result<(), RunnerFailureReason> {
        if request.kind == WorkerKind::OnDemand {
            self.gate.wait_for_cancellation(&request.cancellation);
            return Err(RunnerFailureReason::RunnerCrashed);
        }
        Ok(())
    }

    fn terminate(&self, worker_id: WorkerId) {
        self.terminated.lock().unwrap().push(worker_id);
    }
}

#[test]
fn cancellation_wins_when_transport_loss_races_provisioning() {
    let gate = Arc::new(OnDemandGate::default());
    let provider = Arc::new(CrashOnCancelledProvision {
        gate: gate.clone(),
        terminated: Mutex::new(Vec::new()),
    });
    let profile = RunnerResourcesProfile::default();
    let pool = WorkerPoolControl::new(provider.clone(), profile.clone(), 0).unwrap();
    assert!(pool.wait_for_ready_count(3, Duration::from_secs(2)));
    for _ in 0..3 {
        pool.acquire(AcquireRequest::new(RunId::new(), 1, profile.clone()), 1)
            .unwrap();
    }
    let run_id = RunId::new();
    let acquire_pool = pool.clone();
    let (sender, receiver) = std::sync::mpsc::channel();
    thread::spawn(move || {
        sender
            .send(acquire_pool.acquire(AcquireRequest::new(run_id, 1, profile), 2))
            .unwrap();
    });
    gate.wait_for_start();

    pool.cancel_run(run_id, 3).unwrap();

    assert_eq!(
        receiver.recv_timeout(Duration::from_secs(2)).unwrap(),
        Err(PoolError::Cancelled)
    );
    assert!(!provider.terminated.lock().unwrap().is_empty());
}
