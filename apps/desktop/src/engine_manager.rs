//! Engine installation manager and packaged Quack runner ownership.
//!
//! Pipeline execution uses the packaged database sidecar. The downloaded DuckDB
//! CLI remains the product's extension provisioner: the Setup UI installs the
//! extensions Duckle needs, including Quack, into DuckDB's shared user cache.

#[allow(dead_code)]
#[path = "engine_manager_base.rs"]
mod base;
pub use base::*;

/// Report the real DuckDB CLI installation state. The packaged sidecar is not
/// enough by itself: the setup is complete only when the CLI-managed Quack
/// extension can actually be loaded from the local DuckDB cache.
pub fn status(app_data: &Path) -> Vec<EngineStatus> {
    let mut statuses = base::status(app_data);
    if let Some(duckdb) = statuses.iter_mut().find(|status| status.id == "duckdb") {
        duckdb.name = "DuckDB / Quack".into();
        duckdb.description = "DuckDB CLI and extensions for the packaged database runner".into();
        if duckdb.installed {
            let quack_ready = quack_is_loadable(&base::duckdb_path(app_data));
            duckdb.installed = quack_ready;
            if !quack_ready {
                duckdb.description =
                    "DuckDB is installed, but the Quack extension still needs installation".into();
            }
        }
    }
    statuses
}

/// Install DuckDB through the existing UI flow, then install Quack exactly like
/// the other DuckDB extensions. Quack is not pinned or hashed by Duckle: the CLI
/// selects the available extension and DuckDB decides whether it can be loaded.
pub fn install<F: FnMut(InstallProgress)>(
    app_data: &Path,
    engine_id: &str,
    mut on_progress: F,
) -> Result<String, String> {
    if engine_id != "duckdb" {
        return base::install(app_data, engine_id, on_progress);
    }

    // base::install emits Done after installing its standard extension set.
    // Hold that final event until Quack has also been installed successfully.
    let path = base::install(app_data, engine_id, |progress| match progress {
        InstallProgress::Done { .. } => {}
        other => on_progress(other),
    })?;

    on_progress(InstallProgress::InstallingExtension {
        name: "quack".into(),
        index: 1,
        total: 1,
    });
    install_quack(&base::duckdb_path(app_data))?;
    on_progress(InstallProgress::Done { path: path.clone() });
    Ok(path)
}

fn duckdb_extension_command(bin: &Path) -> std::process::Command {
    let mut command = std::process::Command::new(bin);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    command.arg("-no-init");
    command
}

fn quack_is_loadable(bin: &Path) -> bool {
    if !bin.is_file() {
        return false;
    }
    duckdb_extension_command(bin)
        .arg(":memory:")
        .arg("-bail")
        .arg("-c")
        // The status probe must not silently contact a repository. It answers
        // only whether the extension previously installed by Setup can load.
        .arg("SET autoinstall_known_extensions = false; LOAD quack;")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn install_quack(bin: &Path) -> Result<(), String> {
    let output = duckdb_extension_command(bin)
        .arg(":memory:")
        .arg("-bail")
        .arg("-c")
        .arg("INSTALL quack; LOAD quack;")
        .output()
        .map_err(|error| format!("could not start DuckDB to install Quack: {error}"))?;
    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        "DuckDB could not install or load the Quack extension".into()
    };
    Err(detail)
}

use duckle_db_runner::cutover::{CutoverGate, EntryPointClass};
use duckle_db_runner::model::{
    RunCancellation, RunId, RunnerFailureReason, WorkerLease,
};
use duckle_db_runner::resources::RunnerResourcesProfile;
#[cfg(any(windows, test))]
use duckle_db_runner::resources::HostResourceLimits;
#[cfg(test)]
use duckle_db_runner::resources::{
    resolve_workspace_runner_resources, WorkspaceRunnerResources, WorkspaceRunnerResourcesError,
};
use duckle_db_runner::run_database::{PreviewResult, SqlBatchResult};
#[cfg(windows)]
use duckle_db_runner::sidecar_diagnostics::{
    append_desktop_diagnostic, desktop_sidecar_program_path, SidecarDiagnosticCode,
};
#[cfg(windows)]
use duckle_db_runner::worker_pool::{PoolError, WorkerPoolControl};
use duckle_duckdb_engine::OfficialRunnerController;
use serde::Deserialize;
#[cfg(windows)]
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
#[cfg(windows)]
use std::sync::OnceLock;

