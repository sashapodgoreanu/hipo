//! Windows local-process implementation of the trusted Quack sidecar.
//!
//! The child receives its credential only through the inherited bootstrap
//! pipe. The parent keeps the Quack endpoint and client secret inside this
//! module; `WorkerPoolControl`, IPC and the executor see only `RunDatabase`.

use crate::bootstrap::{read_authenticated_readiness, write_authenticated_readiness, BootstrapMessage};
use crate::local_process_provider::{LaunchedLocalSidecar, LocalSidecarLaunch, LocalSidecarLauncher, ManagedSidecar};
use crate::model::{RunCancellation, RunnerFailureReason};
use crate::resources::ResolvedRunnerResources;
use crate::run_database::{QuackTransport, RunDatabase};
use duckdb::{params, Connection};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const REMOTE_ALIAS: &str = "duckle_runner_remote";

/// Starts the packaged `duckle-db-sidecar` executable on Windows.
///
/// It is intentionally a provider implementation rather than an engine API:
/// its process handle, endpoint and Quack credential cannot escape this
/// module.
#[cfg(windows)]
pub struct WindowsLocalSidecarLauncher {
    program: PathBuf,
}

#[cfg(windows)]
impl WindowsLocalSidecarLauncher {
    pub fn new(program: PathBuf) -> Result<Self, RunnerFailureReason> {
        if !program.is_absolute() {
            return Err(RunnerFailureReason::RunnerUnavailable);
        }
        Ok(Self { program })
    }
}

#[cfg(windows)]
impl LocalSidecarLauncher for WindowsLocalSidecarLauncher {
    fn launch(
        &self,
        request: LocalSidecarLaunch,
    ) -> Result<LaunchedLocalSidecar, RunnerFailureReason> {
        if request.cancellation.is_cancelled() {
            return Err(RunnerFailureReason::Cancelled);
        }
        let mut process = crate::windows_bootstrap::spawn_sidecar(&self.program, &[])
            .map_err(|_| RunnerFailureReason::RunnerUnavailable)?;
        if process.send_bootstrap(&request.bootstrap).is_err() {
            let _ = process.terminate_tree();
            return Err(RunnerFailureReason::RunnerUnavailable);
        }
        let readiness = match read_authenticated_readiness(process.control_reader(), &request.bootstrap) {
            Ok(readiness) => readiness,
            Err(_) => {
                let _ = process.terminate_tree();
                return Err(RunnerFailureReason::RunnerUnavailable);
            }
        };
        if request.cancellation.is_cancelled() {
            let _ = process.terminate_tree();
            return Err(RunnerFailureReason::Cancelled);
        }

        Ok(LaunchedLocalSidecar::new(
            Box::new(WindowsManagedSidecar {
                process,
                readiness_endpoint: readiness.endpoint(),
                bootstrap: request.bootstrap,
                profile: request.effective_profile,
            }),
            readiness,
        ))
    }
}

#[cfg(windows)]
struct WindowsManagedSidecar {
    process: crate::windows_bootstrap::SpawnedSidecar,
    readiness_endpoint: std::net::SocketAddr,
    bootstrap: BootstrapMessage,
    profile: ResolvedRunnerResources,
}

#[cfg(windows)]
impl WindowsManagedSidecar {
    fn open_quack_database(
        &self,
        cancellation: RunCancellation,
    ) -> Result<RunDatabase, RunnerFailureReason> {
        if cancellation.is_cancelled() {
            return Err(RunnerFailureReason::Cancelled);
        }
        let connection = Connection::open_in_memory().map_err(|_| RunnerFailureReason::RunnerUnavailable)?;
        connection
            .execute_batch("LOAD quack;")
            .map_err(|_| RunnerFailureReason::RunnerUnavailable)?;
        if let Some(threads) = self.profile.cpu_threads {
            connection
                .execute_batch(&format!("SET threads = {threads};"))
                .map_err(|_| RunnerFailureReason::RunnerUnavailable)?;
        }
        let uri = format!("quack:{}", self.readiness_endpoint);
        connection
            .execute(
                "CREATE TEMPORARY SECRET duckle_runner_quack_credentials (TYPE quack, SCOPE ?, TOKEN ?)",
                params![uri, bootstrap_token(&self.bootstrap)],
            )
            .map_err(|_| RunnerFailureReason::RunnerUnavailable)?;
        connection
            .execute_batch(&format!(
                "ATTACH {} AS {REMOTE_ALIAS} (TYPE quack);",
                sql_literal(&uri)
            ))
            .map_err(|_| RunnerFailureReason::RunnerUnavailable)?;
        let transport = QuackTransport::from_attached_connection(connection, REMOTE_ALIAS.into())?;
        Ok(RunDatabase::new(Arc::new(transport), cancellation))
    }
}

#[cfg(windows)]
impl ManagedSidecar for WindowsManagedSidecar {
    fn apply_effective_profile(
        &mut self,
        profile: &ResolvedRunnerResources,
    ) -> Result<(), RunnerFailureReason> {
        // Quack has no authenticated reconfiguration control protocol. An
        // idle worker carrying another profile must be replaced rather than
        // being silently relabelled with settings it has not applied.
        if profile == &self.profile {
            Ok(())
        } else {
            Err(RunnerFailureReason::ConfigurationApplyFailed)
        }
    }

    fn open_database(
        &mut self,
        cancellation: RunCancellation,
    ) -> Result<RunDatabase, RunnerFailureReason> {
        self.open_quack_database(cancellation)
    }

