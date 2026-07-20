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
const DEBUG_LOG_ENV: &str = "DUCKLE_SIDECAR_DEBUG_LOG";

fn debug_log(message: &str) {
    use std::io::Write;

    let Some(path) = std::env::var_os(DEBUG_LOG_ENV).map(PathBuf::from) else {
        return;
    };
    let timestamp_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{timestamp_ms} pid={} {message}", std::process::id());
        let _ = file.flush();
    }
}

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
            debug_log("parent.launch.program_not_absolute");
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
        debug_log("parent.launch.start");
        if request.cancellation.is_cancelled() {
            debug_log("parent.launch.cancelled_before_spawn");
            return Err(RunnerFailureReason::Cancelled);
        }
        let mut process = match crate::windows_bootstrap::spawn_sidecar(&self.program, &[]) {
            Ok(process) => process,
            Err(error) => {
                debug_log(&format!("parent.launch.spawn.error={error}"));
                return Err(RunnerFailureReason::RunnerUnavailable);
            }
        };
        debug_log("parent.launch.spawn.ok");
        if let Err(error) = process.send_bootstrap(&request.bootstrap) {
            debug_log(&format!("parent.bootstrap.write.error={error}"));
            let _ = process.terminate_tree();
            return Err(RunnerFailureReason::RunnerUnavailable);
        }
        debug_log("parent.bootstrap.write.ok");
        let readiness = match read_authenticated_readiness(process.control_reader(), &request.bootstrap) {
            Ok(readiness) => readiness,
            Err(error) => {
                debug_log(&format!("parent.readiness.read.error={error}"));
                let _ = process.terminate_tree();
                return Err(RunnerFailureReason::RunnerUnavailable);
            }
        };
        debug_log("parent.readiness.read.ok");
        if request.cancellation.is_cancelled() {
            debug_log("parent.launch.cancelled_after_readiness");
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
        debug_log("parent.database.open.start");
        if cancellation.is_cancelled() {
            debug_log("parent.database.open.cancelled");
            return Err(RunnerFailureReason::Cancelled);
        }
        let connection = Connection::open_in_memory().map_err(|error| {
            debug_log(&format!("parent.database.open.error={error}"));
            RunnerFailureReason::RunnerUnavailable
        })?;
        connection.execute_batch("LOAD quack;").map_err(|error| {
            debug_log(&format!("parent.quack.load.error={error}"));
            RunnerFailureReason::RunnerUnavailable
        })?;
        debug_log("parent.quack.load.ok");
        if let Some(threads) = self.profile.cpu_threads {
            connection
                .execute_batch(&format!("SET threads = {threads};"))
                .map_err(|error| {
                    debug_log(&format!("parent.profile.threads.error={error}"));
                    RunnerFailureReason::RunnerUnavailable
                })?;
        }
        let uri = format!("quack:{}", self.readiness_endpoint);
        connection
            .execute(
                "CREATE TEMPORARY SECRET duckle_runner_quack_credentials (TYPE quack, SCOPE ?, TOKEN ?)",
                params![uri, bootstrap_token(&self.bootstrap)],
            )
            .map_err(|error| {
                debug_log(&format!("parent.quack.secret.error={error}"));
                RunnerFailureReason::RunnerUnavailable
            })?;
        debug_log("parent.quack.secret.ok");
        connection
            .execute_batch(&format!(
                "ATTACH {} AS {REMOTE_ALIAS} (TYPE quack);",
                sql_literal(&uri)
            ))
            .map_err(|error| {
                debug_log(&format!("parent.quack.attach.error={error}"));
                RunnerFailureReason::RunnerUnavailable
            })?;
        debug_log("parent.quack.attach.ok");
        let transport = QuackTransport::from_attached_connection(connection, REMOTE_ALIAS.into())
            .map_err(|reason| {
                debug_log(&format!("parent.quack.transport.error={reason:?}"));
                reason
            })?;
        debug_log("parent.database.open.ok");
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
            debug_log("parent.profile.apply.mismatch");
            Err(RunnerFailureReason::ConfigurationApplyFailed)
        }
    }

    fn verify_effective_profile(
        &mut self,
        profile: &ResolvedRunnerResources,
    ) -> Result<(), RunnerFailureReason> {
        // The child queried DuckDB's effective settings and verified its private
        // temporary directory before authenticating readiness. The parent then
        // authenticates that exact resolved profile; accepting any other value
        // here would silently relabel the managed process.
        if profile == &self.profile {
            Ok(())
        } else {
            debug_log("parent.profile.verify.mismatch");
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
        debug_log("parent.sidecar.terminate");
        let _ = self.process.terminate_tree();
    }
}

