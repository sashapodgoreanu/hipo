//! Engine installation manager and immutable official-runner artifact pin.
//!
//! The existing installation implementation remains isolated in the base
//! module. This wrapper owns the DuckDB/Quack pair metadata, performs offline
//! checksum verification, and provides the real per-workspace runner controller.

#[path = "engine_manager_base.rs"]
mod base;
pub use base::*;

use duckle_db_runner::cutover::{
    configured_entry_point_class, packaged_cutover_gate, CutoverGate, EntryPointClass,
};
use duckle_db_runner::resources::{
    resolve_workspace_runner_resources, HostResourceLimits, RunnerResourcesProfile,
    WorkspaceRunnerResources, WorkspaceRunnerResourcesError,
};
use duckle_db_runner::worker_pool::WorkerPoolControl;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub const QUACK_VERSION: &str = DUCKDB_VERSION;
pub const QUACK_LICENSE: &str = "MIT";
pub const QUACK_PROVENANCE: &str = "duckdb/duckdb-quack";
pub const QUACK_EXTENSION_FILE: &str = "quack.duckdb_extension";

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

#[derive(Default)]
struct DesktopControllerState {
    pools: HashMap<PathBuf, Arc<WorkerPoolControl>>,
    profiles: HashMap<PathBuf, RunnerResourcesProfile>,
}

/// Per-workspace controller used by the desktop shell. A staged sidecar is
/// converted into one real WorkerPoolControl for each workspace. The profile is
/// not copied into PipelineDoc and a later saved generation updates the same
/// pool immediately.
pub struct DesktopRunnerController {
    state: Mutex<DesktopControllerState>,
    sidecar_path: Option<PathBuf>,
}

impl DesktopRunnerController {
    pub fn new(sidecar_path: Option<PathBuf>) -> Self {
        Self {
            state: Mutex::new(DesktopControllerState::default()),
            sidecar_path,
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
    ) -> Option<Arc<WorkerPoolControl>> {
        profile.validate().ok()?;
        let key = workspace
            .canonicalize()
            .unwrap_or_else(|_| workspace.to_path_buf());

        {
            let mut state = self.state.lock().ok()?;
            if let Some(pool) = state.pools.get(&key).cloned() {
                if apply_profile_locked(&mut state, &key, profile, &pool).is_err() {
                    return None;
                }
                return Some(pool);
            }
        }

        let sidecar_path = self.sidecar_path.as_ref()?.clone();

        #[cfg(windows)]
        {
            use duckle_db_runner::local_process_provider::LocalProcessProvider;
            use duckle_db_runner::local_quack_sidecar::WindowsLocalSidecarLauncher;

            let launcher = WindowsLocalSidecarLauncher::new(sidecar_path).ok()?;
            let provider = Arc::new(LocalProcessProvider::new(
                Arc::new(launcher),
                HostResourceLimits::default(),
            ));
            let candidate = Arc::new(
                WorkerPoolControl::new(provider, profile.clone(), now_millis()).ok()?,
            );
            let mut state = self.state.lock().ok()?;
            if let Some(existing) = state.pools.get(&key).cloned() {
                if apply_profile_locked(&mut state, &key, profile, &existing).is_err() {
                    return None;
                }
                return Some(existing);
            }
            state.profiles.insert(key.clone(), profile.clone());
            state.pools.insert(key, candidate.clone());
            Some(candidate)
        }

        #[cfg(not(windows))]
        {
            let _ = sidecar_path;
            None
        }
    }

    /// Apply a saved generation without waiting for another pipeline run. If the
    /// workspace has not created a pool yet, persistence remains authoritative
    /// and the first pool will start with that profile.
    pub fn apply_profile_if_active(
        &self,
        workspace: &Path,
        profile: &RunnerResourcesProfile,
    ) -> Result<(), String> {
        profile
            .validate()
            .map_err(|_| "invalid_runner_resources".to_string())?;
        let key = workspace
            .canonicalize()
            .unwrap_or_else(|_| workspace.to_path_buf());
        let mut state = self
            .state
            .lock()
            .map_err(|_| "runner_resources_apply_failed".to_string())?;
        let Some(pool) = state.pools.get(&key).cloned() else {
            return Ok(());
        };
        apply_profile_locked(&mut state, &key, profile, &pool)
    }

    pub fn entry_point_class(&self) -> EntryPointClass {
        configured_entry_point_class()
    }

    pub fn cutover_gate(&self) -> CutoverGate {
        packaged_cutover_gate()
    }
}

fn apply_profile_locked(
    state: &mut DesktopControllerState,
    key: &Path,
    profile: &RunnerResourcesProfile,
    pool: &Arc<WorkerPoolControl>,
) -> Result<(), String> {
    if let Some(current) = state.profiles.get(key) {
        if current == profile {
            return Ok(());
        }
        if profile.version <= current.version {
            return Err("invalid_runner_resources".to_string());
        }
    }
    pool.set_desired_profile(profile.clone(), now_millis())
        .map_err(|_| "runner_resources_apply_failed".to_string())?;
    state.profiles.insert(key.to_path_buf(), profile.clone());
    Ok(())
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
}
