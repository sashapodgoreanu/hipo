//! Versioned, workspace-scoped resource configuration for runner workers.
//!
//! A profile is deliberately independent of `PipelineDoc`: desktop, headless,
//! scheduler and MCP all resolve the same requested profile before a worker may
//! become ready. No value in this module is a pool budget.

use serde::{Deserialize, Serialize};
use std::path::Path;
use thiserror::Error;

/// A byte limit can be inherited from the host, expressed as a percentage of
/// the relevant host capacity, or set explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", content = "value", rename_all = "camelCase")]
pub enum ResourceLimit {
    Automatic,
    Percent(u8),
    Bytes(u64),
}

impl Default for ResourceLimit {
    fn default() -> Self {
        Self::Automatic
    }
}

/// Positive integer setting that also supports an automatic value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", content = "value", rename_all = "camelCase")]
pub enum AutomaticOrU16 {
    Automatic,
    Value(u16),
}

impl Default for AutomaticOrU16 {
    fn default() -> Self {
        Self::Automatic
    }
}

/// Requested resources for all workers belonging to one workspace.
///
/// `version` is supplied by persistence. It identifies a whole profile,
/// rather than independent field updates, so a worker can apply it atomically.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RunnerResourcesProfile {
    pub version: u64,
    pub memory: ResourceLimit,
    pub cpu_threads: AutomaticOrU16,
    pub spill: ResourceLimit,
    pub quack_parallelism: AutomaticOrU16,
    pub base_capacity: u32,
}

impl Default for RunnerResourcesProfile {
    fn default() -> Self {
        Self {
            version: 1,
            memory: ResourceLimit::Automatic,
            cpu_threads: AutomaticOrU16::Automatic,
            spill: ResourceLimit::Automatic,
            // Automatic deliberately resolves to the verified maximum.
            quack_parallelism: AutomaticOrU16::Automatic,
            base_capacity: 3,
        }
    }
}

/// Old settings format retained only to make migration explicit and lossless.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyRunnerResources {
    pub memory_limit_mb: Option<u32>,
}

impl RunnerResourcesProfile {
    pub const DEFAULT_BASE_CAPACITY: u32 = 3;
    pub const MAX_QUACK_PARALLELISM: u16 = 8;

    /// Migrates the former desktop memory setting into a complete profile.
    pub fn from_legacy(legacy: LegacyRunnerResources) -> Self {
        let mut profile = Self::default();
        if let Some(megabytes) = legacy.memory_limit_mb.filter(|value| *value > 0) {
            profile.memory = ResourceLimit::Bytes(u64::from(megabytes) * 1024 * 1024);
        }
        profile
    }

    pub fn validate(&self) -> Result<(), ResourceProfileError> {
        if self.version == 0 {
            return Err(ResourceProfileError::ZeroVersion);
        }
        validate_limit("memory", self.memory)?;
        validate_limit("spill", self.spill)?;
        validate_positive("cpu_threads", self.cpu_threads)?;
        match self.quack_parallelism {
            AutomaticOrU16::Automatic => {}
            AutomaticOrU16::Value(value) if (1..=Self::MAX_QUACK_PARALLELISM).contains(&value) => {}
            AutomaticOrU16::Value(value) => {
                return Err(ResourceProfileError::ParallelismOutOfRange(value))
            }
        }
        if self.base_capacity == 0 {
            return Err(ResourceProfileError::ZeroBaseCapacity);
        }
        Ok(())
    }

    /// Resolves portable requested values against currently available local
    /// capacity. Missing host values intentionally stay automatic rather than
    /// becoming an invented limit.
    pub fn resolve(
        &self,
        host: HostResourceLimits,
    ) -> Result<ResolvedRunnerResources, ResourceProfileError> {
        self.validate()?;
        let (memory_bytes, memory_reason) =
            resolve_limit(self.memory, host.memory_bytes, host.memory_cap_bytes)?;
        let (spill_bytes, spill_reason) =
            resolve_limit(self.spill, host.spill_bytes, host.spill_cap_bytes)?;
        let (cpu_threads, cpu_reason) =
            resolve_threads(self.cpu_threads, host.cpu_threads, host.cpu_thread_cap)?;
        let quack_parallelism = match self.quack_parallelism {
            AutomaticOrU16::Automatic => Self::MAX_QUACK_PARALLELISM,
            AutomaticOrU16::Value(value) => value,
        };

        Ok(ResolvedRunnerResources {
            requested_version: self.version,
            effective_version: self.version,
            memory_bytes,
            cpu_threads,
            spill_bytes,
            quack_parallelism,
            base_capacity: self.base_capacity,
            diagnostics: [memory_reason, spill_reason, cpu_reason]
                .into_iter()
                .flatten()
                .collect(),
        })
    }
}

