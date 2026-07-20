#![cfg(windows)]

use duckle_db_runner::local_process_provider::LocalProcessProvider;
use duckle_db_runner::local_quack_sidecar::WindowsLocalSidecarLauncher;
use duckle_db_runner::model::{RunCancellation, TransportKind, WorkerId, WorkerKind};
use duckle_db_runner::resources::{
    AutomaticOrU16, HostResourceLimits, ResourceLimit, RunnerResourcesProfile,
};
use duckle_db_runner::worker_pool::{WorkerProvider, WorkerProvisionRequest};
use std::path::PathBuf;
use std::sync::Arc;

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

fn deterministic_host_limits() -> HostResourceLimits {
    HostResourceLimits {
        memory_bytes: Some(1_000_000_000),
        memory_cap_bytes: Some(600_000_000),
        cpu_threads: Some(12),
        cpu_thread_cap: Some(4),
        spill_bytes: Some(2_000_000_000),
        spill_cap_bytes: Some(1_000_000_000),
    }
}

#[test]
#[ignore = "requires the locally staged and pinned DuckDB 1.5.4 Quack extension"]
fn packaged_windows_sidecar_bootstraps_and_executes_a_quack_batch() {
    // This integration test belongs to the package that owns the binary, so
    // Cargo always builds the exact sidecar under test before exposing this path.
    let program = PathBuf::from(env!("CARGO_BIN_EXE_duckle-db-sidecar"));
    assert!(program.is_absolute(), "Cargo must expose an absolute sidecar path");
    assert!(program.is_file(), "Cargo did not build the database sidecar");

    let launcher = WindowsLocalSidecarLauncher::new(program).expect("valid sidecar executable");
    let provider = LocalProcessProvider::new(Arc::new(launcher), deterministic_host_limits());
    let worker_id = WorkerId::new();
    let cancellation = RunCancellation::default();

    provider
        .provision(WorkerProvisionRequest {
            worker_id,
            kind: WorkerKind::Warm,
            profile: requested_profile(),
            cancellation: cancellation.clone(),
        })
        .expect("authenticated sidecar readiness");

    let database = provider
        .open_database(worker_id, cancellation)
        .expect("private Quack database facade");
    let result = database
        .execute_batch(vec!["SELECT 42 AS answer".to_string()])
        .expect("Quack batch");

    assert_eq!(result.rows, 1);
    assert_eq!(result.transport, TransportKind::Quack);
    provider.terminate(worker_id);
}