/// Entrypoint used exclusively by the packaged Windows sidecar executable.
/// It intentionally accepts only inherited handle numbers; all resource
/// settings and the one-shot credential arrive in the authenticated payload.
#[cfg(windows)]
pub fn run_windows_sidecar(args: &[std::ffi::OsString]) -> Result<(), RunnerFailureReason> {
    debug_log("child.bootstrap.start");
    let bootstrap_handle = inherited_handle(args, "--duckle-bootstrap-read-handle")?;
    let control_handle = inherited_handle(args, "--duckle-control-write-handle")?;
    debug_log("child.bootstrap.handles.ok");
    let (mut bootstrap_reader, control_writer) = unsafe {
        crate::windows_bootstrap::take_child_pipes(bootstrap_handle, control_handle)
    };
    let bootstrap = crate::bootstrap::read_bootstrap(&mut bootstrap_reader).map_err(|error| {
        debug_log(&format!("child.bootstrap.read.error={error}"));
        RunnerFailureReason::RunnerUnavailable
    })?;
    debug_log("child.bootstrap.read.ok");
    run_windows_sidecar_from_bootstrap(&bootstrap, control_writer)
}

#[cfg(windows)]
fn run_windows_sidecar_from_bootstrap(
    bootstrap: &BootstrapMessage,
    mut control_writer: std::fs::File,
) -> Result<(), RunnerFailureReason> {
    let profile = bootstrap.effective_profile();
    let temp_directory = std::env::temp_dir().join(format!("duckle-db-runner-{}", bootstrap.worker_id()));
    std::fs::create_dir_all(&temp_directory).map_err(|error| {
        debug_log(&format!("child.temp_directory.create.error={error}"));
        RunnerFailureReason::RunnerUnavailable
    })?;
    debug_log("child.temp_directory.create.ok");
    let cleanup = TempDirectoryGuard(temp_directory.clone());
    let connection = Connection::open_in_memory().map_err(|error| {
        debug_log(&format!("child.database.open.error={error}"));
        RunnerFailureReason::RunnerUnavailable
    })?;
    debug_log("child.database.open.ok");
    apply_and_verify_profile(&connection, profile, &temp_directory).map_err(|reason| {
        debug_log(&format!("child.profile.apply.error={reason:?}"));
        reason
    })?;
    debug_log("child.profile.apply.ok");
    connection.execute_batch("LOAD quack;").map_err(|error| {
        debug_log(&format!("child.quack.load.error={error}"));
        RunnerFailureReason::RunnerUnavailable
    })?;
    debug_log("child.quack.load.ok");
    connection
        .execute_batch(
            "SET GLOBAL quack_authentication_function = 'quack_check_token'; \
             SET GLOBAL quack_authorization_function = 'quack_nop_authorization';",
        )
        .map_err(|error| {
            debug_log(&format!("child.quack.authentication.error={error}"));
            RunnerFailureReason::RunnerUnavailable
        })?;
    debug_log("child.quack.authentication.ok");
    let token = bootstrap_token(bootstrap);
    let (uri, _url, returned_token): (String, String, String) = connection
        .query_row(
            "CALL quack_serve(?, token => ?)",
            params!["quack:127.0.0.1:0", token],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|error| {
            debug_log(&format!("child.quack.serve.error={error}"));
            RunnerFailureReason::RunnerUnavailable
        })?;
    debug_log("child.quack.serve.ok");
    if returned_token != token {
        debug_log("child.quack.serve.token_mismatch");
        return Err(RunnerFailureReason::RunnerUnavailable);
    }
    let endpoint = uri
        .strip_prefix("quack:")
        .and_then(|value| value.parse::<std::net::SocketAddr>().ok())
        .ok_or_else(|| {
            debug_log("child.quack.serve.endpoint_invalid");
            RunnerFailureReason::RunnerUnavailable
        })?;
    write_authenticated_readiness(&mut control_writer, bootstrap, endpoint).map_err(|error| {
        debug_log(&format!("child.readiness.write.error={error}"));
        RunnerFailureReason::RunnerUnavailable
    })?;
    debug_log("child.readiness.write.ok");
    drop(control_writer);

    // The embedded connection owns Quack's listener. The parent Job Object
    // terminates this process on cancellation, crash, or parent shutdown.
    loop {
        std::thread::park_timeout(std::time::Duration::from_secs(60));
        let _ = &cleanup;
    }
}

