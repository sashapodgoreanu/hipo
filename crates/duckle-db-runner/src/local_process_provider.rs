//! Provider boundary for a locally managed sidecar process.
//!
//! `WorkerPoolControl` sees only this `WorkerProvider`; the launcher and its
//! managed process remain private to the provider. The launcher receives an
//! already-resolved profile and a one-shot bootstrap message, so readiness may
//! be reported only after the effective limits and authenticated handshake are
//! in place.

use crate::bootstrap::{AuthenticatedReadiness, BootstrapMessage};
use crate::model::{RunnerFailureReason, WorkerId, WorkerKind};
use crate::resources::{HostResourceLimits, ResolvedRunnerResources};
use crate::run_database::RunDatabase;
use crate::worker_pool::{WorkerProvider, WorkerProvisionRequest};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Provider-private launch input. It has no endpoint, PID, filesystem path,
/// readiness file, or serializable credential field.
pub struct LocalSidecarLaunch {
    pub worker_id: WorkerId,
    pub worker_kind: WorkerKind,
    pub effective_profile: ResolvedRunnerResources,
    pub cancellation: crate::model::RunCancellation,
    pub bootstrap: BootstrapMessage,
}

/// A launch succeeds only after the implementation has started the sidecar,
/// applied the effective profile, received its control message, and completed
/// its authenticated Quack handshake. Implementations must retain endpoint and
/// credential state in their private managed handle.
pub trait LocalSidecarLauncher: Send + Sync + 'static {
    fn launch(
        &self,
        request: LocalSidecarLaunch,
    ) -> Result<LaunchedLocalSidecar, RunnerFailureReason>;
}

/// A process is not publishable until its child-to-parent control response has
/// been authenticated. The endpoint remains inside `AuthenticatedReadiness`
/// and is inaccessible to pool, runtime, IPC, or user code.
pub struct LaunchedLocalSidecar {
    managed: Box<dyn ManagedSidecar>,
    readiness: AuthenticatedReadiness,
}

impl LaunchedLocalSidecar {
    /// Builds the provider-private launch result. Launcher implementations can
    /// return their managed process and authenticated handshake, but callers
    /// cannot inspect either after the provider takes ownership.
    pub fn new(managed: Box<dyn ManagedSidecar>, readiness: AuthenticatedReadiness) -> Self {
        Self { managed, readiness }
    }
}

/// Opaque managed process owned solely by `LocalProcessProvider`.
pub trait ManagedSidecar: Send + 'static {
    fn apply_effective_profile(
        &mut self,
        profile: &ResolvedRunnerResources,
    ) -> Result<(), RunnerFailureReason>;

    /// Returns a new controlled database facade bound to this managed
    /// sidecar. Implementors retain all endpoint and credential state.
    fn open_database(
        &mut self,
        cancellation: crate::model::RunCancellation,
    ) -> Result<RunDatabase, RunnerFailureReason>;

    fn terminate(self: Box<Self>);
}

/// The local provider resolves host-dependent limits before it asks a launcher
/// to provision. A concrete desktop launcher supplies the process and Quack
/// details without exposing them to pool callers.
pub struct LocalProcessProvider {
    launcher: Arc<dyn LocalSidecarLauncher>,
    host_limits: HostResourceLimits,
    workers: Mutex<HashMap<WorkerId, Box<dyn ManagedSidecar>>>,
}

impl LocalProcessProvider {
    pub fn new(launcher: Arc<dyn LocalSidecarLauncher>, host_limits: HostResourceLimits) -> Self {
        Self {
            launcher,
            host_limits,
            workers: Mutex::new(HashMap::new()),
        }
    }

    pub fn active_workers(&self) -> usize {
        self.workers
            .lock()
            .expect("local sidecar state poisoned")
            .len()
    }

