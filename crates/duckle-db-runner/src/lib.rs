//! Official per-run DuckDB/Quack sidecar boundary.
//!
//! The crate owns worker lifecycle and the private database protocol. The
//! planner and orchestration remain in `duckle-duckdb-engine`.

#![deny(unsafe_op_in_unsafe_fn)]

pub mod autoscaler;
pub mod bootstrap;
pub mod bundle;
pub mod cutover;
pub mod demand;
pub mod events;
pub mod local_quack_sidecar;
pub mod local_process_provider;
pub mod model;
pub mod process_cleanup;
pub mod resources;
pub mod run_database;
pub mod run_session;
pub mod sidecar_diagnostics;
pub mod worker_pool;

#[cfg(windows)]
pub mod windows_bootstrap;

/// Protocol revision implemented by the official runner contract.
pub const RUNNER_PROTOCOL_VERSION: u32 = 1;
