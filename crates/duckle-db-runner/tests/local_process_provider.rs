use duckle_db_runner::bootstrap::{
    read_authenticated_readiness, write_authenticated_readiness, write_bootstrap, BootstrapMessage,
};
use duckle_db_runner::local_process_provider::{
    LaunchedLocalSidecar, LocalProcessProvider, LocalSidecarLaunch, LocalSidecarLauncher,
    ManagedSidecar,
};
use duckle_db_runner::model::{
    RunCancellation, RunId, RunnerFailureReason, WorkerId, WorkerKind, WorkerLease, WorkerLeaseId,
};
use duckle_db_runner::resources::{
    AutomaticOrU16, HostResourceLimits, ResolvedRunnerResources, ResourceLimit,
    RunnerResourcesProfile,
};
use duckle_db_runner::run_database::RunDatabase;
use duckle_db_runner::worker_pool::{WorkerProvider, WorkerProvisionRequest};
use std::fmt::Write as _;
use std::io::Cursor;
use std::sync::{Arc, Mutex};

#[cfg(windows)]
use duckle_db_runner::local_quack_sidecar::WindowsLocalSidecarLauncher;

#[derive(Clone, Copy)]
enum LaunchMode {
    Healthy,
    MismatchedReadiness,
    CancelAfterReadiness,
}

#[derive(Default)]
struct LaunchState {
    effective_profiles: Vec<ResolvedRunnerResources>,
    verified_profiles: Vec<ResolvedRunnerResources>,
    bootstrap_wires: Vec<Vec<u8>>,
    terminated: Vec<WorkerId>,
}

struct RecordingLauncher {
    mode: LaunchMode,
    state: Arc<Mutex<LaunchState>>,
}

impl LocalSidecarLauncher for RecordingLauncher {
    fn launch(
        &self,
        request: LocalSidecarLaunch,
    ) -> Result<LaunchedLocalSidecar, RunnerFailureReason> {
        assert_eq!(
            request.bootstrap.effective_profile(),
            &request.effective_profile,
            "the effective profile must be fixed before bootstrap/readiness"
        );
        let mut wire = Vec::new();
        write_bootstrap(&mut wire, &request.bootstrap)
            .map_err(|_| RunnerFailureReason::RunnerUnavailable)?;
        self.state.lock().unwrap().bootstrap_wires.push(wire);

        let readiness_bootstrap = match self.mode {
            LaunchMode::MismatchedReadiness => {
                BootstrapMessage::new(WorkerId::new(), request.effective_profile.clone())
                    .map_err(|_| RunnerFailureReason::RunnerUnavailable)?
            }
            LaunchMode::Healthy | LaunchMode::CancelAfterReadiness => request.bootstrap,
        };
        let mut control = Vec::new();
        write_authenticated_readiness(
            &mut control,
            &readiness_bootstrap,
            "127.0.0.1:43123".parse().unwrap(),
        )
        .map_err(|_| RunnerFailureReason::RunnerUnavailable)?;
        let readiness =
            read_authenticated_readiness(&mut Cursor::new(control), &readiness_bootstrap)
                .map_err(|_| RunnerFailureReason::RunnerUnavailable)?;

        self.state
            .lock()
            .unwrap()
            .effective_profiles
            .push(request.effective_profile);
        if matches!(self.mode, LaunchMode::CancelAfterReadiness) {
            request.cancellation.cancel();
        }
        Ok(LaunchedLocalSidecar::new(
            Box::new(RecordingSidecar {
                worker_id: request.worker_id,
                state: self.state.clone(),
            }),
            readiness,
        ))
    }
}

struct RecordingSidecar {
    worker_id: WorkerId,
    state: Arc<Mutex<LaunchState>>,
}

impl ManagedSidecar for RecordingSidecar {
    fn apply_effective_profile(
        &mut self,
        profile: &ResolvedRunnerResources,
    ) -> Result<(), RunnerFailureReason> {
        self.state
            .lock()
            .unwrap()
            .effective_profiles
            .push(profile.clone());
        Ok(())
    }

    fn verify_effective_profile(
        &mut self,
        profile: &ResolvedRunnerResources,
    ) -> Result<(), RunnerFailureReason> {
        self.state
            .lock()
            .unwrap()
            .verified_profiles
            .push(profile.clone());
        Ok(())
    }

    fn open_database(
        &mut self,
        _cancellation: RunCancellation,
    ) -> Result<RunDatabase, RunnerFailureReason> {
        Err(RunnerFailureReason::RunnerUnavailable)
    }

    fn terminate(self: Box<Self>) {
        self.state.lock().unwrap().terminated.push(self.worker_id);
    }
}

fn requested_profile() -> RunnerResourcesProfile {
    RunnerResourcesProfile {
        version: 7,
        memory: ResourceLimit::Percent(75),
        cpu_threads: AutomaticOrU16::Value(6),
        spill: ResourceLimit::Bytes(512 * 1024 * 1024),
        quack_parallelism: AutomaticOrU16::Value(4),
        base_capacity: 3,
    }
}

fn provider(mode: LaunchMode) -> (LocalProcessProvider, Arc<Mutex<LaunchState>>) {
    let state = Arc::new(Mutex::new(LaunchState::default()));
    let provider = LocalProcessProvider::new(
        Arc::new(RecordingLauncher {
            mode,
            state: state.clone(),
        }),
        HostResourceLimits {
            memory_bytes: Some(1_000_000_000),
            memory_cap_bytes: Some(600_000_000),
            cpu_threads: Some(12),
            cpu_thread_cap: Some(4),
            spill_bytes: Some(2_000_000_000),
            spill_cap_bytes: Some(1_000_000_000),
        },
    );
    (provider, state)
}