    fn resolve(
        &self,
        profile: &crate::resources::RunnerResourcesProfile,
    ) -> Result<ResolvedRunnerResources, RunnerFailureReason> {
        profile
            .resolve(self.host_limits)
            .map_err(|_| RunnerFailureReason::InvalidProfile)
    }
}

impl WorkerProvider for LocalProcessProvider {
    fn provision(&self, request: WorkerProvisionRequest) -> Result<(), RunnerFailureReason> {
        if request.cancellation.is_cancelled() {
            return Err(RunnerFailureReason::Cancelled);
        }
        if self
            .workers
            .lock()
            .expect("local sidecar state poisoned")
            .contains_key(&request.worker_id)
        {
            return Err(RunnerFailureReason::RunnerUnavailable);
        }
        let effective_profile = self.resolve(&request.profile)?;
        let bootstrap = BootstrapMessage::new(request.worker_id, effective_profile.clone())
            .map_err(|_| RunnerFailureReason::RunnerUnavailable)?;
        let launched = self.launcher.launch(LocalSidecarLaunch {
            worker_id: request.worker_id,
            worker_kind: request.kind,
            effective_profile: effective_profile.clone(),
            cancellation: request.cancellation.clone(),
            bootstrap,
        })?;
        if launched.readiness.worker_id() != request.worker_id
            || launched.readiness.effective_profile() != &effective_profile
            || launched.readiness.runner_protocol_version() != crate::RUNNER_PROTOCOL_VERSION
            || !launched.readiness.endpoint().ip().is_loopback()
        {
            launched.managed.terminate();
            return Err(RunnerFailureReason::RunnerVersionMismatch);
        }
        let managed = launched.managed;
        if request.cancellation.is_cancelled() {
            managed.terminate();
            return Err(RunnerFailureReason::Cancelled);
        }
        let mut workers = self.workers.lock().expect("local sidecar state poisoned");
        if workers.contains_key(&request.worker_id) {
            drop(workers);
            managed.terminate();
            return Err(RunnerFailureReason::RunnerUnavailable);
        }
        workers.insert(request.worker_id, managed);
        Ok(())
    }

    fn apply_profile(
        &self,
        worker_id: WorkerId,
        profile: &crate::resources::RunnerResourcesProfile,
    ) -> Result<(), RunnerFailureReason> {
        let effective_profile = self.resolve(profile)?;
        let mut workers = self.workers.lock().expect("local sidecar state poisoned");
        let worker = workers
            .get_mut(&worker_id)
            .ok_or(RunnerFailureReason::RunnerUnavailable)?;
        worker.apply_effective_profile(&effective_profile)
    }

    fn open_database(
        &self,
        worker_id: WorkerId,
        cancellation: crate::model::RunCancellation,
    ) -> Result<RunDatabase, RunnerFailureReason> {
        let mut workers = self.workers.lock().expect("local sidecar state poisoned");
        let worker = workers
            .get_mut(&worker_id)
            .ok_or(RunnerFailureReason::RunnerUnavailable)?;
        worker.open_database(cancellation)
    }

