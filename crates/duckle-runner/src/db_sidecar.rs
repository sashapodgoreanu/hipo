#![cfg_attr(windows, windows_subsystem = "windows")]

//! Packaged local database sidecar entrypoint.
//!
//! The Quack extension is embedded only after build-time checksum validation.
//! At process start it is staged into a private versioned directory beside the
//! sidecar, then the provider-private bootstrap protocol starts the loopback-only
//! server. DuckDB's global user extension cache is never modified.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const EMBEDDED_QUACK_EXTENSION: &[u8] =
    include_bytes!(env!("DUCKLE_EMBEDDED_QUACK_EXTENSION"));
const EXTENSION_STAGE_LOCK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const EXTENSION_STAGE_LOCK_STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(30);
static EXTENSION_STAGE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn write_failure_marker(code: &str) {
    use std::io::Write;

    let Some(path) = std::env::current_exe()
        .ok()
        .map(|executable| executable.with_extension("log"))
    else {
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
        // Only stable stage identifiers are persisted. Never write command-line
        // arguments, endpoints, paths, SQL, credentials, PIDs or raw errors.
        let _ = writeln!(file, "{timestamp_ms} {code}");
    }
}

fn extension_path() -> Result<PathBuf, String> {
    let executable = std::env::current_exe().map_err(|_| "runner_unavailable".to_string())?;
    let platform = duckle_db_runner::bundle::BundlePlatform::current()
        .ok_or_else(|| "runner_unavailable".to_string())?;
    duckle_db_runner::bundle::packaged_extension_path(&executable, platform)
        .map_err(|_| "runner_unavailable".to_string())
}

struct ExtensionStageLock {
    path: PathBuf,
}

impl Drop for ExtensionStageLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn file_matches(path: &Path, bytes: &[u8]) -> bool {
    std::fs::read(path)
        .map(|existing| existing == bytes)
        .unwrap_or(false)
}

fn lock_is_stale(path: &Path) -> bool {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .and_then(|modified| modified.elapsed().map_err(std::io::Error::other))
        .map(|age| age >= EXTENSION_STAGE_LOCK_STALE_AFTER)
        .unwrap_or(false)
}

fn acquire_extension_stage_lock(
    path: &Path,
    bytes: &[u8],
) -> Result<Option<ExtensionStageLock>, String> {
    let lock_path = path.with_extension("lock");
    let deadline = std::time::Instant::now() + EXTENSION_STAGE_LOCK_TIMEOUT;

    loop {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(_) => return Ok(Some(ExtensionStageLock { path: lock_path })),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                // Another freshly-started warm/on-demand worker may already have
                // completed the identical private staging operation.
                if file_matches(path, bytes) {
                    return Ok(None);
                }
                if lock_is_stale(&lock_path) {
                    let _ = std::fs::remove_file(&lock_path);
                    continue;
                }
                if std::time::Instant::now() >= deadline {
                    return Err("runner_unavailable".to_string());
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(_) => return Err("runner_unavailable".to_string()),
        }
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "runner_unavailable".to_string())?;
    std::fs::create_dir_all(parent).map_err(|_| "runner_unavailable".to_string())?;

    let Some(_lock) = acquire_extension_stage_lock(path, bytes)? else {
        return Ok(());
    };

    // Re-check after acquiring the cross-process lock. A competing sidecar may
    // have completed between the caller's first read and lock acquisition.
    if file_matches(path, bytes) {
        return Ok(());
    }

    // Never delete or replace an extension already present at the private,
    // versioned package path. A mismatch fails closed and leaves the file intact.
    if path.exists() {
        return Err("runner_unavailable".to_string());
    }

    let sequence = EXTENSION_STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temp = path.with_extension(format!("part{}-{sequence}", std::process::id()));
    std::fs::write(&temp, bytes).map_err(|_| "runner_unavailable".to_string())?;

    match std::fs::rename(&temp, path) {
        Ok(()) => Ok(()),
        Err(_) if file_matches(path, bytes) => {
            // Another process won the create race with the same verified bytes.
            let _ = std::fs::remove_file(&temp);
            Ok(())
        }
        Err(_) => {
            // Preserve whatever appeared at the final path. Only our private
            // temporary file is removed.
            let _ = std::fs::remove_file(&temp);
            Err("runner_unavailable".to_string())
        }
    }
}

