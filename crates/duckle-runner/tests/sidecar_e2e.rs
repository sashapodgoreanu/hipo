#![cfg(windows)]

use duckdb::{params, Connection};
use duckle_db_runner::local_process_provider::LocalProcessProvider;
use duckle_db_runner::local_quack_sidecar::WindowsLocalSidecarLauncher;
use duckle_db_runner::model::{RunCancellation, TransportKind, WorkerId, WorkerKind};
use duckle_db_runner::resources::{
    AutomaticOrU16, HostResourceLimits, ResourceLimit, RunnerResourcesProfile,
};
use duckle_db_runner::worker_pool::{WorkerProvider, WorkerProvisionRequest};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const PROBE_TOKEN: &str = "duckle-sidecar-probe-token";

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

fn staged_extension() -> PathBuf {
    let path = std::env::var_os("DUCKLE_QUACK_EXTENSION")
        .map(PathBuf::from)
        .expect("DUCKLE_QUACK_EXTENSION must point to the pinned extension");
    assert!(path.is_absolute(), "the staged extension path must be absolute");
    assert!(path.is_file(), "the staged extension does not exist");
    path
}

fn sql_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn load_staged_quack(connection: &Connection, extension: &Path) {
    let extension = extension.to_string_lossy().replace('\\', "/");
    connection
        .execute_batch(&format!("LOAD {};", sql_literal(&extension)))
        .expect("load the pinned Quack extension into bundled DuckDB 1.5.4");
}

#[test]
#[ignore = "requires the locally staged and pinned DuckDB 1.5.4 Quack extension"]
fn pinned_quack_extension_loads_in_bundled_duckdb() {
    let connection = Connection::open_in_memory().expect("open bundled DuckDB");
    load_staged_quack(&connection, &staged_extension());

    let loaded: bool = connection
        .query_row(
            "SELECT loaded FROM duckdb_extensions() WHERE extension_name = 'quack'",
            [],
            |row| row.get(0),
        )
        .expect("inspect loaded Quack extension");
    assert!(loaded, "Quack was not marked as loaded");
}

#[test]
#[ignore = "requires the locally staged and pinned DuckDB 1.5.4 Quack extension"]
fn pinned_quack_extension_starts_an_ephemeral_loopback_server() {
    let connection = Connection::open_in_memory().expect("open bundled DuckDB");
    load_staged_quack(&connection, &staged_extension());
    connection
        .execute_batch(
            "SET GLOBAL quack_authentication_function = 'quack_check_token'; \
             SET GLOBAL quack_authorization_function = 'quack_nop_authorization';",
        )
        .expect("configure Quack authentication and authorization callbacks");

    let (uri, returned_token): (String, String) = connection
        .query_row(
            "SELECT listen_uri, auth_token FROM quack_serve(?, token => ?)",
            params!["quack:127.0.0.1:0", PROBE_TOKEN],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("start Quack on an ephemeral loopback port");

    assert!(uri.starts_with("quack:127.0.0.1:"), "unexpected listen URI shape");
    assert!(!uri.ends_with(":0"), "Quack did not allocate a concrete port");
    assert_eq!(returned_token, PROBE_TOKEN);

    connection
        .query_row("CALL quack_stop(?)", params![uri], |_| Ok(()))
        .expect("stop the probe Quack server");
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
