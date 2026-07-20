//! T074 smoke coverage for the final sidecar/Quack package pair.
//!
//! The workflow points these tests at the files copied into the release-shaped
//! package directory. The sidecar is then executed with a clean DuckDB home and
//! deliberately unusable network proxies. It must stage the embedded extension
//! before failing only because no private bootstrap handles were supplied.

use duckle_db_runner::bundle::{bundle_for, verify_staged_bundle, BundlePlatform, DUCKDB_VERSION};
use duckle_db_runner::model::RunnerFailureReason;
use std::path::{Path, PathBuf};
use std::process::Command;

fn package_path(name: &str) -> PathBuf {
    std::env::var_os(name)
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("{name} must point at the release-shaped package artifact"))
}

fn extension_cache_path(home: &Path, platform: BundlePlatform) -> PathBuf {
    home.join(".duckdb")
        .join("extensions")
        .join(format!("v{DUCKDB_VERSION}"))
        .join(platform.repository_name())
        .join("quack.duckdb_extension")
}

#[test]
#[ignore = "requires the verified release-shaped sidecar/Quack package"]
fn packaged_sidecar_stages_embedded_quack_with_a_clean_offline_home() {
    let sidecar = package_path("DUCKLE_PACKAGE_SIDECAR");
    let packaged_extension = package_path("DUCKLE_PACKAGE_EXTENSION");
    let platform = BundlePlatform::current().expect("T074 runs only on approved package targets");
    let expected = bundle_for(platform).expect("approved bundle entry");
    verify_staged_bundle(&packaged_extension, expected).expect("packaged extension identity");

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
        .env_remove("DUCKLE_QUACK_EXTENSION")
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
        "the offline package must fail only at the private bootstrap boundary"
    );
    assert!(!missing_cli.exists(), "the package must never create or install a CLI");

    let cached = extension_cache_path(home.path(), platform);
    assert!(cached.is_file(), "embedded Quack extension was not staged");
    assert_eq!(
        std::fs::read(&cached).expect("read staged cache extension"),
        std::fs::read(&packaged_extension).expect("read packaged extension"),
        "the executable and adjacent extension must be one matching package pair"
    );
    verify_staged_bundle(&cached, expected).expect("offline-staged extension identity");
}

#[test]
#[ignore = "requires the verified release-shaped sidecar/Quack package"]
fn altered_package_extension_is_rejected_as_a_version_mismatch() {
    let packaged_extension = package_path("DUCKLE_PACKAGE_EXTENSION");
    let platform = BundlePlatform::current().expect("T074 runs only on approved package targets");
    let expected = bundle_for(platform).expect("approved bundle entry");
    verify_staged_bundle(&packaged_extension, expected).expect("unmodified package identity");

    let temp = tempfile::tempdir().expect("mismatch workspace");
    let altered = temp.path().join("quack.duckdb_extension");
    let mut bytes = std::fs::read(&packaged_extension).expect("read package extension");
    assert!(!bytes.is_empty(), "verified extension cannot be empty");
    bytes[0] ^= 0x01;
    std::fs::write(&altered, bytes).expect("write altered extension");

    assert_eq!(
        verify_staged_bundle(&altered, expected),
        Err(RunnerFailureReason::RunnerVersionMismatch)
    );
}
