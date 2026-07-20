//! Engine installation manager and immutable official-runner artifact pin.
//!
//! The existing installation implementation remains isolated in the base
//! module. This wrapper owns the DuckDB/Quack pair metadata, performs offline
//! checksum verification, and provides the real lazy per-workspace controller.

#[path = "engine_manager_base.rs"]
mod base;
pub use base::*;

use duckle_db_runner::cutover::{
    configured_entry_point_class, packaged_cutover_gate, CutoverGate, EntryPointClass,
};
use duckle_db_runner::model::{
    RunCancellation, RunId, RunnerFailureReason, WorkerLease,
};
use duckle_db_runner::resources::{
    resolve_workspace_runner_resources, HostResourceLimits, RunnerResourcesProfile,
    WorkspaceRunnerResources, WorkspaceRunnerResourcesError,
};
use duckle_db_runner::run_database::{PreviewResult, SqlBatchResult};
#[cfg(windows)]
use duckle_db_runner::worker_pool::{PoolError, WorkerPoolControl};
use duckle_duckdb_engine::OfficialRunnerController;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

pub const QUACK_VERSION: &str = DUCKDB_VERSION;
pub const QUACK_LICENSE: &str = "MIT";
pub const QUACK_PROVENANCE: &str = "duckdb/duckdb-quack";
pub const QUACK_EXTENSION_FILE: &str = "quack.duckdb_extension";
pub const SLOTHDB_DISABLED_DIAGNOSTIC: &str =
    "engine_disabled: SlothDB is temporarily disabled during the sidecar runner migration; no fallback engine will be selected";

const QUACK_WINDOWS_AMD64_SHA256: &str =
    "52d20e78a0498c721fb0764e94d8e5b287fded3d8fcf6e95365cb03e5905b895";
const QUACK_MACOS_AMD64_SHA256: &str =
    "85a48992d0b940f7cf1c55bbe4efd02f46c9724b67e238a990df3f3244d8e970";
const QUACK_LINUX_AMD64_SHA256: &str =
    "decb78a4d953ff9cc65c300cf2c8d3f3d8f4732851205684565c922113bc2b9e";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficialRunnerPin {
    pub duckdb_version: &'static str,
    pub quack_version: &'static str,
    pub quack_sha256: &'static str,
    pub license: &'static str,
    pub provenance: &'static str,
    pub extension_file: &'static str,
}

/// Return the only approved DuckDB/Quack pair for a supported release target.
/// Unsupported targets stay unavailable rather than downloading an unpinned
/// extension at build or run time.
pub fn official_runner_pin_for(os: &str, arch: &str) -> Option<OfficialRunnerPin> {
    let quack_sha256 = match (os, arch) {
        ("windows", "x86_64") => QUACK_WINDOWS_AMD64_SHA256,
        ("macos", "x86_64") => QUACK_MACOS_AMD64_SHA256,
        ("linux", "x86_64") => QUACK_LINUX_AMD64_SHA256,
        _ => return None,
    };
    Some(OfficialRunnerPin {
        duckdb_version: DUCKDB_VERSION,
        quack_version: QUACK_VERSION,
        quack_sha256,
        license: QUACK_LICENSE,
        provenance: QUACK_PROVENANCE,
        extension_file: QUACK_EXTENSION_FILE,
    })
}