fn validate_limit(field: &'static str, value: ResourceLimit) -> Result<(), ResourceProfileError> {
    match value {
        ResourceLimit::Automatic => Ok(()),
        ResourceLimit::Percent(1..=100) => Ok(()),
        ResourceLimit::Percent(value) => Err(ResourceProfileError::InvalidPercent { field, value }),
        ResourceLimit::Bytes(0) => Err(ResourceProfileError::ZeroBytes(field)),
        ResourceLimit::Bytes(_) => Ok(()),
    }
}

fn validate_positive(
    field: &'static str,
    value: AutomaticOrU16,
) -> Result<(), ResourceProfileError> {
    match value {
        AutomaticOrU16::Automatic | AutomaticOrU16::Value(1..) => Ok(()),
        AutomaticOrU16::Value(0) => Err(ResourceProfileError::ZeroValue(field)),
    }
}

/// Capacity information available to the runner at apply time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HostResourceLimits {
    pub memory_bytes: Option<u64>,
    pub spill_bytes: Option<u64>,
    pub cpu_threads: Option<u16>,
    pub memory_cap_bytes: Option<u64>,
    pub spill_cap_bytes: Option<u64>,
    pub cpu_thread_cap: Option<u16>,
}

/// A diagnostic never contains a filesystem path, endpoint, SQL text or
/// credential; it only explains why a requested number was clamped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceClampReason {
    HostLimit,
    WorkspaceCapacity,
    LicenseLimit,
}

/// Fully resolved values a provider must apply before it reports readiness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedRunnerResources {
    pub requested_version: u64,
    pub effective_version: u64,
    pub memory_bytes: Option<u64>,
    pub cpu_threads: Option<u16>,
    pub spill_bytes: Option<u64>,
    pub quack_parallelism: u16,
    pub base_capacity: u32,
    pub diagnostics: Vec<ResourceClampReason>,
}

impl ResolvedRunnerResources {
    pub fn validate(&self) -> Result<(), ResourceProfileError> {
        if self.requested_version == 0 || self.effective_version == 0 {
            return Err(ResourceProfileError::ZeroVersion);
        }
        if self.requested_version != self.effective_version {
            return Err(ResourceProfileError::VersionMismatch);
        }
        if !(1..=RunnerResourcesProfile::MAX_QUACK_PARALLELISM).contains(&self.quack_parallelism) {
            return Err(ResourceProfileError::ParallelismOutOfRange(
                self.quack_parallelism,
            ));
        }
        if self.base_capacity == 0 {
            return Err(ResourceProfileError::ZeroBaseCapacity);
        }
        Ok(())
    }
}

/// The non-sensitive requested/effective view shared by every entry point.
/// Keeping this DTO in the runner crate prevents desktop, headless, scheduler,
/// and MCP from inventing different interpretations of the same settings file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceRunnerResources {
    pub requested: RunnerResourcesProfile,
    pub effective: ResolvedRunnerResources,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct WorkspaceSettingsResources {
    memory_limit_mb: Option<u32>,
    runner_resources: Option<RunnerResourcesProfile>,
}

/// Load the complete requested profile from `<workspace>/.duckle/settings.json`.
/// A missing file means the documented defaults. A present but unreadable,
/// malformed, or invalid profile fails closed with a stable non-sensitive code.
pub fn load_workspace_runner_resources(
    workspace: &Path,
) -> Result<RunnerResourcesProfile, WorkspaceRunnerResourcesError> {
    let path = workspace.join(".duckle").join("settings.json");
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RunnerResourcesProfile::default())
        }
        Err(_) => return Err(WorkspaceRunnerResourcesError::ReadFailed),
    };
    let settings: WorkspaceSettingsResources =
        serde_json::from_slice(&bytes).map_err(|_| WorkspaceRunnerResourcesError::ParseFailed)?;
    let profile = settings.runner_resources.unwrap_or_else(|| {
        RunnerResourcesProfile::from_legacy(LegacyRunnerResources {
            memory_limit_mb: settings.memory_limit_mb,
        })
    });
    profile
        .validate()
        .map_err(WorkspaceRunnerResourcesError::InvalidProfile)?;
    Ok(profile)
}