fn stage_embedded_extension() -> Result<PathBuf, String> {
    if EMBEDDED_QUACK_EXTENSION.is_empty() {
        return Err("runner_unavailable".to_string());
    }
    let path = extension_path()?;
    if !file_matches(&path, EMBEDDED_QUACK_EXTENSION) {
        write_atomic(&path, EMBEDDED_QUACK_EXTENSION)?;
    }

    let platform = duckle_db_runner::bundle::BundlePlatform::current()
        .ok_or_else(|| "runner_unavailable".to_string())?;
    let expected = duckle_db_runner::bundle::bundle_for(platform)
        .ok_or_else(|| "runner_unavailable".to_string())?;
    duckle_db_runner::bundle::verify_staged_bundle(&path, expected)
        .map_err(|_| "runner_unavailable".to_string())?;
    Ok(path)
}

#[cfg(windows)]
fn run() -> Result<(), String> {
    let extension = stage_embedded_extension().map_err(|reason| {
        write_failure_marker("sidecar.extension_stage_failed");
        reason
    })?;
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    duckle_db_runner::local_quack_sidecar::run_windows_sidecar(&args, &extension).map_err(|_| {
        write_failure_marker("sidecar.bootstrap_or_quack_failed");
        "runner_unavailable".to_string()
    })
}

#[cfg(not(windows))]
fn run() -> Result<(), String> {
    // Packaging exists for all approved targets, while process-launch support
    // remains gated until the platform-specific containment implementation is
    // selected. Never fall back to a CLI or network install.
    let _extension = stage_embedded_extension().map_err(|reason| {
        write_failure_marker("sidecar.extension_stage_failed");
        reason
    })?;
    write_failure_marker("sidecar.platform_unavailable");
    Err("runner_unavailable".to_string())
}

fn main() {
    if let Err(reason) = run() {
        eprintln!("{reason}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn approved_extension_path_is_private_versioned_and_platform_scoped() {
        if duckle_db_runner::bundle::BundlePlatform::current().is_none() {
            return;
        }
        let path = extension_path().unwrap();
        let text = path.to_string_lossy();
        assert!(text.contains("extensions"));
        assert!(text.contains("v1.5.4"));
        assert!(text.ends_with(duckle_db_runner::bundle::QUACK_EXTENSION_FILE));
        assert!(!text.contains(".duckdb"));
    }

    #[test]
    fn concurrent_identical_staging_keeps_one_complete_extension() {
        let directory = tempfile::tempdir().unwrap();
        let path = Arc::new(directory.path().join("quack.duckdb_extension"));
        let payload = Arc::new(vec![0x51; 128 * 1024]);
        let mut workers = Vec::new();

        for _ in 0..8 {
            let path = path.clone();
            let payload = payload.clone();
            workers.push(std::thread::spawn(move || {
                write_atomic(&path, &payload).unwrap();
            }));
        }
        for worker in workers {
            worker.join().unwrap();
        }

        assert_eq!(
            std::fs::read(path.as_ref()).unwrap(),
            payload.as_ref().as_slice()
        );
        assert!(!path.with_extension("lock").exists());
    }

    #[test]
    fn existing_different_extension_is_never_replaced() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("quack.duckdb_extension");
        let existing = b"existing-extension";
        std::fs::write(&path, existing).unwrap();

        assert_eq!(
            write_atomic(&path, b"embedded-extension"),
            Err("runner_unavailable".to_string())
        );
        assert_eq!(std::fs::read(&path).unwrap(), existing);
    }

    #[test]
    fn failure_marker_codes_are_static_and_non_sensitive() {
        for code in [
            "sidecar.extension_stage_failed",
            "sidecar.bootstrap_or_quack_failed",
            "sidecar.platform_unavailable",
        ] {
            assert!(code.starts_with("sidecar."));
            assert!(!code.contains(['\\', '/', ':', '=', ' ']));
        }
    }
}