#[cfg(windows)]
fn apply_and_verify_profile(
    connection: &Connection,
    profile: &ResolvedRunnerResources,
    temp_directory: &Path,
) -> Result<(), RunnerFailureReason> {
    profile.validate().map_err(|error| {
        debug_log(&format!("child.profile.validate.error={error}"));
        RunnerFailureReason::ConfigurationApplyFailed
    })?;
    if let Some(memory_bytes) = profile.memory_bytes {
        connection
            .execute_batch(&format!("SET memory_limit = '{}B';", memory_bytes))
            .map_err(|error| {
                debug_log(&format!("child.profile.memory.error={error}"));
                RunnerFailureReason::ConfigurationApplyFailed
            })?;
    }
    if let Some(cpu_threads) = profile.cpu_threads {
        connection
            .execute_batch(&format!("SET threads = {cpu_threads};"))
            .map_err(|error| {
                debug_log(&format!("child.profile.threads.error={error}"));
                RunnerFailureReason::ConfigurationApplyFailed
            })?;
    }
    if let Some(spill_bytes) = profile.spill_bytes {
        connection
            .execute_batch(&format!("SET max_temp_directory_size = '{}B';", spill_bytes))
            .map_err(|error| {
                debug_log(&format!("child.profile.spill.error={error}"));
                RunnerFailureReason::ConfigurationApplyFailed
            })?;
    }
    connection
        .execute_batch(&format!(
            "SET temp_directory = {}; SET preserve_insertion_order = false;",
            sql_literal(&temp_directory.to_string_lossy())
        ))
        .map_err(|error| {
            debug_log(&format!("child.profile.temp_directory.error={error}"));
            RunnerFailureReason::ConfigurationApplyFailed
        })?;

    verify_temporary_directory(temp_directory)?;
    verify_duckdb_profile(connection, profile, temp_directory)
}

#[cfg(windows)]
fn verify_duckdb_profile(
    connection: &Connection,
    profile: &ResolvedRunnerResources,
    temp_directory: &Path,
) -> Result<(), RunnerFailureReason> {
    if let Some(expected) = profile.memory_bytes {
        let actual = setting_value(connection, "memory_limit")?;
        if !setting_bytes_match(&actual, expected) {
            debug_log(&format!("child.profile.memory_mismatch expected={expected} actual={actual}"));
            return Err(RunnerFailureReason::ConfigurationApplyFailed);
        }
    }
    if let Some(expected) = profile.cpu_threads {
        let actual_text = setting_value(connection, "threads")?;
        let actual = actual_text.parse::<u16>().map_err(|error| {
            debug_log(&format!("child.profile.threads_parse.error={error}"));
            RunnerFailureReason::ConfigurationApplyFailed
        })?;
        if actual != expected {
            debug_log(&format!("child.profile.threads_mismatch expected={expected} actual={actual}"));
            return Err(RunnerFailureReason::ConfigurationApplyFailed);
        }
    }
    if let Some(expected) = profile.spill_bytes {
        let actual = setting_value(connection, "max_temp_directory_size")?;
        if !setting_bytes_match(&actual, expected) {
            debug_log(&format!("child.profile.spill_mismatch expected={expected} actual={actual}"));
            return Err(RunnerFailureReason::ConfigurationApplyFailed);
        }
    }
    let actual_temp = setting_value(connection, "temp_directory")?;
    if PathBuf::from(actual_temp) != temp_directory {
        debug_log("child.profile.temp_directory_mismatch");
        return Err(RunnerFailureReason::ConfigurationApplyFailed);
    }
    Ok(())
}

