//! Deterministic ownership of sidecar process trees and run-scoped artifacts.
//!
//! Process providers place their platform containment handle behind
//! `ProcessTreeHandle`; the guard makes termination single-owner, idempotent,
//! and automatic on drop. Artifact cleanup is restricted to direct children
//! created by this module and carrying a matching marker.

use crate::model::RunId;
use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

const ARTIFACT_MARKER: &str = ".duckle-run-artifact.json";
const MAX_MARKER_BYTES: u64 = 4 * 1024;

/// Platform containment handles implement this with a Windows Job Object or
/// Unix process group. Implementations must terminate descendants as well as
/// the immediate child and may safely be called more than once.
pub trait ProcessTreeHandle: Send {
    fn terminate_tree(&mut self) -> io::Result<()>;
}

/// Single owner for one contained sidecar/runtime process tree.
pub struct ProcessTreeGuard {
    handle: Option<Box<dyn ProcessTreeHandle>>,
}

impl ProcessTreeGuard {
    pub fn new(handle: Box<dyn ProcessTreeHandle>) -> Self {
        Self {
            handle: Some(handle),
        }
    }

    pub fn terminate(&mut self) -> io::Result<()> {
        let Some(mut handle) = self.handle.take() else {
            return Ok(());
        };
        handle.terminate_tree()
    }

    pub fn is_terminated(&self) -> bool {
        self.handle.is_none()
    }
}

