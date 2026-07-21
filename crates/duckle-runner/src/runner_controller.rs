//! Headless ownership boundary for the packaged Quack runner.
//!
//! CLI, management-console, and web entry points share one controller per
//! workspace. There is no runtime backend selector: every run is assigned
//! through WorkerPoolControl and the packaged sidecar.

use duckle_db_runner::cutover::{CutoverGate, EntryPointClass};
#[cfg(windows)]
use duckle_db_runner::model::{RunCancellation, RunId, RunnerFailureReason, WorkerLease};
use duckle_db_runner::resources::{
    resolve_workspace_runner_resources, HostResourceLimits, RunnerResourcesProfile,
    WorkspaceRunnerResources, WorkspaceRunnerResourcesError,
};
#[cfg(windows)]
use duckle_db_runner::run_database::{PreviewResult, SqlBatchResult};
#[cfg(windows)]
use duckle_db_runner::worker_pool::{PoolError, WorkerPoolControl};
use duckle_duckdb_engine::{DuckdbEngine, OfficialRunnerController};
#[cfg(windows)]
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

#[cfg(windows)]
static CONTROLLERS: OnceLock<Mutex<HashMap<PathBuf, Arc<HeadlessController>>>> = OnceLock::new();

/// Build a headless engine using the single controller owned by this workspace.
/// The retained path argument is ignored while callers are cleaned up; it can no
/// longer select a binary or alter the runtime route.
pub(crate) fn engine_for_workspace(_retired_duckdb: PathBuf, workspace: &Path) -> DuckdbEngine {
    let base = DuckdbEngine::new(PathBuf::new());
    let resources = workspace_resources(workspace);
    if let Err(error) = &resources {
        eprintln!("duckle-runner: {error}");
    }

    let engine = resources
        .ok()
        .and_then(|resources| controller_for_workspace(workspace, &resources.requested))
        .map(|controller| base.with_official_runner_controller(controller))
        .unwrap_or(base);

    // Fixed internal adapter for the old engine API. select_runner() has one
    // possible result, so neither environment nor evidence can change routing.
    engine.with_runner_selection(EntryPointClass::Production, &CutoverGate::Approved)
}

pub(crate) fn workspace_resources(
    workspace: &Path,
) -> Result<WorkspaceRunnerResources, WorkspaceRunnerResourcesError> {
    resolve_workspace_runner_resources(workspace, HostResourceLimits::default())
}

#[cfg(windows)]
fn controller_for_workspace(
    workspace: &Path,
    profile: &RunnerResourcesProfile,
) -> Option<Arc<dyn OfficialRunnerController>> {
    let sidecar_path = resolve_sidecar_path()?;
    let workspace_key = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let controllers = CONTROLLERS.get_or_init(|| Mutex::new(HashMap::new()));

    if let Some(existing) = controllers
        .lock()
        .expect("headless controller registry poisoned")
        .get(&workspace_key)
        .cloned()
    {
        if existing.apply_requested_profile(profile.clone()).is_err() {
            return None;
        }
        return Some(existing);
    }

    let controller = Arc::new(HeadlessController::new(sidecar_path, profile.clone()));
    let mut registry = controllers
        .lock()
        .expect("headless controller registry poisoned");
    let controller = registry
        .entry(workspace_key)
        .or_insert_with(|| controller.clone())
        .clone();
    if controller.apply_requested_profile(profile.clone()).is_err() {
        return None;
    }
    Some(controller)
}

#[cfg(not(windows))]
fn controller_for_workspace(
    _workspace: &Path,
    _profile: &RunnerResourcesProfile,
) -> Option<Arc<dyn OfficialRunnerController>> {
    None
}

fn sidecar_name() -> &'static str {
    if cfg!(windows) {
        "duckle-db-sidecar.exe"
    } else {
        "duckle-db-sidecar"
    }
}

/// Only packaged locations are considered. Runtime environment overrides are
/// deliberately unsupported.
fn resolve_sidecar_path() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let directory = executable.parent()?;
    [
        directory.join(sidecar_name()),
        directory
            .parent()
            .map(|engines| engines.join("db-sidecar").join(sidecar_name()))?,
    ]
    .into_iter()
    .find_map(absolute_existing_file)
}

fn absolute_existing_file(path: PathBuf) -> Option<PathBuf> {
    let absolute = if path.is_absolute() {
        path
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    absolute
        .canonicalize()
        .ok()
        .filter(|candidate| candidate.is_file())
}

#[cfg(windows)]
struct HeadlessController {
    sidecar_path: PathBuf,
    requested_profile: Mutex<RunnerResourcesProfile>,
    pool: OnceLock<Result<Arc<WorkerPoolControl>, RunnerFailureReason>>,
}

#[cfg(windows)]
impl HeadlessController {
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
impl OfficialRunnerController for HeadlessController {
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
    use duckle_duckdb_engine::ExecutionRoute;

    #[test]
    fn headless_engine_has_only_the_quack_route() {
        let engine = engine_for_workspace(PathBuf::new(), Path::new("."));
        assert_eq!(engine.execution_route(), ExecutionRoute::OfficialRunner);
    }

    #[test]
    fn missing_relative_sidecar_is_rejected() {
        assert!(absolute_existing_file(PathBuf::from(
            "definitely-missing-duckle-sidecar"
        ))
        .is_none());
    }

    #[test]
    fn headless_resolves_the_complete_workspace_profile() {
        let workspace = tempfile::tempdir().unwrap();
        let settings = workspace.path().join(".duckle");
        std::fs::create_dir_all(&settings).unwrap();
        std::fs::write(
            settings.join("settings.json"),
            r#"{"runner_resources":{"version":7,"memory":{"mode":"bytes","value":268435456},"cpuThreads":{"mode":"value","value":3},"spill":{"mode":"bytes","value":536870912},"quackParallelism":{"mode":"value","value":4},"baseCapacity":5}}"#,
        )
        .unwrap();

        let status = workspace_resources(workspace.path()).unwrap();
        assert_eq!(status.requested.version, 7);
        assert_eq!(status.requested.memory, ResourceLimit::Bytes(268435456));
        assert_eq!(status.requested.cpu_threads, AutomaticOrU16::Value(3));
        assert_eq!(status.effective.requested_version, 7);
        assert_eq!(status.effective.effective_version, 7);
        assert_eq!(status.effective.quack_parallelism, 4);
        assert_eq!(status.effective.base_capacity, 5);
    }
}