pub const SLOTHDB_DISABLED_DIAGNOSTIC: &str =
    "engine_disabled: SlothDB is temporarily disabled during the sidecar runner migration; no fallback engine will be selected";

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct WorkspaceEngineMetadata {
    engine: Option<String>,
}

pub fn workspace_engine_diagnostic(workspace: &Path) -> Result<Option<&'static str>, String> {
    let path = workspace.join("duckle.json");
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("workspace_engine_read_failed".to_string()),
    };
    let metadata: WorkspaceEngineMetadata = serde_json::from_slice(&bytes)
        .map_err(|_| "workspace_engine_parse_failed".to_string())?;
    Ok(metadata
        .engine
        .as_deref()
        .map(str::trim)
        .filter(|engine| engine.eq_ignore_ascii_case("slothdb"))
        .map(|_| SLOTHDB_DISABLED_DIAGNOSTIC))
}

#[derive(Default)]
struct DesktopControllerState {
    #[cfg(windows)]
    controllers: HashMap<PathBuf, Arc<DesktopWorkspaceController>>,
}

pub struct DesktopRunnerController {
    state: Mutex<DesktopControllerState>,
    sidecar_path: Option<PathBuf>,
    legacy_engine_disabled: Mutex<bool>,
}

impl DesktopRunnerController {
    pub fn new(sidecar_path: Option<PathBuf>) -> Self {
        #[cfg(windows)]
        let sidecar_path = resolve_desktop_sidecar_path(sidecar_path);

        #[cfg(windows)]
        append_desktop_diagnostic(
            sidecar_path.as_deref(),
            if sidecar_path.is_some() {
                SidecarDiagnosticCode::DesktopControllerReady
            } else {
                SidecarDiagnosticCode::DesktopSidecarMissing
            },
        );

        Self {
            state: Mutex::new(DesktopControllerState::default()),
            sidecar_path,
            legacy_engine_disabled: Mutex::new(false),
        }
    }

    #[cfg(test)]
    pub fn resources_for_workspace(
        &self,
        workspace: &Path,
    ) -> Result<WorkspaceRunnerResources, WorkspaceRunnerResourcesError> {
        resolve_workspace_runner_resources(workspace, HostResourceLimits::default())
    }

    pub fn controller_for_workspace(
        &self,
        workspace: &Path,
        profile: &RunnerResourcesProfile,
    ) -> Option<Arc<dyn OfficialRunnerController>> {
        let legacy_disabled = workspace_engine_diagnostic(workspace).ok().flatten().is_some();
        *self.legacy_engine_disabled.lock().ok()? = legacy_disabled;
        if legacy_disabled {
            return Some(Arc::new(DisabledLegacyEngineController));
        }

        if profile.validate().is_err() {
            #[cfg(windows)]
            append_desktop_diagnostic(
                self.sidecar_path.as_deref(),
                SidecarDiagnosticCode::DesktopProfileInvalid,
            );
            return None;
        }

        let Some(sidecar_path) = self.sidecar_path.as_ref().cloned() else {
            #[cfg(windows)]
            append_desktop_diagnostic(None, SidecarDiagnosticCode::DesktopSidecarMissing);
            return None;
        };

        #[cfg(windows)]
        {
            let key = workspace
                .canonicalize()
                .unwrap_or_else(|_| workspace.to_path_buf());
            let mut state = match self.state.lock() {
                Ok(state) => state,
                Err(_) => {
                    append_desktop_diagnostic(
                        Some(&sidecar_path),
                        SidecarDiagnosticCode::DesktopPoolCreateFailed,
                    );
                    return None;
                }
            };
            let controller = state
                .controllers
                .entry(key)
                .or_insert_with(|| {
                    Arc::new(DesktopWorkspaceController::new(
                        sidecar_path.clone(),
                        profile.clone(),
                    ))
                })
                .clone();
            drop(state);

            if controller.apply_requested_profile(profile.clone()).is_err() {
                append_desktop_diagnostic(
                    Some(&sidecar_path),
                    SidecarDiagnosticCode::DesktopProfileApplyFailed,
                );
                return None;
            }

            append_desktop_diagnostic(
                Some(&sidecar_path),
                SidecarDiagnosticCode::DesktopControllerReady,
            );
            Some(controller)
        }

        #[cfg(not(windows))]
        {
            let _ = (workspace, profile, sidecar_path);
            None
        }
    }

