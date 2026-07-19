//! Offline Quack bundle identity and verification.
//!
//! The runner never calls `INSTALL quack` at runtime. Release packaging stages
//! the exact loadable extension selected here, then this module verifies its
//! platform/version/checksum identity before a provider can report readiness.

use crate::model::RunnerFailureReason;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

pub const DUCKDB_VERSION: &str = "1.5.4";
pub const QUACK_EXTENSION_VERSION: &str = "1.5.4";
pub const QUACK_LICENSE: &str = "MIT";
pub const QUACK_PROVENANCE: &str = "https://github.com/duckdb/duckdb-quack";
pub const QUACK_EXTENSION_BASE_URL: &str = "https://extensions.duckdb.org/v1.5.4";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundlePlatform {
    WindowsAmd64,
    MacosAmd64,
    LinuxAmd64,
}

impl BundlePlatform {
    pub const fn repository_name(self) -> &'static str {
        match self {
            Self::WindowsAmd64 => "windows_amd64",
            Self::MacosAmd64 => "osx_amd64",
            Self::LinuxAmd64 => "linux_amd64",
        }
    }

    pub const fn current() -> Option<Self> {
        #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
        {
            return Some(Self::WindowsAmd64);
        }
        #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
        {
            return Some(Self::MacosAmd64);
        }
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        {
            return Some(Self::LinuxAmd64);
        }
        #[allow(unreachable_code)]
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuackBundleEntry {
    pub platform: BundlePlatform,
    pub duckdb_version: &'static str,
    pub quack_version: &'static str,
    pub sha256: &'static str,
    pub license: &'static str,
    pub provenance: &'static str,
}

pub const QUACK_BUNDLES: [QuackBundleEntry; 3] = [
    QuackBundleEntry {
        platform: BundlePlatform::WindowsAmd64,
        duckdb_version: DUCKDB_VERSION,
        quack_version: QUACK_EXTENSION_VERSION,
        sha256: "52d20e78a0498c721fb0764e94d8e5b287fded3d8fcf6e95365cb03e5905b895",
        license: QUACK_LICENSE,
        provenance: QUACK_PROVENANCE,
    },
    QuackBundleEntry {
        platform: BundlePlatform::MacosAmd64,
        duckdb_version: DUCKDB_VERSION,
        quack_version: QUACK_EXTENSION_VERSION,
        sha256: "85a48992d0b940f7cf1c55bbe4efd02f46c9724b67e238a990df3f3244d8e970",
        license: QUACK_LICENSE,
        provenance: QUACK_PROVENANCE,
    },
    QuackBundleEntry {
        platform: BundlePlatform::LinuxAmd64,
        duckdb_version: DUCKDB_VERSION,
        quack_version: QUACK_EXTENSION_VERSION,
        sha256: "decb78a4d953ff9cc65c300cf2c8d3f3d8f4732851205684565c922113bc2b9e",
        license: QUACK_LICENSE,
        provenance: QUACK_PROVENANCE,
    },
];

pub fn bundle_for(platform: BundlePlatform) -> Option<&'static QuackBundleEntry> {
    QUACK_BUNDLES.iter().find(|entry| entry.platform == platform)
}

/// Verifies a release-staged extension without loading it. A missing artifact
/// is an availability error; any identity mismatch stays distinct and blocks
/// worker readiness before a sidecar can receive a lease.
pub fn verify_staged_bundle(path: &Path, expected: &QuackBundleEntry) -> Result<(), RunnerFailureReason> {
    let mut file = File::open(path).map_err(|_| RunnerFailureReason::RunnerUnavailable)?;
    let hash = sha256_reader(&mut file).map_err(|_| RunnerFailureReason::RunnerUnavailable)?;
    if hash != expected.sha256 {
        return Err(RunnerFailureReason::RunnerVersionMismatch);
    }
    Ok(())
}

fn sha256_reader(reader: &mut impl Read) -> io::Result<String> {
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn staged_bundle_verification_never_treats_a_mismatch_as_available() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        let mut file = temp.reopen().unwrap();
        file.write_all(b"not-a-quack-extension").unwrap();
        file.flush().unwrap();
        let entry = bundle_for(BundlePlatform::WindowsAmd64).unwrap();
        assert_eq!(
            verify_staged_bundle(temp.path(), entry),
            Err(RunnerFailureReason::RunnerVersionMismatch)
        );
    }

    #[test]
    fn manifest_carries_a_complete_non_network_identity() {
        for entry in QUACK_BUNDLES {
            assert_eq!(entry.duckdb_version, DUCKDB_VERSION);
            assert_eq!(entry.quack_version, QUACK_EXTENSION_VERSION);
            assert_eq!(entry.license, QUACK_LICENSE);
            assert!(entry.sha256.chars().all(|character| character.is_ascii_hexdigit()));
            assert_eq!(entry.sha256.len(), 64);
        }
    }
}
