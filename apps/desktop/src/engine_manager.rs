//! Engine installation manager and immutable official-runner artifact pin.
//!
//! The existing installation/controller implementation remains isolated in the
//! base module. This wrapper owns the DuckDB/Quack pair metadata and performs
//! offline checksum verification without exposing provider endpoints or tokens.

#[path = "engine_manager_base.rs"]
mod base;
pub use base::*;

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::Path;

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

#[cfg(test)]
mod runner_pin_tests {
    use super::*;

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
}
