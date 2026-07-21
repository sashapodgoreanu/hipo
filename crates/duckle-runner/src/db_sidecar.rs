#![cfg_attr(windows, windows_subsystem = "windows")]

//! Packaged local database sidecar entrypoint.
//!
//! Duckle's DuckDB CLI installation flow provisions Quack together with the
//! other product extensions. The sidecar configures that CLI-managed cache as a
//! local extension repository and asks DuckDB to install/load Quack at runtime.
//! DuckDB itself validates whether the discovered extension can be loaded.

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

#[cfg(windows)]
fn run() -> Result<(), String> {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    duckle_db_runner::local_quack_sidecar::run_windows_sidecar(&args).map_err(|_| {
        write_failure_marker("sidecar.bootstrap_or_quack_failed");
        "runner_unavailable".to_string()
    })
}

#[cfg(not(windows))]
fn run() -> Result<(), String> {
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

    #[test]
    fn failure_marker_codes_are_static_and_non_sensitive() {
        for code in [
            "sidecar.bootstrap_or_quack_failed",
            "sidecar.platform_unavailable",
        ] {
            assert!(code.starts_with("sidecar."));
            assert!(!code.contains(['\\', '/', ':', '=', ' ']));
        }
    }
}