fn provision_request(worker_id: WorkerId, cancellation: RunCancellation) -> WorkerProvisionRequest {
    WorkerProvisionRequest {
        worker_id,
        kind: WorkerKind::Warm,
        profile: requested_profile(),
        cancellation,
    }
}

#[test]
fn effective_profile_is_resolved_and_verified_before_authenticated_readiness() {
    let (provider, state) = provider(LaunchMode::Healthy);
    let worker_id = WorkerId::new();

    provider
        .provision(provision_request(worker_id, RunCancellation::default()))
        .unwrap();

    let state = state.lock().unwrap();
    assert_eq!(state.effective_profiles.len(), 1);
    assert_eq!(state.verified_profiles.len(), 1);
    let effective = &state.effective_profiles[0];
    assert_eq!(&state.verified_profiles[0], effective);
    assert_eq!(effective.requested_version, 7);
    assert_eq!(effective.effective_version, 7);
    assert_eq!(effective.memory_bytes, Some(600_000_000));
    assert_eq!(effective.cpu_threads, Some(4));
    assert_eq!(effective.spill_bytes, Some(512 * 1024 * 1024));
    assert_eq!(effective.quack_parallelism, 4);
    assert_eq!(provider.active_workers(), 1);
}

#[test]
fn mismatched_readiness_is_rejected_and_the_process_is_terminated() {
    let (provider, state) = provider(LaunchMode::MismatchedReadiness);
    let worker_id = WorkerId::new();

    assert_eq!(
        provider.provision(provision_request(worker_id, RunCancellation::default())),
        Err(RunnerFailureReason::RunnerVersionMismatch)
    );
    assert_eq!(state.lock().unwrap().terminated, vec![worker_id]);
    assert_eq!(provider.active_workers(), 0);
}

#[test]
fn cancellation_after_readiness_terminates_without_publication() {
    let (provider, state) = provider(LaunchMode::CancelAfterReadiness);
    let worker_id = WorkerId::new();

    assert_eq!(
        provider.provision(provision_request(worker_id, RunCancellation::default())),
        Err(RunnerFailureReason::Cancelled)
    );
    assert_eq!(state.lock().unwrap().terminated, vec![worker_id]);
    assert_eq!(provider.active_workers(), 0);
}

#[test]
fn credentials_are_unique_and_remain_only_in_the_inherited_bootstrap_payload() {
    let (provider, state) = provider(LaunchMode::Healthy);
    let artifact_root = tempfile::tempdir().unwrap();
    provider
        .provision(provision_request(
            WorkerId::new(),
            RunCancellation::default(),
        ))
        .unwrap();
    provider
        .provision(provision_request(
            WorkerId::new(),
            RunCancellation::default(),
        ))
        .unwrap();

    let state = state.lock().unwrap();
    assert_eq!(state.bootstrap_wires.len(), 2);
    let credentials = state
        .bootstrap_wires
        .iter()
        .map(|wire| wire[wire.len() - 32..].to_vec())
        .collect::<Vec<_>>();
    assert_ne!(credentials[0], credentials[1]);

    for credential in credentials {
        let mut encoded = String::with_capacity(64);
        for byte in &credential {
            write!(&mut encoded, "{byte:02x}").unwrap();
        }
        assert!(std::env::args_os().all(|arg| !arg.to_string_lossy().contains(&encoded)));
        assert!(std::env::vars_os().all(|(key, value)| {
            !key.to_string_lossy().contains(&encoded) && !value.to_string_lossy().contains(&encoded)
        }));
    }
    assert_eq!(std::fs::read_dir(artifact_root.path()).unwrap().count(), 0);
}

#[test]
fn external_runtime_receives_only_opaque_lease_metadata() {
    let lease = WorkerLease {
        lease_id: WorkerLeaseId::new(),
        worker_id: WorkerId::new(),
        run_id: RunId::new(),
        worker_kind: WorkerKind::OnDemand,
        profile_version: 7,
    };

    let value = serde_json::to_value(lease).unwrap();
    let object = value.as_object().unwrap();
    assert_eq!(object.len(), 5);
    for key in ["leaseId", "workerId", "runId", "workerKind", "profileVersion"] {
        assert!(object.contains_key(key), "missing opaque lease field {key}");
    }
    for forbidden in ["endpoint", "port", "pid", "path", "token", "secret", "sql", "capability"] {
        assert!(!object.contains_key(forbidden), "lease leaked {forbidden}");
    }
}

#[cfg(windows)]
#[test]
#[ignore = "requires the locally packaged DuckDB 1.5.4 Quack extension"]
fn windows_sidecar_bootstraps_over_pipes_and_executes_a_quack_batch() {
    let program = std::path::PathBuf::from(env!("CARGO_BIN_EXE_duckle-db-sidecar"));
    let provider = LocalProcessProvider::new(
        Arc::new(WindowsLocalSidecarLauncher::new(program).unwrap()),
        HostResourceLimits::default(),
    );
    let worker_id = WorkerId::new();
    let cancellation = RunCancellation::default();

    provider
        .provision(provision_request(worker_id, cancellation.clone()))
        .expect("authenticated sidecar readiness");
    let database = provider
        .open_database(worker_id, cancellation)
        .expect("private Quack database facade");
    let result = database
        .execute_batch(vec!["SELECT 42 AS answer".to_string()])
        .expect("Quack batch");

    assert_eq!(result.rows, 1);
    assert_eq!(result.transport, duckle_db_runner::model::TransportKind::Quack);
    provider.terminate(worker_id);
}