    pub fn apply_profile_if_active(
        &self,
        workspace: &Path,
        profile: &RunnerResourcesProfile,
    ) -> Result<(), String> {
        profile
            .validate()
            .map_err(|_| "invalid_runner_resources".to_string())?;

        #[cfg(windows)]
        {
            let key = workspace
                .canonicalize()
                .unwrap_or_else(|_| workspace.to_path_buf());
            let controller = self
                .state
                .lock()
                .map_err(|_| "runner_resources_apply_failed".to_string())?
                .controllers
                .get(&key)
                .cloned();
            if let Some(controller) = controller {
                controller
                    .apply_requested_profile(profile.clone())
                    .map_err(|_| "runner_resources_apply_failed".to_string())?;
            }
        }

        #[cfg(not(windows))]
        let _ = (workspace, profile);

        Ok(())
    }

    pub fn entry_point_class(&self) -> EntryPointClass {
        EntryPointClass::Production
    }

    pub fn cutover_gate(&self) -> CutoverGate {
        CutoverGate::Approved
    }

    #[cfg(test)]
    pub fn legacy_disabled_diagnostic(&self) -> Option<&'static str> {
        self.legacy_engine_disabled
            .lock()
            .ok()
            .filter(|disabled| **disabled)
            .map(|_| SLOTHDB_DISABLED_DIAGNOSTIC)
    }
}

#[cfg(windows)]
fn resolve_desktop_sidecar_path(staged: Option<PathBuf>) -> Option<PathBuf> {
    let adjacent = std::env::current_exe()
        .ok()
        .and_then(|executable| executable.parent().map(Path::to_path_buf))
        .map(|directory| directory.join("duckle-db-sidecar.exe"));
    let app_data = Some(desktop_sidecar_program_path());

    [staged, adjacent, app_data]
        .into_iter()
        .flatten()
        .find(|candidate| candidate.is_absolute() && candidate.is_file())
}

struct DisabledLegacyEngineController;

impl OfficialRunnerController for DisabledLegacyEngineController {
    fn acquire(
        &self,
        _run_id: RunId,
        _attempt: u32,
        _cancellation: RunCancellation,
        _now_millis: u64,
    ) -> Result<WorkerLease, RunnerFailureReason> {
        Err(RunnerFailureReason::RunnerUnavailable)
    }

    fn release(&self, _lease: WorkerLease, _now_millis: u64) {}

    fn execute_batch(
        &self,
        _lease: &WorkerLease,
        _statements: Vec<String>,
        _cancellation: RunCancellation,
    ) -> Result<SqlBatchResult, RunnerFailureReason> {
        Err(RunnerFailureReason::RunnerUnavailable)
    }

    fn preview_relation(
        &self,
        _lease: &WorkerLease,
        _sql: &str,
        _limit: u32,
        _cancellation: RunCancellation,
    ) -> Result<PreviewResult, RunnerFailureReason> {
        Err(RunnerFailureReason::RunnerUnavailable)
    }
}

#[cfg(windows)]
struct DesktopWorkspaceController {
    sidecar_path: PathBuf,
    requested_profile: Mutex<RunnerResourcesProfile>,
    pool: OnceLock<Result<Arc<WorkerPoolControl>, RunnerFailureReason>>,
}