impl Drop for ProcessTreeGuard {
    fn drop(&mut self) {
        let _ = self.terminate();
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ArtifactMarker {
    run_id: RunId,
    created_unix_millis: u64,
}

/// Internal files for one run. Dropping the scope removes only the marked
/// directory this constructor created; persisted pipeline outputs must live
/// outside it.
pub struct RunArtifactScope {
    root: PathBuf,
    path: PathBuf,
    run_id: RunId,
    cleaned: bool,
}

impl RunArtifactScope {
    pub fn create(root: &Path, run_id: RunId, created_unix_millis: u64) -> io::Result<Self> {
        fs::create_dir_all(root)?;
        let root = fs::canonicalize(root)?;
        let path = root.join(directory_name(run_id));
        fs::create_dir(&path)?;
        let marker = ArtifactMarker {
            run_id,
            created_unix_millis,
        };
        let marker_path = path.join(ARTIFACT_MARKER);
        let mut marker_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&marker_path)?;
        if let Err(error) = serde_json::to_writer(&mut marker_file, &marker) {
            let _ = fs::remove_dir_all(&path);
            return Err(io::Error::new(io::ErrorKind::InvalidData, error));
        }
        Ok(Self {
            root,
            path,
            run_id,
            cleaned: false,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn cleanup(mut self) -> io::Result<()> {
        let result = remove_marked_directory(&self.root, &self.path, self.run_id);
        if result.is_ok() {
            self.cleaned = true;
        }
        result
    }
}

impl Drop for RunArtifactScope {
    fn drop(&mut self) {
        if !self.cleaned {
            let _ = remove_marked_directory(&self.root, &self.path, self.run_id);
            self.cleaned = true;
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SweepReport {
    pub removed: u32,
    pub retained: u32,
    pub rejected: u32,
}

/// Removes abandoned internal run directories older than `ttl`. Unmarked,
/// malformed, symlinked, future-dated, or nested paths are retained.
pub fn sweep_run_artifacts(
    root: &Path,
    now_unix_millis: u64,
    ttl: Duration,
) -> io::Result<SweepReport> {
    if !root.exists() {
        return Ok(SweepReport::default());
    }
    let root = fs::canonicalize(root)?;
    let ttl_millis = u64::try_from(ttl.as_millis()).unwrap_or(u64::MAX);
    let mut report = SweepReport::default();
    for entry in fs::read_dir(&root)? {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                report.rejected = report.rejected.saturating_add(1);
                continue;
            }
        };
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => {
                report.rejected = report.rejected.saturating_add(1);
                continue;
            }
        };
        if !file_type.is_dir() || file_type.is_symlink() {
            report.retained = report.retained.saturating_add(1);
            continue;
        }
        let path = entry.path();
        let marker = match read_marker(&path) {
            Ok(marker) if entry.file_name() == OsStr::new(&directory_name(marker.run_id)) => marker,
            _ => {
                report.rejected = report.rejected.saturating_add(1);
                continue;
            }
        };
        let age = now_unix_millis.checked_sub(marker.created_unix_millis);
        if age.is_none_or(|age| age < ttl_millis) {
            report.retained = report.retained.saturating_add(1);
            continue;
        }
        match remove_marked_directory(&root, &path, marker.run_id) {
            Ok(()) => report.removed = report.removed.saturating_add(1),
            Err(_) => report.rejected = report.rejected.saturating_add(1),
        }
    }
    Ok(report)
}

fn directory_name(run_id: RunId) -> String {
    format!("run-{run_id}")
}

fn read_marker(path: &Path) -> io::Result<ArtifactMarker> {
    let marker_path = path.join(ARTIFACT_MARKER);
    let metadata = fs::symlink_metadata(&marker_path)?;
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > MAX_MARKER_BYTES
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid run artifact marker",
        ));
    }
    let bytes = fs::read(marker_path)?;
    serde_json::from_slice(&bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn remove_marked_directory(root: &Path, path: &Path, expected_run_id: RunId) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "run artifact path is not a real directory",
        ));
    }
    let canonical = fs::canonicalize(path)?;
    if canonical.parent() != Some(root) || canonical.file_name() != path.file_name() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "run artifact path escaped its root",
        ));
    }
    let marker = read_marker(&canonical)?;
    if marker.run_id != expected_run_id
        || canonical.file_name() != Some(directory_name(marker.run_id).as_ref())
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "run artifact marker does not match its directory",
        ));
    }
    fs::remove_dir_all(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct RecordingTree(Arc<Mutex<u32>>);

    impl ProcessTreeHandle for RecordingTree {
        fn terminate_tree(&mut self) -> io::Result<()> {
            let mut calls = self.0.lock().unwrap();
            *calls += 1;
            Ok(())
        }
    }

    #[test]
    fn process_tree_termination_is_single_owner_and_idempotent() {
        let calls = Arc::new(Mutex::new(0));
        let mut guard = ProcessTreeGuard::new(Box::new(RecordingTree(calls.clone())));
        guard.terminate().unwrap();
        guard.terminate().unwrap();
        drop(guard);
        assert_eq!(*calls.lock().unwrap(), 1);
    }

    #[test]
    fn artifact_scope_removes_only_its_marked_run_directory() {
        let temp = tempfile::tempdir().unwrap();
        let unrelated = temp.path().join("keep-me");
        fs::create_dir(&unrelated).unwrap();
        let scope = RunArtifactScope::create(temp.path(), RunId::new(), 100).unwrap();
        fs::write(scope.path().join("spill.tmp"), b"internal").unwrap();
        let run_path = scope.path().to_path_buf();

        scope.cleanup().unwrap();

        assert!(!run_path.exists());
        assert!(unrelated.exists());
    }

    #[test]
    fn sweeper_removes_only_stale_valid_marked_directories() {
        let temp = tempfile::tempdir().unwrap();
        let stale = RunArtifactScope::create(temp.path(), RunId::new(), 100).unwrap();
        let stale_path = stale.path().to_path_buf();
        std::mem::forget(stale);
        let fresh = RunArtifactScope::create(temp.path(), RunId::new(), 950).unwrap();
        let fresh_path = fresh.path().to_path_buf();
        std::mem::forget(fresh);
        let unrelated = temp.path().join("run-untrusted");
        fs::create_dir(&unrelated).unwrap();

        let report = sweep_run_artifacts(temp.path(), 1_000, Duration::from_millis(500)).unwrap();

        assert_eq!(report.removed, 1);
        assert_eq!(report.retained, 1);
        assert_eq!(report.rejected, 1);
        assert!(!stale_path.exists());
        assert!(fresh_path.exists());
        assert!(unrelated.exists());
    }

    #[test]
    fn sweeper_rejects_a_marker_in_a_differently_named_directory() {
        let temp = tempfile::tempdir().unwrap();
        let run_id = RunId::new();
        let mismatched = temp.path().join("run-untrusted");
        fs::create_dir(&mismatched).unwrap();
        let marker = ArtifactMarker {
            run_id,
            created_unix_millis: 100,
        };
        fs::write(
            mismatched.join(ARTIFACT_MARKER),
            serde_json::to_vec(&marker).unwrap(),
        )
        .unwrap();

        let report = sweep_run_artifacts(temp.path(), 1_000, Duration::from_millis(500)).unwrap();

        assert_eq!(report, SweepReport {
            removed: 0,
            retained: 0,
            rejected: 1,
        });
        assert!(mismatched.exists());
    }
}