    fn terminate(&self, worker_id: WorkerId) {
        if let Some(worker) = self
            .workers
            .lock()
            .expect("local sidecar state poisoned")
            .remove(&worker_id)
        {
            worker.terminate();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::RunCancellation;
    use crate::resources::{AutomaticOrU16, ResourceLimit, RunnerResourcesProfile};
    use std::io::Cursor;

    #[derive(Default)]
    struct State {
        launched: Vec<(WorkerId, ResolvedRunnerResources)>,
        applied: Vec<(WorkerId, ResolvedRunnerResources)>,
        terminated: Vec<WorkerId>,
    }

    struct FakeLauncher {
        state: Arc<Mutex<State>>,
    }

    impl LocalSidecarLauncher for FakeLauncher {
        fn launch(
            &self,
            request: LocalSidecarLaunch,
        ) -> Result<LaunchedLocalSidecar, RunnerFailureReason> {
            assert_eq!(request.bootstrap.worker_id(), request.worker_id);
            assert_eq!(
                request.bootstrap.effective_profile(),
                &request.effective_profile
            );
            let mut control = Vec::new();
            crate::bootstrap::write_authenticated_readiness(
                &mut control,
                &request.bootstrap,
                "127.0.0.1:43123".parse().unwrap(),
            )
            .unwrap();
            let readiness = crate::bootstrap::read_authenticated_readiness(
                &mut Cursor::new(control),
                &request.bootstrap,
            )
            .unwrap();
            self.state
                .lock()
                .unwrap()
                .launched
                .push((request.worker_id, request.effective_profile.clone()));
            Ok(LaunchedLocalSidecar::new(
                Box::new(FakeSidecar {
                    worker_id: request.worker_id,
                    state: self.state.clone(),
                }),
                readiness,
            ))
        }
    }

    struct FakeSidecar {
        worker_id: WorkerId,
        state: Arc<Mutex<State>>,
    }

    impl ManagedSidecar for FakeSidecar {
        fn apply_effective_profile(
            &mut self,
            profile: &ResolvedRunnerResources,
        ) -> Result<(), RunnerFailureReason> {
            self.state
                .lock()
                .unwrap()
                .applied
                .push((self.worker_id, profile.clone()));
            Ok(())
        }

        fn open_database(
            &mut self,
            _cancellation: crate::model::RunCancellation,
        ) -> Result<RunDatabase, RunnerFailureReason> {
            Err(RunnerFailureReason::RunnerUnavailable)
        }

        fn terminate(self: Box<Self>) {
            self.state.lock().unwrap().terminated.push(self.worker_id);
        }
    }

    #[test]
    fn resolves_the_profile_before_launch_and_keeps_process_details_private() {
        let state = Arc::new(Mutex::new(State::default()));
        let provider = LocalProcessProvider::new(
            Arc::new(FakeLauncher {
                state: state.clone(),
            }),
            HostResourceLimits {
                memory_bytes: Some(1_000),
                memory_cap_bytes: Some(600),
                cpu_threads: Some(16),
                cpu_thread_cap: Some(8),
                ..HostResourceLimits::default()
            },
        );
        let worker_id = WorkerId::new();
        let profile = RunnerResourcesProfile {
            memory: ResourceLimit::Percent(80),
            cpu_threads: AutomaticOrU16::Value(12),
            ..RunnerResourcesProfile::default()
        };

        provider
            .provision(WorkerProvisionRequest {
                worker_id,
                kind: WorkerKind::Warm,
                profile: profile.clone(),
                cancellation: RunCancellation::default(),
            })
            .unwrap();
        {
            let state = state.lock().unwrap();
            assert_eq!(state.launched.len(), 1);
            assert_eq!(state.launched[0].1.memory_bytes, Some(600));
            assert_eq!(state.launched[0].1.cpu_threads, Some(8));
        }
        assert_eq!(provider.active_workers(), 1);

        provider.apply_profile(worker_id, &profile).unwrap();
        provider.terminate(worker_id);
        let state = state.lock().unwrap();
        assert_eq!(state.applied.len(), 1);
        assert_eq!(state.terminated, vec![worker_id]);
    }

    #[test]
    fn cancelled_requests_do_not_reach_the_launcher() {
        let state = Arc::new(Mutex::new(State::default()));
        let provider = LocalProcessProvider::new(
            Arc::new(FakeLauncher {
                state: state.clone(),
            }),
            HostResourceLimits::default(),
        );
        let cancellation = RunCancellation::default();
        cancellation.cancel();

        assert_eq!(
            provider.provision(WorkerProvisionRequest {
                worker_id: WorkerId::new(),
                kind: WorkerKind::OnDemand,
                profile: RunnerResourcesProfile::default(),
                cancellation,
            }),
            Err(RunnerFailureReason::Cancelled)
        );
        assert!(state.lock().unwrap().launched.is_empty());
    }
}