/// Resolve the same requested profile every entry point passes to its provider.
pub fn resolve_workspace_runner_resources(
    workspace: &Path,
    host: HostResourceLimits,
) -> Result<WorkspaceRunnerResources, WorkspaceRunnerResourcesError> {
    let requested = load_workspace_runner_resources(workspace)?;
    let effective = requested
        .resolve(host)
        .map_err(WorkspaceRunnerResourcesError::InvalidProfile)?;
    Ok(WorkspaceRunnerResources {
        requested,
        effective,
    })
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum WorkspaceRunnerResourcesError {
    #[error("runner_resources_read_failed")]
    ReadFailed,
    #[error("runner_resources_parse_failed")]
    ParseFailed,
    #[error("invalid_runner_resources")]
    InvalidProfile(#[source] ResourceProfileError),
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ResourceProfileError {
    #[error("resource profile version must be positive")]
    ZeroVersion,
    #[error("requested and effective profile versions must match")]
    VersionMismatch,
    #[error("{field} percentage {value} must be in 1..=100")]
    InvalidPercent { field: &'static str, value: u8 },
    #[error("{0} byte limit must be positive")]
    ZeroBytes(&'static str),
    #[error("{0} must be positive")]
    ZeroValue(&'static str),
    #[error("Quack parallelism {0} must be in 1..=8")]
    ParallelismOutOfRange(u16),
    #[error("base capacity must be positive")]
    ZeroBaseCapacity,
    #[error("a percentage requires an available host capacity")]
    MissingCapacityForPercent,
}

fn resolve_limit(
    requested: ResourceLimit,
    available: Option<u64>,
    cap: Option<u64>,
) -> Result<(Option<u64>, Option<ResourceClampReason>), ResourceProfileError> {
    let requested = match requested {
        ResourceLimit::Automatic => {
            return Ok((
                cap.or(available),
                cap.map(|_| ResourceClampReason::HostLimit),
            ));
        }
        ResourceLimit::Bytes(value) => value,
        ResourceLimit::Percent(percent) => {
            let total = available.ok_or(ResourceProfileError::MissingCapacityForPercent)?;
            total.saturating_mul(u64::from(percent)) / 100
        }
    };
    match cap {
        Some(limit) if requested > limit => Ok((Some(limit), Some(ResourceClampReason::HostLimit))),
        _ => Ok((Some(requested), None)),
    }
}

fn resolve_threads(
    requested: AutomaticOrU16,
    available: Option<u16>,
    cap: Option<u16>,
) -> Result<(Option<u16>, Option<ResourceClampReason>), ResourceProfileError> {
    let requested = match requested {
        AutomaticOrU16::Automatic => {
            return Ok((
                cap.or(available),
                cap.map(|_| ResourceClampReason::HostLimit),
            ));
        }
        AutomaticOrU16::Value(value) => value,
    };
    match cap {
        Some(limit) if requested > limit => Ok((Some(limit), Some(ResourceClampReason::HostLimit))),
        _ => Ok((Some(requested), None)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_complete_and_use_base_three() {
        let profile = RunnerResourcesProfile::default();
        assert_eq!(profile.base_capacity, 3);
        assert_eq!(profile.quack_parallelism, AutomaticOrU16::Automatic);
        profile.validate().unwrap();
    }

    #[test]
    fn legacy_memory_is_migrated_to_an_absolute_limit() {
        let profile = RunnerResourcesProfile::from_legacy(LegacyRunnerResources {
            memory_limit_mb: Some(128),
        });
        assert_eq!(profile.memory, ResourceLimit::Bytes(128 * 1024 * 1024));
    }

    #[test]
    fn rejects_invalid_parallelism_and_base_capacity() {
        let mut profile = RunnerResourcesProfile::default();
        profile.quack_parallelism = AutomaticOrU16::Value(9);
        assert_eq!(
            profile.validate(),
            Err(ResourceProfileError::ParallelismOutOfRange(9))
        );
        profile.quack_parallelism = AutomaticOrU16::Value(8);
        profile.base_capacity = 0;
        assert_eq!(
            profile.validate(),
            Err(ResourceProfileError::ZeroBaseCapacity)
        );
    }

    #[test]
    fn resolves_percent_and_clamps_to_host_limit() {
        let profile = RunnerResourcesProfile {
            memory: ResourceLimit::Percent(80),
            cpu_threads: AutomaticOrU16::Value(12),
            ..RunnerResourcesProfile::default()
        };
        let resolved = profile
            .resolve(HostResourceLimits {
                memory_bytes: Some(1_000),
                memory_cap_bytes: Some(600),
                cpu_threads: Some(16),
                cpu_thread_cap: Some(8),
                ..HostResourceLimits::default()
            })
            .unwrap();
        assert_eq!(resolved.memory_bytes, Some(600));
        assert_eq!(resolved.cpu_threads, Some(8));
        assert_eq!(resolved.diagnostics.len(), 2);
    }

    #[test]
    fn serde_defaults_missing_fields_and_round_trips_a_complete_profile() {
        let migrated: RunnerResourcesProfile = serde_json::from_str(r#"{"version":4}"#).unwrap();
        assert_eq!(migrated.version, 4);
        assert_eq!(migrated.base_capacity, 3);
        assert_eq!(migrated.quack_parallelism, AutomaticOrU16::Automatic);

        let complete = RunnerResourcesProfile {
            version: 9,
            memory: ResourceLimit::Percent(75),
            cpu_threads: AutomaticOrU16::Value(6),
            spill: ResourceLimit::Bytes(512 * 1024 * 1024),
            quack_parallelism: AutomaticOrU16::Value(4),
            base_capacity: 7,
        };
        let json = serde_json::to_string(&complete).unwrap();
        assert_eq!(
            serde_json::from_str::<RunnerResourcesProfile>(&json).unwrap(),
            complete
        );
    }

    #[test]
    fn validates_every_numeric_boundary() {
        let invalid = [
            RunnerResourcesProfile {
                version: 0,
                ..RunnerResourcesProfile::default()
            },
            RunnerResourcesProfile {
                memory: ResourceLimit::Percent(0),
                ..RunnerResourcesProfile::default()
            },
            RunnerResourcesProfile {
                spill: ResourceLimit::Percent(101),
                ..RunnerResourcesProfile::default()
            },
            RunnerResourcesProfile {
                memory: ResourceLimit::Bytes(0),
                ..RunnerResourcesProfile::default()
            },
            RunnerResourcesProfile {
                cpu_threads: AutomaticOrU16::Value(0),
                ..RunnerResourcesProfile::default()
            },
            RunnerResourcesProfile {
                quack_parallelism: AutomaticOrU16::Value(0),
                ..RunnerResourcesProfile::default()
            },
            RunnerResourcesProfile {
                quack_parallelism: AutomaticOrU16::Value(9),
                ..RunnerResourcesProfile::default()
            },
            RunnerResourcesProfile {
                base_capacity: 0,
                ..RunnerResourcesProfile::default()
            },
        ];
        assert!(invalid.iter().all(|profile| profile.validate().is_err()));

        for parallelism in 1..=RunnerResourcesProfile::MAX_QUACK_PARALLELISM {
            RunnerResourcesProfile {
                quack_parallelism: AutomaticOrU16::Value(parallelism),
                ..RunnerResourcesProfile::default()
            }
            .validate()
            .unwrap();
        }
    }

    #[test]
    fn percent_resolution_requires_real_host_capacity() {
        let profile = RunnerResourcesProfile {
            spill: ResourceLimit::Percent(50),
            ..RunnerResourcesProfile::default()
        };
        assert_eq!(
            profile.resolve(HostResourceLimits::default()),
            Err(ResourceProfileError::MissingCapacityForPercent)
        );
    }

    #[test]
    fn workspace_loader_migrates_legacy_and_resolves_one_version() {
        let workspace = tempfile::tempdir().unwrap();
        let settings_dir = workspace.path().join(".duckle");
        std::fs::create_dir_all(&settings_dir).unwrap();
        std::fs::write(
            settings_dir.join("settings.json"),
            r#"{"memory_limit_mb":256}"#,
        )
        .unwrap();

        let status = resolve_workspace_runner_resources(
            workspace.path(),
            HostResourceLimits::default(),
        )
        .unwrap();
        assert_eq!(
            status.requested.memory,
            ResourceLimit::Bytes(256 * 1024 * 1024)
        );
        assert_eq!(status.requested.version, status.effective.requested_version);
        assert_eq!(
            status.effective.requested_version,
            status.effective.effective_version
        );
    }

    #[test]
    fn workspace_loader_rejects_present_invalid_settings() {
        let workspace = tempfile::tempdir().unwrap();
        let settings_dir = workspace.path().join(".duckle");
        std::fs::create_dir_all(&settings_dir).unwrap();
        std::fs::write(
            settings_dir.join("settings.json"),
            r#"{"runner_resources":{"version":0}}"#,
        )
        .unwrap();

        assert!(matches!(
            load_workspace_runner_resources(workspace.path()),
            Err(WorkspaceRunnerResourcesError::InvalidProfile(_))
        ));
    }

    #[test]
    fn missing_workspace_settings_use_the_complete_default_profile() {
        let workspace = tempfile::tempdir().unwrap();
        assert_eq!(
            load_workspace_runner_resources(workspace.path()).unwrap(),
            RunnerResourcesProfile::default()
        );
    }
}