#[cfg(windows)]
impl DesktopWorkspaceController {
    fn new(sidecar_path: PathBuf, requested_profile: RunnerResourcesProfile) -> Self {
        Self {
            sidecar_path,
            requested_profile: Mutex::new(requested_profile),
            pool: OnceLock::new(),
        }
    }

    fn pool(&self) -> Result<&Arc<WorkerPoolControl>, RunnerFailureReason> {
        match self.pool.get_or_init(|| self.create_pool()) {
            Ok(pool) => Ok(pool),
            Err(reason) => Err(*reason),
        }
    }

    fn create_pool(&self) -> Result<Arc<WorkerPoolControl>, RunnerFailureReason> {
        use duckle_db_runner::local_process_provider::LocalProcessProvider;
        use duckle_db_runner::local_quack_sidecar::WindowsLocalSidecarLauncher;

        append_desktop_diagnostic(
            Some(&self.sidecar_path),
            SidecarDiagnosticCode::DesktopPoolCreateStarted,
        );

        let result = (|| {
            let profile = self
                .requested_profile
                .lock()
                .map_err(|_| RunnerFailureReason::InvalidProfile)?
                .clone();
            let launcher = WindowsLocalSidecarLauncher::new(self.sidecar_path.clone())?;
            let provider = Arc::new(LocalProcessProvider::new(
                Arc::new(launcher),
                HostResourceLimits::default(),
            ));
            WorkerPoolControl::new(provider, profile, now_millis())
                .map(Arc::new)
                .map_err(pool_failure)
        })();

        append_desktop_diagnostic(
            Some(&self.sidecar_path),
            if result.is_ok() {
                SidecarDiagnosticCode::DesktopPoolReady
            } else {
                SidecarDiagnosticCode::DesktopPoolCreateFailed
            },
        );
        result
    }

    fn apply_requested_profile(
        &self,
        profile: RunnerResourcesProfile,
    ) -> Result<(), RunnerFailureReason> {
        profile
            .validate()
            .map_err(|_| RunnerFailureReason::InvalidProfile)?;
        let mut requested = self
            .requested_profile
            .lock()
            .map_err(|_| RunnerFailureReason::InvalidProfile)?;
        if *requested == profile {
            return Ok(());
        }
        if profile.version <= requested.version {
            return Err(RunnerFailureReason::InvalidProfile);
        }
        if let Some(Ok(pool)) = self.pool.get() {
            pool.set_desired_profile(profile.clone(), now_millis())
                .map_err(pool_failure)?;
        }
        *requested = profile;
        Ok(())
    }
}

#[cfg(windows)]
impl OfficialRunnerController for DesktopWorkspaceController {
    fn acquire(
        &self,
        run_id: RunId,
        attempt: u32,
        cancellation: RunCancellation,
        now_millis: u64,
    ) -> Result<WorkerLease, RunnerFailureReason> {
        self.pool()?
            .acquire_for_current_profile(run_id, attempt, cancellation, now_millis)
            .map_err(pool_failure)
    }

    fn release(&self, lease: WorkerLease, now_millis: u64) {
        if let Some(Ok(pool)) = self.pool.get() {
            let _ = pool.release(lease, now_millis);
        }
    }

    fn execute_batch(
        &self,
        lease: &WorkerLease,
        statements: Vec<String>,
        cancellation: RunCancellation,
    ) -> Result<SqlBatchResult, RunnerFailureReason> {
        self.pool()?
            .execute_database_batch(lease, statements, cancellation)
    }

    fn preview_relation(
        &self,
        lease: &WorkerLease,
        sql: &str,
        limit: u32,
        cancellation: RunCancellation,
    ) -> Result<PreviewResult, RunnerFailureReason> {
        self.pool()?
            .preview_database_relation(lease, sql, limit, cancellation)
    }
}

