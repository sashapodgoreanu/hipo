//! Windows local-process implementation of the trusted Quack sidecar.
//!
//! The child receives its credential only through the inherited bootstrap
//! pipe. The parent keeps the Quack endpoint and client secret inside this
//! module; `WorkerPoolControl`, IPC and the executor see only `RunDatabase`.

use crate::bootstrap::{
    read_authenticated_readiness, write_authenticated_readiness, BootstrapMessage,
};
use crate::bundle::{
    bundle_for, packaged_extension_path, verify_staged_bundle, BundlePlatform,
};
use crate::local_process_provider::{
    LaunchedLocalSidecar, LocalSidecarLaunch, LocalSidecarLauncher, ManagedSidecar,
};
use crate::model::{RunCancellation, RunnerFailureReason};
use crate::resources::ResolvedRunnerResources;
use crate::run_database::{QuackTransport, RunDatabase};
use crate::sidecar_diagnostics::{append_sidecar_diagnostic, SidecarDiagnosticCode};
use duckdb::{params, Connection};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const REMOTE_ALIAS: &str = "duckle_runner_remote";
const QUACK_BIND_ATTEMPTS: usize = 8;

#[cfg(windows)]
fn verified_packaged_extension(program: &Path) -> Result<PathBuf, RunnerFailureReason> {
    let platform = BundlePlatform::current().ok_or(RunnerFailureReason::RunnerUnavailable)?;
    let path = packaged_extension_path(program, platform)?;
    let expected = bundle_for(platform).ok_or(RunnerFailureReason::RunnerUnavailable)?;
    verify_staged_bundle(&path, expected)?;
    Ok(path)
}

fn quack_load_sql(path: &Path) -> String {
    // Forward slashes avoid any ambiguity around Windows backslashes inside a
    // SQL string literal. DuckDB accepts them on Windows paths.
    let normalized = path.to_string_lossy().replace('\\', "/");
    format!("LOAD {};", sql_literal(&normalized))
}