/// Verify an already staged Quack extension entirely offline. This function
/// never installs, downloads, or accepts a provider-supplied checksum.
pub fn verify_offline_quack_extension(
    path: &Path,
    os: &str,
    arch: &str,
) -> Result<OfficialRunnerPin, String> {
    let pin = official_runner_pin_for(os, arch)
        .ok_or_else(|| format!("official_runner.unsupported_target:{os}-{arch}"))?;
    let bytes = std::fs::read(path)
        .map_err(|_| "official_runner.extension_read_failed".to_string())?;
    let actual = format!("{:x}", Sha256::digest(&bytes));
    if actual != pin.quack_sha256 {
        return Err("official_runner.extension_checksum_mismatch".to_string());
    }
    Ok(pin)
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct WorkspaceEngineMetadata {
    engine: Option<String>,
}

/// Read the persisted workspace engine without rewriting it. SlothDB remains a
/// valid stored value, but execution receives an explicit no-fallback diagnostic.
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

/// Process-wide desktop registry. It owns one lazy controller per workspace,
/// while each controller owns at most one WorkerPoolControl. Registering the
/// staged sidecar before cutover does not start a process or create warm workers.
pub struct DesktopRunnerController {
    state: Mutex<DesktopControllerState>,
    sidecar_path: Option<PathBuf>,
    legacy_engine_disabled: Mutex<bool>,
}

impl DesktopRunnerController {
    pub fn new(sidecar_path: Option<PathBuf>) -> Self {
        Self {
            state: Mutex::new(DesktopControllerState::default()),
            sidecar_path,
            legacy_engine_disabled: Mutex::new(false),
        }
    }

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
            // This controller exists solely to prevent the production compatibility
            // selector from falling through to DuckDB CLI for a SlothDB workspace.
            return Some(Arc::new(DisabledLegacyEngineController));
        }

        profile.validate().ok()?;
        let sidecar_path = self.sidecar_path.as_ref()?.clone();

        #[cfg(windows)]
        {
            let key = workspace
                .canonicalize()
                .unwrap_or_else(|_| workspace.to_path_buf());
            let mut state = self.state.lock().ok()?;
            let controller = state
                .controllers
                .entry(key)
                .or_insert_with(|| {
                    Arc::new(DesktopWorkspaceController::new(
                        sidecar_path,
                        profile.clone(),
                    ))
                })
                .clone();
            drop(state);
            controller.apply_requested_profile(profile.clone()).ok()?;
            Some(controller)
        }

        #[cfg(not(windows))]
        {
            let _ = (workspace, profile, sidecar_path);
            None
        }
    }

    /// Apply a saved generation without waiting for another pipeline run. If the
    /// lazy controller has not been registered yet, persistence remains the
    /// authority and the first registration will read that profile.
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
        configured_entry_point_class()
    }

    pub fn cutover_gate(&self) -> CutoverGate {
        if self
            .legacy_engine_disabled
            .lock()
            .map(|disabled| *disabled)
            .unwrap_or(true)
        {
            // Force the engine onto the rejecting controller. Compatibility is a
            // DuckDB fallback and is therefore forbidden for persisted SlothDB.
            CutoverGate::Approved
        } else {
            packaged_cutover_gate()
        }
    }

    pub fn legacy_disabled_diagnostic(&self) -> Option<&'static str> {
        self.legacy_engine_disabled
            .lock()
            .ok()
            .filter(|disabled| **disabled)
            .map(|_| SLOTHDB_DISABLED_DIAGNOSTIC)
    }
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

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod runner_pin_tests {
    use super::*;
    use duckle_db_runner::resources::{AutomaticOrU16, ResourceLimit};

    #[test]
    fn official_runner_pin_is_atomic_and_records_provenance() {
        let pin = official_runner_pin_for("windows", "x86_64").unwrap();
        assert_eq!(pin.duckdb_version, "1.5.4");
        assert_eq!(pin.quack_version, pin.duckdb_version);
        assert_eq!(pin.license, "MIT");
        assert_eq!(pin.provenance, "duckdb/duckdb-quack");
        assert_eq!(pin.extension_file, "quack.duckdb_extension");
        assert_eq!(pin.quack_sha256.len(), 64);
        assert!(official_runner_pin_for("windows", "aarch64").is_none());
    }

    #[test]
    fn offline_verification_rejects_unpinned_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let extension = dir.path().join(QUACK_EXTENSION_FILE);
        std::fs::write(&extension, b"not-the-pinned-extension").unwrap();

        assert_eq!(
            verify_offline_quack_extension(&extension, "windows", "x86_64"),
            Err("official_runner.extension_checksum_mismatch".to_string())
        );
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
        let official = controller
            .controller_for_workspace(workspace.path(), &RunnerResourcesProfile::default())
            .expect("disabled workspace receives a rejecting controller without a sidecar");

        assert_eq!(controller.cutover_gate(), CutoverGate::Approved);
        assert_eq!(
            controller.legacy_disabled_diagnostic(),
            Some(SLOTHDB_DISABLED_DIAGNOSTIC)
        );
        assert_eq!(
            official.acquire(RunId::new(), 1, RunCancellation::default(), 0),
            Err(RunnerFailureReason::RunnerUnavailable)
        );
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("duckle.json")).unwrap(),
            original,
            "compatibility diagnostics must not rewrite the persisted engine"
        );
    }

    #[test]
    fn slothdb_install_is_rejected_before_download_or_fallback() {
        let app_data = tempfile::tempdir().unwrap();
        let error = install(app_data.path(), "slothdb", |_| {})
            .expect_err("SlothDB install remains disabled");
        assert!(error.contains("engine_disabled"), "{error}");
        assert!(error.contains("SlothDB"), "{error}");
        assert!(!app_data.path().join("engines").join("slothdb").exists());
    }

    #[cfg(windows)]
    #[test]
    fn desktop_controller_registration_does_not_create_the_pool() {
        let controller = DesktopRunnerController::new(Some(PathBuf::from("missing-sidecar.exe")));
        let workspace = tempfile::tempdir().unwrap();
        let profile = RunnerResourcesProfile::default();
        let official = controller
            .controller_for_workspace(workspace.path(), &profile)
            .expect("lazy controller registration does not inspect or launch the sidecar");
        drop(official);

        let state = controller.state.lock().unwrap();
        let registered = state.controllers.values().next().unwrap();
        assert!(registered.pool.get().is_none());
    }
}