#[cfg(windows)]
fn pool_failure(error: PoolError) -> RunnerFailureReason {
    match error {
        PoolError::InvalidProfile => RunnerFailureReason::InvalidProfile,
        PoolError::Cancelled => RunnerFailureReason::Cancelled,
        PoolError::Provision(reason) => reason,
        PoolError::DuplicateRun | PoolError::UnknownLease | PoolError::ShuttingDown => {
            RunnerFailureReason::RunnerUnavailable
        }
    }
}

#[cfg(windows)]
fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use duckle_db_runner::resources::{AutomaticOrU16, ResourceLimit};

    #[test]
    fn missing_cli_is_reported_as_not_installed() {
        let app_data = tempfile::tempdir().unwrap();
        let statuses = status(app_data.path());
        let runner = statuses.iter().find(|status| status.id == "duckdb").unwrap();
        assert!(!runner.installed);
        assert!(runner.required);
        assert_eq!(runner.name, "DuckDB / Quack");
    }

    #[test]
    fn desktop_resolves_the_complete_workspace_profile() {
        let workspace = tempfile::tempdir().unwrap();
        let settings = workspace.path().join(".duckle");
        std::fs::create_dir_all(&settings).unwrap();
        std::fs::write(
            settings.join("settings.json"),
            r#"{"runner_resources":{"version":7,"memory":{"mode":"bytes","value":268435456},"cpuThreads":{"mode":"value","value":3},"spill":{"mode":"bytes","value":536870912},"quackParallelism":{"mode":"value","value":4},"baseCapacity":5}}"#,
        )
        .unwrap();

        let controller = DesktopRunnerController::new(None);
        let status = controller.resources_for_workspace(workspace.path()).unwrap();
        assert_eq!(status.requested.version, 7);
        assert_eq!(status.requested.memory, ResourceLimit::Bytes(268435456));
        assert_eq!(status.requested.cpu_threads, AutomaticOrU16::Value(3));
        assert_eq!(status.effective.requested_version, 7);
        assert_eq!(status.effective.effective_version, 7);
        assert_eq!(status.effective.quack_parallelism, 4);
        assert_eq!(status.effective.base_capacity, 5);
        assert_eq!(
            crate::app_settings::load_runner_resources(workspace.path()),
            status.requested
        );
    }

    #[test]
    fn persisted_slothdb_is_readable_but_forces_a_no_fallback_controller() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(
            workspace.path().join("duckle.json"),
            r#"{"version":2,"engine":"slothdb"}"#,
        )
        .unwrap();
        let original = std::fs::read_to_string(workspace.path().join("duckle.json")).unwrap();
        let controller = DesktopRunnerController::new(None);
        let runner = controller
            .controller_for_workspace(workspace.path(), &RunnerResourcesProfile::default())
            .expect("disabled workspace receives a rejecting controller without a sidecar");

        assert_eq!(
            controller.legacy_disabled_diagnostic(),
            Some(SLOTHDB_DISABLED_DIAGNOSTIC)
        );
        assert_eq!(
            runner.acquire(RunId::new(), 1, RunCancellation::default(), 0),
            Err(RunnerFailureReason::RunnerUnavailable)
        );
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("duckle.json")).unwrap(),
            original,
            "disabled-engine diagnostics must not rewrite the persisted engine"
        );
    }

    #[cfg(windows)]
    #[test]
    fn desktop_controller_registration_does_not_create_the_pool() {
        let sidecar_dir = tempfile::tempdir().unwrap();
        let sidecar = sidecar_dir.path().join("duckle-db-sidecar.exe");
        std::fs::write(&sidecar, b"test-sidecar").unwrap();

        let controller = DesktopRunnerController::new(Some(sidecar));
        let workspace = tempfile::tempdir().unwrap();
        let profile = RunnerResourcesProfile::default();
        let runner = controller
            .controller_for_workspace(workspace.path(), &profile)
            .expect("an existing staged sidecar registers a lazy controller");
        drop(runner);

        let state = controller.state.lock().unwrap();
        let registered = state.controllers.values().next().unwrap();
        assert!(registered.pool.get().is_none());
    }
}