#[cfg(windows)]
fn load_packaged_quack(
    connection: &Connection,
    extension: &Path,
) -> Result<(), RunnerFailureReason> {
    let platform = BundlePlatform::current().ok_or(RunnerFailureReason::RunnerUnavailable)?;
    let expected = bundle_for(platform).ok_or(RunnerFailureReason::RunnerUnavailable)?;
    verify_staged_bundle(extension, expected)?;
    connection
        .execute_batch(&quack_load_sql(extension))
        .map_err(|_| RunnerFailureReason::RunnerUnavailable)
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
            append_sidecar_diagnostic(&program, SidecarDiagnosticCode::ParentProgramInvalid);
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
        append_sidecar_diagnostic(&self.program, SidecarDiagnosticCode::ParentLaunchStarted);

        if request.cancellation.is_cancelled() {
            append_sidecar_diagnostic(&self.program, SidecarDiagnosticCode::ParentCancelled);
            return Err(RunnerFailureReason::Cancelled);
        }

        let mut process = match crate::windows_bootstrap::spawn_sidecar(&self.program, &[]) {
            Ok(process) => process,
            Err(_) => {
                append_sidecar_diagnostic(&self.program, SidecarDiagnosticCode::ParentSpawnFailed);
                return Err(RunnerFailureReason::RunnerUnavailable);
            }
        };

        if process.send_bootstrap(&request.bootstrap).is_err() {
            append_sidecar_diagnostic(
                &self.program,
                SidecarDiagnosticCode::ParentBootstrapSendFailed,
            );
            let _ = process.terminate_tree();
            return Err(RunnerFailureReason::RunnerUnavailable);
        }

        let readiness = match read_authenticated_readiness(
            process.control_reader(),
            &request.bootstrap,
        ) {
            Ok(readiness) => readiness,
            Err(_) => {
                append_sidecar_diagnostic(
                    &self.program,
                    SidecarDiagnosticCode::ParentReadinessFailed,
                );
                let _ = process.terminate_tree();
                return Err(RunnerFailureReason::RunnerUnavailable);
            }
        };

        if request.cancellation.is_cancelled() {
            append_sidecar_diagnostic(&self.program, SidecarDiagnosticCode::ParentCancelled);
            let _ = process.terminate_tree();
            return Err(RunnerFailureReason::Cancelled);
        }

        append_sidecar_diagnostic(&self.program, SidecarDiagnosticCode::ParentReadinessOk);
        Ok(LaunchedLocalSidecar::new(
            Box::new(WindowsManagedSidecar {
                process,
                program: self.program.clone(),
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
    program: PathBuf,
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
        append_sidecar_diagnostic(&self.program, SidecarDiagnosticCode::ClientOpenStarted);

        if cancellation.is_cancelled() {
            append_sidecar_diagnostic(&self.program, SidecarDiagnosticCode::ParentCancelled);
            return Err(RunnerFailureReason::Cancelled);
        }

        let connection = match Connection::open_in_memory() {
            Ok(connection) => connection,
            Err(_) => {
                append_sidecar_diagnostic(
                    &self.program,
                    SidecarDiagnosticCode::ClientConnectionOpenFailed,
                );
                return Err(RunnerFailureReason::RunnerUnavailable);
            }
        };

        let extension = verified_packaged_extension(&self.program).map_err(|reason| {
            append_sidecar_diagnostic(
                &self.program,
                SidecarDiagnosticCode::ClientQuackLoadFailed,
            );
            reason
        })?;
        load_packaged_quack(&connection, &extension).map_err(|reason| {
            append_sidecar_diagnostic(
                &self.program,
                SidecarDiagnosticCode::ClientQuackLoadFailed,
            );
            reason
        })?;

        if let Some(threads) = self.profile.cpu_threads {
            if connection
                .execute_batch(&format!("SET threads = {threads};"))
                .is_err()
            {
                append_sidecar_diagnostic(
                    &self.program,
                    SidecarDiagnosticCode::ClientProfileApplyFailed,
                );
                return Err(RunnerFailureReason::RunnerUnavailable);
            }
        }

        let uri = format!("quack:{}", self.readiness_endpoint);
        if connection
            .execute(
                "CREATE TEMPORARY SECRET duckle_runner_quack_credentials (TYPE quack, SCOPE ?, TOKEN ?)",
                params![uri, bootstrap_token(&self.bootstrap)],
            )
            .is_err()
        {
            append_sidecar_diagnostic(
                &self.program,
                SidecarDiagnosticCode::ClientSecretCreateFailed,
            );
            return Err(RunnerFailureReason::RunnerUnavailable);
        }

        if connection
            .execute_batch(&format!(
                "ATTACH {} AS {REMOTE_ALIAS} (TYPE quack);",
                sql_literal(&uri)
            ))
            .is_err()
        {
            append_sidecar_diagnostic(
                &self.program,
                SidecarDiagnosticCode::ClientAttachFailed,
            );
            return Err(RunnerFailureReason::RunnerUnavailable);
        }

        let transport = match QuackTransport::from_attached_connection(
            connection,
            REMOTE_ALIAS.into(),
        ) {
            Ok(transport) => transport,
            Err(reason) => {
                append_sidecar_diagnostic(
                    &self.program,
                    SidecarDiagnosticCode::ClientTransportFailed,
                );
                return Err(reason);
            }
        };

        append_sidecar_diagnostic(&self.program, SidecarDiagnosticCode::ClientAttachOk);
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
pub fn run_windows_sidecar(
    args: &[std::ffi::OsString],
    quack_extension: &Path,
) -> Result<(), RunnerFailureReason> {
    let bootstrap_handle = inherited_handle(args, "--duckle-bootstrap-read-handle")?;
    let control_handle = inherited_handle(args, "--duckle-control-write-handle")?;
    let (mut bootstrap_reader, control_writer) = unsafe {
        crate::windows_bootstrap::take_child_pipes(bootstrap_handle, control_handle)
    };
    let bootstrap = crate::bootstrap::read_bootstrap(&mut bootstrap_reader)
        .map_err(|_| RunnerFailureReason::RunnerUnavailable)?;
    run_windows_sidecar_from_bootstrap(&bootstrap, control_writer, quack_extension)
}

#[cfg(windows)]
fn run_windows_sidecar_from_bootstrap(
    bootstrap: &BootstrapMessage,
    mut control_writer: std::fs::File,
    quack_extension: &Path,
) -> Result<(), RunnerFailureReason> {
    let profile = bootstrap.effective_profile();
    let temp_directory =
        std::env::temp_dir().join(format!("duckle-db-runner-{}", bootstrap.worker_id()));
    std::fs::create_dir_all(&temp_directory)
        .map_err(|_| RunnerFailureReason::RunnerUnavailable)?;
    let cleanup = TempDirectoryGuard(temp_directory.clone());

    let connection =
        Connection::open_in_memory().map_err(|_| RunnerFailureReason::RunnerUnavailable)?;
    apply_and_verify_profile(&connection, profile, &temp_directory)?;
    load_packaged_quack(&connection, quack_extension)?;
    connection
        .execute_batch(
            "SET GLOBAL quack_authentication_function = 'quack_check_token'; \
             SET GLOBAL quack_authorization_function = 'quack_nop_authorization';",
        )
        .map_err(|_| RunnerFailureReason::RunnerUnavailable)?;

    let token = bootstrap_token(bootstrap);
    let (uri, _url, returned_token) = serve_quack_on_free_loopback_port(&connection, &token)?;
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
fn serve_quack_on_free_loopback_port(
    connection: &Connection,
    token: &str,
) -> Result<(String, String, String), RunnerFailureReason> {
    for _ in 0..QUACK_BIND_ATTEMPTS {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
            .map_err(|_| RunnerFailureReason::RunnerUnavailable)?;
        let port = listener
            .local_addr()
            .map_err(|_| RunnerFailureReason::RunnerUnavailable)?
            .port();
        drop(listener);

        let bind_uri = format!("quack:127.0.0.1:{port}");
        if let Ok(result) = connection.query_row(
            "CALL quack_serve(?, token => ?)",
            params![bind_uri, token],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ) {
            return Ok(result);
        }
    }

    Err(RunnerFailureReason::RunnerUnavailable)
}

#[cfg(windows)]
fn apply_and_verify_profile(
    connection: &Connection,
    profile: &ResolvedRunnerResources,
    temp_directory: &Path,
) -> Result<(), RunnerFailureReason> {
    profile
        .validate()
        .map_err(|_| RunnerFailureReason::ConfigurationApplyFailed)?;

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
    if let Some(spill_bytes) = profile.spill_bytes {
        connection
            .execute_batch(&format!("SET max_temp_directory_size = '{}B';", spill_bytes))
            .map_err(|_| RunnerFailureReason::ConfigurationApplyFailed)?;
    }

    connection
        .execute_batch(&format!(
            "SET temp_directory = {}; SET preserve_insertion_order = false;",
            sql_literal(&temp_directory.to_string_lossy())
        ))
        .map_err(|_| RunnerFailureReason::ConfigurationApplyFailed)?;

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
            return Err(RunnerFailureReason::ConfigurationApplyFailed);
        }
    }

    if let Some(expected) = profile.cpu_threads {
        let actual = setting_value(connection, "threads")?
            .parse::<u16>()
            .map_err(|_| RunnerFailureReason::ConfigurationApplyFailed)?;
        if actual != expected {
            return Err(RunnerFailureReason::ConfigurationApplyFailed);
        }
    }

    if let Some(expected) = profile.spill_bytes {
        let actual = setting_value(connection, "max_temp_directory_size")?;
        if !setting_bytes_match(&actual, expected) {
            return Err(RunnerFailureReason::ConfigurationApplyFailed);
        }
    }

    let actual_temp = setting_value(connection, "temp_directory")?;
    if PathBuf::from(actual_temp) != temp_directory {
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
        .map_err(|_| RunnerFailureReason::ConfigurationApplyFailed)
}

#[cfg(windows)]
fn verify_temporary_directory(temp_directory: &Path) -> Result<(), RunnerFailureReason> {
    use std::io::Write;

    if !temp_directory.is_dir() {
        return Err(RunnerFailureReason::ConfigurationApplyFailed);
    }

    let probe = temp_directory.join(format!(".duckle-resource-probe-{}", std::process::id()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&probe)
            .map_err(|_| RunnerFailureReason::ConfigurationApplyFailed)?;
        file.write_all(b"duckle")
            .and_then(|_| file.sync_all())
            .map_err(|_| RunnerFailureReason::ConfigurationApplyFailed)
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
    let compact: String = value
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
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
fn inherited_handle(
    args: &[std::ffi::OsString],
    flag: &str,
) -> Result<usize, RunnerFailureReason> {
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
        assert!(token
            .chars()
            .all(|character| character.is_ascii_hexdigit()));
    }

    #[test]
    fn packaged_quack_load_uses_an_explicit_normalized_path() {
        let sql = quack_load_sql(Path::new(
            r"C:\Users\tester\AppData\Roaming\io.duckle.app\engines\db-sidecar\extensions\v1.5.4\windows_amd64\quack.duckdb_extension",
        ));
        assert_eq!(
            sql,
            "LOAD 'C:/Users/tester/AppData/Roaming/io.duckle.app/engines/db-sidecar/extensions/v1.5.4/windows_amd64/quack.duckdb_extension';"
        );
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