    fn terminate(mut self: Box<Self>) {
        let _ = self.process.terminate_tree();
    }
}

/// Entrypoint used exclusively by the packaged Windows sidecar executable.
/// It intentionally accepts only inherited handle numbers; all resource
/// settings and the one-shot credential arrive in the authenticated payload.
#[cfg(windows)]
pub fn run_windows_sidecar(args: &[std::ffi::OsString]) -> Result<(), RunnerFailureReason> {
    let bootstrap_handle = inherited_handle(args, "--duckle-bootstrap-read-handle")?;
    let control_handle = inherited_handle(args, "--duckle-control-write-handle")?;
    let (mut bootstrap_reader, control_writer) = unsafe {
        crate::windows_bootstrap::take_child_pipes(bootstrap_handle, control_handle)
    };
    let bootstrap = crate::bootstrap::read_bootstrap(&mut bootstrap_reader)
        .map_err(|_| RunnerFailureReason::RunnerUnavailable)?;
    run_windows_sidecar_from_bootstrap(&bootstrap, control_writer)
}

#[cfg(windows)]
fn run_windows_sidecar_from_bootstrap(
    bootstrap: &BootstrapMessage,
    mut control_writer: std::fs::File,
) -> Result<(), RunnerFailureReason> {
    let profile = bootstrap.effective_profile();
    let temp_directory = std::env::temp_dir().join(format!("duckle-db-runner-{}", bootstrap.worker_id()));
    std::fs::create_dir_all(&temp_directory).map_err(|_| RunnerFailureReason::RunnerUnavailable)?;
    let cleanup = TempDirectoryGuard(temp_directory.clone());
    let connection = Connection::open_in_memory().map_err(|_| RunnerFailureReason::RunnerUnavailable)?;
    apply_profile(&connection, profile, &temp_directory)?;
    connection
        .execute_batch("LOAD quack;")
        .map_err(|_| RunnerFailureReason::RunnerUnavailable)?;
    connection
        .execute_batch(
            "SET GLOBAL quack_authentication_function = 'quack_check_token'; \
             SET GLOBAL quack_authorization_function = 'quack_nop_authorization';",
        )
        .map_err(|_| RunnerFailureReason::RunnerUnavailable)?;
    let token = bootstrap_token(bootstrap);
    let (uri, _url, returned_token): (String, String, String) = connection
        .query_row(
            "CALL quack_serve(?, token => ?)",
            params!["quack:127.0.0.1:0", token],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|_| RunnerFailureReason::RunnerUnavailable)?;
    if returned_token != token {
        return Err(RunnerFailureReason::RunnerUnavailable);
    }
    let endpoint = uri
        .strip_prefix("quack:")
        .and_then(|value| value.parse::<std::net::SocketAddr>().ok())
        .ok_or(RunnerFailureReason::RunnerUnavailable)?;
    write_authenticated_readiness(&mut control_writer, bootstrap, endpoint)
        .map_err(|_| RunnerFailureReason::RunnerUnavailable)?;
    drop(control_writer);

    // The embedded connection owns Quack's listener. The parent Job Object
    // terminates this process on cancellation, crash, or parent shutdown.
    loop {
        std::thread::park_timeout(std::time::Duration::from_secs(60));
        let _ = &cleanup;
    }
}

#[cfg(windows)]
fn apply_profile(
    connection: &Connection,
    profile: &ResolvedRunnerResources,
    temp_directory: &Path,
) -> Result<(), RunnerFailureReason> {
    if let Some(memory_bytes) = profile.memory_bytes {
        connection
            .execute_batch(&format!("SET memory_limit = '{}B';", memory_bytes))
            .map_err(|_| RunnerFailureReason::ConfigurationApplyFailed)?;
    }
    if let Some(cpu_threads) = profile.cpu_threads {
        connection
            .execute_batch(&format!("SET threads = {cpu_threads};"))
            .map_err(|_| RunnerFailureReason::ConfigurationApplyFailed)?;
    }
    connection
        .execute_batch(&format!(
            "SET temp_directory = {}; SET preserve_insertion_order = false;",
            sql_literal(&temp_directory.to_string_lossy())
        ))
        .map_err(|_| RunnerFailureReason::ConfigurationApplyFailed)
}

#[cfg(windows)]
fn inherited_handle(args: &[std::ffi::OsString], flag: &str) -> Result<usize, RunnerFailureReason> {
    let mut values = args.iter().map(|value| value.to_string_lossy());
    while let Some(value) = values.next() {
        if value == flag {
            return values
                .next()
                .and_then(|value| value.parse::<usize>().ok())
                .ok_or(RunnerFailureReason::RunnerUnavailable);
        }
    }
    Err(RunnerFailureReason::RunnerUnavailable)
}

fn bootstrap_token(bootstrap: &BootstrapMessage) -> String {
    bootstrap
        .credential()
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn sql_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(windows)]
struct TempDirectoryGuard(PathBuf);

#[cfg(windows)]
impl Drop for TempDirectoryGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::{HostResourceLimits, RunnerResourcesProfile};

    #[test]
    fn bootstrap_token_is_hex_and_does_not_require_an_environment_variable() {
        let bootstrap = BootstrapMessage::new(
            crate::model::WorkerId::new(),
            RunnerResourcesProfile::default()
                .resolve(HostResourceLimits::default())
                .unwrap(),
        )
        .unwrap();
        let token = bootstrap_token(&bootstrap);
        assert_eq!(token.len(), 64);
        assert!(token.chars().all(|character| character.is_ascii_hexdigit()));
    }
}
