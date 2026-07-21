//! Stable, non-sensitive diagnostics for the local database sidecar boundary.
//!
//! The log deliberately accepts only enum-backed identifiers. Raw errors,
//! command lines, paths, endpoints, SQL, credentials and PIDs cannot be passed
//! through this API accidentally.

use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidecarDiagnosticCode {
    ParentLaunchStarted,
    ParentProgramInvalid,
    ParentSpawnFailed,
    ParentBootstrapSendFailed,
    ParentReadinessFailed,
    ParentReadinessOk,
    ParentCancelled,
    ClientOpenStarted,
    ClientConnectionOpenFailed,
    ClientQuackLoadFailed,
    ClientProfileApplyFailed,
    ClientSecretCreateFailed,
    ClientAttachFailed,
    ClientTransportFailed,
    ClientAttachOk,
}

impl SidecarDiagnosticCode {
    pub const ALL: [Self; 15] = [
        Self::ParentLaunchStarted,
        Self::ParentProgramInvalid,
        Self::ParentSpawnFailed,
        Self::ParentBootstrapSendFailed,
        Self::ParentReadinessFailed,
        Self::ParentReadinessOk,
        Self::ParentCancelled,
        Self::ClientOpenStarted,
        Self::ClientConnectionOpenFailed,
        Self::ClientQuackLoadFailed,
        Self::ClientProfileApplyFailed,
        Self::ClientSecretCreateFailed,
        Self::ClientAttachFailed,
        Self::ClientTransportFailed,
        Self::ClientAttachOk,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ParentLaunchStarted => "parent.launch_started",
            Self::ParentProgramInvalid => "parent.program_invalid",
            Self::ParentSpawnFailed => "parent.spawn_failed",
            Self::ParentBootstrapSendFailed => "parent.bootstrap_send_failed",
            Self::ParentReadinessFailed => "parent.readiness_failed",
            Self::ParentReadinessOk => "parent.readiness_ok",
            Self::ParentCancelled => "parent.cancelled",
            Self::ClientOpenStarted => "client.open_started",
            Self::ClientConnectionOpenFailed => "client.connection_open_failed",
            Self::ClientQuackLoadFailed => "client.quack_load_failed",
            Self::ClientProfileApplyFailed => "client.profile_apply_failed",
            Self::ClientSecretCreateFailed => "client.secret_create_failed",
            Self::ClientAttachFailed => "client.attach_failed",
            Self::ClientTransportFailed => "client.transport_failed",
            Self::ClientAttachOk => "client.attach_ok",
        }
    }
}

pub fn diagnostic_log_path(program: &Path) -> PathBuf {
    program.with_extension("log")
}

pub fn append_sidecar_diagnostic(program: &Path, code: SidecarDiagnosticCode) {
    let path = diagnostic_log_path(program);
    let timestamp_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();

    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{timestamp_ms} {}", code.as_str());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_codes_are_static_and_non_sensitive() {
        for code in SidecarDiagnosticCode::ALL {
            let value = code.as_str();
            assert!(value.starts_with("parent.") || value.starts_with("client."));
            assert!(!value.contains(['\\', '/', ':', '=', ' ']));
            assert!(!value.contains("token"));
            assert!(!value.contains("sql"));
        }
    }

    #[test]
    fn diagnostic_log_is_adjacent_to_the_sidecar() {
        let program = Path::new(r"C:\Users\tester\AppData\Roaming\io.duckle.app\engines\db-sidecar\duckle-db-sidecar.exe");
        assert_eq!(
            diagnostic_log_path(program),
            PathBuf::from(r"C:\Users\tester\AppData\Roaming\io.duckle.app\engines\db-sidecar\duckle-db-sidecar.log")
        );
    }
}
