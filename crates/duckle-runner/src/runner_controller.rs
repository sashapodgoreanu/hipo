//! Headless ownership boundary for the official database runner.
//!
//! CLI, management-console, and web entry points build their engines through
//! this module. The concrete pool is intentionally lazy: attaching a controller
//! before cutover must not provision warm workers while production selection is
//! still on the compatibility route.

use duckle_db_runner::cutover::{
    configured_entry_point_class, packaged_cutover_gate, CutoverGate,
};
#[cfg(windows)]
use duckle_db_runner::model::{
    RunCancellation, RunId, RunnerFailureReason, WorkerLease,
};
#[cfg(windows)]
use duckle_db_runner::resources::{HostResourceLimits, RunnerResourcesProfile};
#[cfg(windows)]
use duckle_db_runner::run_database::{PreviewResult, SqlBatchResult};
#[cfg(windows)]
use duckle_db_runner::worker_pool::{PoolError, WorkerPoolControl};
use duckle_duckdb_engine::{DuckdbEngine, OfficialRunnerController};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

static CONTROLLERS: OnceLock<Mutex<HashMap<PathBuf, Arc<dyn OfficialRunnerController>>>> =
    OnceLock::new();
static GATE_DIAGNOSTIC_REPORTED: OnceLock<()> = OnceLock::new();

/// Build a headless engine using the single controller owned by this workspace
/// process. Repeated manual, scheduled, and browser runs resolve the same
/// controller rather than creating an entry-point-specific allocation path.
///
/// The packaged evidence gate retains CLI compatibility until an approved
/// manifest is embedded in the build. The controller and its worker pool are
/// lazy, so merely constructing this engine never starts a sidecar process.
pub(crate) fn engine_for_workspace(duckdb: PathBuf, workspace: &Path) -> DuckdbEngine {
    let base = DuckdbEngine::new(duckdb);
    let with_controller = controller_for_workspace(workspace)
        .map(|controller| base.with_official_runner_controller(controller))
        .unwrap_or(base);
    let gate = packaged_cutover_gate();
    report_gate_once(&gate);
    with_controller.with_runner_selection(configured_entry_point_class(), &gate)
}

fn report_gate_once(gate: &CutoverGate) {
    if let CutoverGate::Rejected { missing_or_failed } = gate {
        GATE_DIAGNOSTIC_REPORTED.get_or_init(|| {
            eprintln!(
                "duckle-runner: official runner gate rejected ({})",
                missing_or_failed.join(",")
            );
        });
    }
}

fn controller_for_workspace(workspace: &Path) -> Option<Arc<dyn OfficialRunnerController>> {
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
        return Some(existing);
    }

    let controller = lazy_controller(sidecar_path)?;
    let mut registry = controllers
        .lock()
        .expect("headless controller registry poisoned");
    Some(
        registry
            .entry(workspace_key)
            .or_insert_with(|| controller.clone())
            .clone(),
    )
}

fn sidecar_name() -> &'static str {
    if cfg!(windows) {
        "duckle-db-sidecar.exe"
    } else {
        "duckle-db-sidecar"
    }
}

fn resolve_sidecar_path() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = std::env::var_os("DUCKLE_DB_SIDECAR_BIN") {
        candidates.push(PathBuf::from(path));
    }
    if let Ok(executable) = std::env::current_exe() {
        if let Some(directory) = executable.parent() {
            candidates.push(directory.join(sidecar_name()));
            if let Some(engines_directory) = directory.parent() {
                candidates.push(
                    engines_directory
                        .join("db-sidecar")
                        .join(sidecar_name()),
                );
            }
        }
    }
    candidates.into_iter().find_map(absolute_existing_file)
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
fn lazy_controller(path: PathBuf) -> Option<Arc<dyn OfficialRunnerController>> {
    Some(Arc::new(LazyHeadlessController {
        sidecar_path: path,
        pool: OnceLock::new(),
    }))
}

#[cfg(not(windows))]
fn lazy_controller(_path: PathBuf) -> Option<Arc<dyn OfficialRunnerController>> {
    // Packaging is present for all approved targets, while platform-specific
    // process containment is still gated. Never substitute a direct spawn.
    None
}

#[cfg(windows)]
struct LazyHeadlessController {
    sidecar_path: PathBuf,
    pool: OnceLock<Result<Arc<WorkerPoolControl>, RunnerFailureReason>>,
}

#[cfg(windows)]
impl LazyHeadlessController {
    fn pool(&self) -> Result<&Arc<WorkerPoolControl>, RunnerFailureReason> {
        match self.pool.get_or_init(|| self.create_pool()) {
            Ok(pool) => Ok(pool),
            Err(reason) => Err(*reason),
        }
    }

    fn create_pool(&self) -> Result<Arc<WorkerPoolControl>, RunnerFailureReason> {
        use duckle_db_runner::local_process_provider::LocalProcessProvider;
        use duckle_db_runner::local_quack_sidecar::WindowsLocalSidecarLauncher;

        let launcher = WindowsLocalSidecarLauncher::new(self.sidecar_path.clone())?;
        let provider = Arc::new(LocalProcessProvider::new(
            Arc::new(launcher),
            HostResourceLimits::default(),
        ));
        WorkerPoolControl::new(
            provider,
            RunnerResourcesProfile::default(),
            now_millis(),
        )
        .map(Arc::new)
        .map_err(pool_failure)
    }
}

#[cfg(windows)]
impl OfficialRunnerController for LazyHeadlessController {
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
    use duckle_duckdb_engine::ExecutionRoute;

    #[test]
    fn production_headless_engine_stays_compatible_before_cutover() {
        let engine = engine_for_workspace(PathBuf::from("duckdb"), Path::new("."));
        assert_eq!(engine.execution_route(), ExecutionRoute::CliCompatibility);
    }

    #[test]
    fn relative_sidecar_override_is_normalized_only_when_it_exists() {
        let missing = PathBuf::from("definitely-missing-duckle-sidecar");
        assert!(absolute_existing_file(missing).is_none());
    }
}