#[cfg(windows)]
fn setting_value(connection: &Connection, name: &str) -> Result<String, RunnerFailureReason> {
    connection
        .query_row(
            "SELECT value FROM duckdb_settings() WHERE name = ?",
            params![name],
            |row| row.get(0),
        )
        .map_err(|error| {
            debug_log(&format!("child.profile.setting_read.error name={name} error={error}"));
            RunnerFailureReason::ConfigurationApplyFailed
        })
}

#[cfg(windows)]
fn verify_temporary_directory(temp_directory: &Path) -> Result<(), RunnerFailureReason> {
    use std::io::Write;

    if !temp_directory.is_dir() {
        debug_log("child.temp_directory.verify.not_directory");
        return Err(RunnerFailureReason::ConfigurationApplyFailed);
    }
    let probe = temp_directory.join(format!(".duckle-resource-probe-{}", std::process::id()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&probe)
            .map_err(|error| {
                debug_log(&format!("child.temp_directory.probe_open.error={error}"));
                RunnerFailureReason::ConfigurationApplyFailed
            })?;
        file.write_all(b"duckle")
            .and_then(|_| file.sync_all())
            .map_err(|error| {
                debug_log(&format!("child.temp_directory.probe_write.error={error}"));
                RunnerFailureReason::ConfigurationApplyFailed
            })
    })();
    let _ = std::fs::remove_file(&probe);
    result
}

fn setting_bytes_match(value: &str, expected: u64) -> bool {
    let Some(actual) = parse_duckdb_bytes(value) else {
        return false;
    };
    let tolerance = (expected / 10_000).max(4 * 1024);
    actual.abs_diff(expected) <= tolerance
}

fn parse_duckdb_bytes(value: &str) -> Option<u64> {
    let compact: String = value.chars().filter(|character| !character.is_whitespace()).collect();
    let unit_start = compact
        .char_indices()
        .find(|(_, character)| !character.is_ascii_digit() && *character != '.')
        .map(|(index, _)| index)
        .unwrap_or(compact.len());
    let number = compact[..unit_start].parse::<f64>().ok()?;
    if !number.is_finite() || number < 0.0 {
        return None;
    }
    let unit = compact[unit_start..].to_ascii_lowercase();
    let multiplier = match unit.as_str() {
        "" | "b" | "byte" | "bytes" => 1_f64,
        "kb" => 1_000_f64,
        "kib" => 1_024_f64,
        "mb" => 1_000_000_f64,
        "mib" => 1_048_576_f64,
        "gb" => 1_000_000_000_f64,
        "gib" => 1_073_741_824_f64,
        "tb" => 1_000_000_000_000_f64,
        "tib" => 1_099_511_627_776_f64,
        _ => return None,
    };
    let bytes = number * multiplier;
    if bytes > u64::MAX as f64 {
        None
    } else {
        Some(bytes.round() as u64)
    }
}

#[cfg(windows)]
fn inherited_handle(args: &[std::ffi::OsString], flag: &str) -> Result<usize, RunnerFailureReason> {
    let mut values = args.iter().map(|value| value.to_string_lossy());
    while let Some(value) = values.next() {
        if value == flag {
            return values
                .next()
                .and_then(|value| value.parse::<usize>().ok())
                .ok_or_else(|| {
                    debug_log("child.bootstrap.handle_value_invalid");
                    RunnerFailureReason::RunnerUnavailable
                });
        }
    }
    debug_log("child.bootstrap.handle_flag_missing");
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

    #[test]
    fn parses_duckdb_binary_and_decimal_size_settings() {
        assert_eq!(parse_duckdb_bytes("1024 B"), Some(1024));
        assert_eq!(parse_duckdb_bytes("1 KiB"), Some(1024));
        assert_eq!(parse_duckdb_bytes("1.5 MiB"), Some(1_572_864));
        assert_eq!(parse_duckdb_bytes("2 GB"), Some(2_000_000_000));
        assert_eq!(parse_duckdb_bytes("unlimited"), None);
    }
}
