#![cfg(windows)]

use duckle_db_runner::local_process_provider::LocalProcessProvider;
use duckle_db_runner::local_quack_sidecar::WindowsLocalSidecarLauncher;
use duckle_db_runner::model::{
    RunCancellation, RunnerFailureReason, TransportKind, WorkerId, WorkerKind,
};
use duckle_db_runner::resources::{
    AutomaticOrU16, HostResourceLimits, ResourceLimit, RunnerResourcesProfile,
};
use duckle_db_runner::run_database::{RunDatabase, SqlBatchResult};
use duckle_db_runner::worker_pool::{WorkerProvider, WorkerProvisionRequest};
use std::path::PathBuf;
use std::sync::Arc;

const DEBUG_LOG_ENV: &str = "DUCKLE_SIDECAR_DEBUG_LOG";

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

fn debug_log_path() -> PathBuf {
    let path = std::env::var_os(DEBUG_LOG_ENV)
        .map(PathBuf::from)
        .expect("set DUCKLE_SIDECAR_DEBUG_LOG to an absolute writable file path");
    assert!(path.is_absolute(), "DUCKLE_SIDECAR_DEBUG_LOG must be absolute");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create sidecar debug log directory");
    }
    let _ = std::fs::remove_file(&path);
    path
}

fn startup_failure(context: &str, reason: RunnerFailureReason, log_path: &PathBuf) -> ! {
    let log = std::fs::read_to_string(log_path)
        .unwrap_or_else(|error| format!("<debug log unavailable: {error}>"));
    panic!(
        "{context}: {reason:?}\nsidecar debug log: {}\n--- sidecar log ---\n{log}\n--- end sidecar log ---",
        log_path.display()
    );
}

fn expect_runner<T>(
    result: Result<T, RunnerFailureReason>,
    context: &str,
    log_path: &PathBuf,
) -> T {
    match result {
        Ok(value) => value,
        Err(reason) => startup_failure(context, reason, log_path),
    }
}

#[test]
#[ignore = "requires the locally staged and pinned DuckDB 1.5.4 Quack extension"]
fn packaged_windows_sidecar_bootstraps_and_executes_a_quack_batch() {
    let log_path = debug_log_path();

    // This integration test belongs to the package that owns the binary, so
    // Cargo always builds the exact sidecar under test before exposing this path.
    let program = PathBuf::from(env!("CARGO_BIN_EXE_duckle-db-sidecar"));
    assert!(program.is_absolute(), "Cargo must expose an absolute sidecar path");
    assert!(program.is_file(), "Cargo did not build the database sidecar");

    let launcher = WindowsLocalSidecarLauncher::new(program).expect("valid sidecar executable");
    let provider = LocalProcessProvider::new(Arc::new(launcher), deterministic_host_limits());
    let worker_id = WorkerId::new();
    let cancellation = RunCancellation::default();

    expect_runner(
        provider.provision(WorkerProvisionRequest {
            worker_id,
            kind: WorkerKind::Warm,
            profile: requested_profile(),
            cancellation: cancellation.clone(),
        }),
        "authenticated sidecar readiness",
        &log_path,
    );

    let database: RunDatabase = expect_runner(
        provider.open_database(worker_id, cancellation),
        "private Quack database facade",
        &log_path,
    );
    let result: SqlBatchResult = expect_runner(
        database.execute_batch(vec!["SELECT 42 AS answer".to_string()]),
        "Quack batch",
        &log_path,
    );

    assert_eq!(result.rows, 1);
    assert_eq!(result.transport, TransportKind::Quack);
    provider.terminate(worker_id);
}
