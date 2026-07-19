//! Opaque identifiers and public state shared by the runner control plane.

use crate::resources::RunnerResourcesProfile;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use uuid::Uuid;

macro_rules! opaque_id {
    ($name:ident) => {
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord,
        )]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

opaque_id!(RunId);
opaque_id!(WorkerId);
opaque_id!(WorkerLeaseId);
opaque_id!(RunSessionId);
opaque_id!(ProviderId);
opaque_id!(EventId);

/// A cooperative cancellation signal which carries no command endpoint or
/// process handle. Providers must observe it during bootstrap.
#[derive(Debug, Clone, Default)]
pub struct RunCancellation(Arc<AtomicBool>);

impl RunCancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerKind {
    Warm,
    OnDemand,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerState {
    Starting,
    Ready,
    Leased,
    Terminating,
    Terminated,
    Failed,
}

#[derive(Debug, Clone)]
pub struct AcquireRequest {
    pub run_id: RunId,
    pub attempt: u32,
    pub profile: RunnerResourcesProfile,
    pub cancellation: RunCancellation,
}

impl AcquireRequest {
    pub fn new(run_id: RunId, attempt: u32, profile: RunnerResourcesProfile) -> Self {
        Self {
            run_id,
            attempt,
            profile,
            cancellation: RunCancellation::default(),
        }
    }
}

/// Exclusive, single-use assignment. It intentionally exposes only opaque
/// identifiers, never a PID, endpoint, secret or provider capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerLease {
    pub lease_id: WorkerLeaseId,
    pub worker_id: WorkerId,
    pub run_id: RunId,
    pub worker_kind: WorkerKind,
    pub profile_version: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunnerFailureReason {
    HostLimit,
    WorkspaceCapacity,
    LicenseLimit,
    InvalidProfile,
    ConfigurationApplyFailed,
    RunnerUnavailable,
    RunnerVersionMismatch,
    Cancelled,
    RunnerCrashed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SanitizedMetrics {
    pub memory_current_bytes: Option<u64>,
    pub memory_peak_bytes: Option<u64>,
    pub spill_current_bytes: Option<u64>,
    pub spill_peak_bytes: Option<u64>,
    pub cpu_ms: Option<u64>,
    pub rows: Option<u64>,
    pub transfer_bytes: Option<u64>,
    pub duration_ms: Option<u64>,
    pub transport_kind: Option<TransportKind>,
}

impl SanitizedMetrics {
    pub const fn empty() -> Self {
        Self {
            memory_current_bytes: None,
            memory_peak_bytes: None,
            spill_current_bytes: None,
            spill_peak_bytes: None,
            cpu_ms: None,
            rows: None,
            transfer_bytes: None,
            duration_ms: None,
            transport_kind: None,
        }
    }
}

impl Default for SanitizedMetrics {
    fn default() -> Self {
        Self::empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportKind {
    SqlRemote,
    Quack,
    Parquet,
}
