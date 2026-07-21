//! Smoke coverage for the final database sidecar package.
//!
//! Quack is provisioned by Duckle's DuckDB CLI setup flow, not embedded in the
//! sidecar. This smoke test therefore verifies only that the packaged executable
//! starts offline, does not invoke/install a separate CLI, and fails at the
//! private bootstrap boundary when launched without inherited handles.

use std::path::PathBuf;
use std::process::Command;

fn package_path(name: &str) -> PathBuf {
    std::env::var_os(name)
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("{name} must point at the release-shaped package artifact"))
}

#[test]
#[ignore = "requires the release-shaped database sidecar package"]
fn packaged_sidecar_reaches_the_private_bootstrap_boundary_offline() {
    let sidecar = package_path("DUCKLE_PACKAGE_SIDECAR");
    let home = tempfile::tempdir().expect("clean package-smoke home");
    let missing_cli = home.path().join(if cfg!(windows) {
        "missing-duckdb.exe"
    } else {
        "missing-duckdb"
    });

    let output = Command::new(&sidecar)
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("DUCKLE_DUCKDB_BIN", &missing_cli)
        .env("HTTP_PROXY", "http://127.0.0.1:9")
        .env("HTTPS_PROXY", "http://127.0.0.1:9")
        .env("ALL_PROXY", "http://127.0.0.1:9")
        .env("NO_PROXY", "")
        .env("DUCKDB_EXTENSION_REPOSITORY", "http://127.0.0.1:9")
        .output()
        .expect("execute packaged database sidecar");

    assert!(
        !output.status.success(),
        "a package-smoke invocation has no inherited bootstrap handles"
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stderr).trim(),
        "runner_unavailable",
        "the package must fail only at the private bootstrap boundary"
    );
    assert!(
        !missing_cli.exists(),
        "the package must never create or install a separate CLI"
    );
    assert!(
        !home.path().join(".duckdb").exists(),
        "without an authenticated bootstrap the sidecar must not touch the extension cache"
    );
}
