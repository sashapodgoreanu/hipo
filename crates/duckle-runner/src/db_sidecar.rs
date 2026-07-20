//! Packaged local database sidecar entrypoint.
//!
//! The Quack extension is embedded only after build-time checksum validation.
//! At process start it is staged into DuckDB's versioned extension cache, then
//! the provider-private bootstrap protocol starts the loopback-only server.

use std::path::{Path, PathBuf};

const EMBEDDED_QUACK_EXTENSION: &[u8] =
    include_bytes!(env!("DUCKLE_EMBEDDED_QUACK_EXTENSION"));
const DUCKDB_VERSION: &str = env!("DUCKLE_RUNNER_DUCKDB_VERSION");
const QUACK_EXTENSION_FILE: &str = env!("DUCKLE_QUACK_EXTENSION_FILE");
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

fn duckdb_home() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

fn repository_platform() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Some("windows_amd64"),
        ("macos", "x86_64") => Some("osx_amd64"),
        ("linux", "x86_64") => Some("linux_amd64"),
        _ => None,
    }
}

fn extension_cache_path() -> Result<PathBuf, String> {
    let home = duckdb_home().ok_or_else(|| {
        debug_log("extension.cache.home_missing");
        "runner_unavailable".to_string()
    })?;
    let platform = repository_platform().ok_or_else(|| {
        debug_log("extension.cache.platform_unsupported");
        "runner_unavailable".to_string()
    })?;
    Ok(home
        .join(".duckdb")
        .join("extensions")
        .join(format!("v{DUCKDB_VERSION}"))
        .join(platform)
        .join(QUACK_EXTENSION_FILE))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or_else(|| {
        debug_log("extension.cache.parent_missing");
        "runner_unavailable".to_string()
    })?;
    std::fs::create_dir_all(parent).map_err(|error| {
        debug_log(&format!("extension.cache.create_directory.error={error}"));
        "runner_unavailable".to_string()
    })?;
    let temp = path.with_extension(format!("part{}", std::process::id()));
    std::fs::write(&temp, bytes).map_err(|error| {
        debug_log(&format!("extension.cache.write.error={error}"));
        "runner_unavailable".to_string()
    })?;
    if std::fs::rename(&temp, path).is_err() {
        let _ = std::fs::remove_file(path);
        std::fs::rename(&temp, path).map_err(|error| {
            let _ = std::fs::remove_file(&temp);
            debug_log(&format!("extension.cache.rename.error={error}"));
            "runner_unavailable".to_string()
        })?;
    }
    Ok(())
}

fn stage_embedded_extension() -> Result<PathBuf, String> {
    debug_log("extension.stage.start");
    if EMBEDDED_QUACK_EXTENSION.is_empty() {
        debug_log("extension.stage.embedded_bytes_empty");
        return Err("runner_unavailable".to_string());
    }
    let path = extension_cache_path()?;
    let same = std::fs::read(&path)
        .map(|existing| existing == EMBEDDED_QUACK_EXTENSION)
        .unwrap_or(false);
    if !same {
        debug_log("extension.stage.cache_write.start");
        write_atomic(&path, EMBEDDED_QUACK_EXTENSION)?;
        debug_log("extension.stage.cache_write.ok");
    } else {
        debug_log("extension.stage.cache_hit");
    }

    let platform = duckle_db_runner::bundle::BundlePlatform::current().ok_or_else(|| {
        debug_log("extension.verify.platform_unsupported");
        "runner_unavailable".to_string()
    })?;
    let expected = duckle_db_runner::bundle::bundle_for(platform).ok_or_else(|| {
        debug_log("extension.verify.manifest_missing");
        "runner_unavailable".to_string()
    })?;
    duckle_db_runner::bundle::verify_staged_bundle(&path, expected).map_err(|reason| {
        debug_log(&format!("extension.verify.error={reason:?}"));
        "runner_unavailable".to_string()
    })?;
    debug_log("extension.verify.ok");
    Ok(path)
}

#[cfg(windows)]
fn run() -> Result<(), String> {
    debug_log("sidecar.process.start");
    let _extension = stage_embedded_extension()?;
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    debug_log("sidecar.bootstrap.dispatch");
    duckle_db_runner::local_quack_sidecar::run_windows_sidecar(&args).map_err(|reason| {
        debug_log(&format!("sidecar.bootstrap.error={reason:?}"));
        "runner_unavailable".to_string()
    })
}

#[cfg(not(windows))]
fn run() -> Result<(), String> {
    // Packaging exists for all approved targets, while process-launch support
    // remains gated until the platform-specific containment implementation is
    // selected. Never fall back to a CLI or network install.
    let _extension = stage_embedded_extension()?;
    debug_log("sidecar.platform_unavailable");
    Err("runner_unavailable".to_string())
}

fn main() {
    if let Err(reason) = run() {
        debug_log(&format!("sidecar.process.exit_error={reason}"));
        eprintln!("{reason}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approved_extension_cache_is_versioned_and_platform_scoped() {
        if repository_platform().is_none() || duckdb_home().is_none() {
            return;
        }
        let path = extension_cache_path().unwrap();
        let text = path.to_string_lossy();
        assert!(text.contains("extensions"));
        assert!(text.contains("v1.5.4"));
        assert!(text.ends_with(QUACK_EXTENSION_FILE));
    }
}
