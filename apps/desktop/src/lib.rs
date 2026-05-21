//! Duckle desktop shell.
//!
//! Boots the Tauri runtime, wires it to `duckle-runtime`, and exposes
//! invoke commands to the frontend.

use duckle_connectors::CsvConnector;
use duckle_duckdb_engine::{DuckdbEngine, PipelineDoc, PipelineEvent, RunResult};
use duckle_metadata::Schema;
use duckle_plugin_sdk::{InspectError, SchemaInspector};
use serde::Serialize;
use serde_json::Value as JsonValue;
use std::sync::OnceLock;
use tauri::ipc::Channel;
use tracing_subscriber::EnvFilter;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    tracing::info!("duckle starting");

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .invoke_handler(tauri::generate_handler![
            ping,
            autodetect_schema,
            run_pipeline,
            run_pipeline_partial,
            cancel_pipeline
        ])
        .run(tauri::generate_context!())
        .expect("error while running duckle");
}

/// Liveness probe. Returns the string `"pong"`.
#[tauri::command]
fn ping() -> &'static str {
    "pong"
}

#[derive(Debug, Serialize)]
pub struct InspectionPayload {
    pub columns: Schema,
    #[serde(rename = "sampleRows")]
    pub sample_rows: Vec<JsonValue>,
}

static DUCKDB_ENGINE: OnceLock<Result<DuckdbEngine, String>> = OnceLock::new();

fn engine() -> Result<DuckdbEngine, String> {
    DUCKDB_ENGINE
        .get_or_init(|| DuckdbEngine::new().map_err(|e| e.to_string()))
        .clone()
}

/// Inspect a source's schema. The frontend hands us a format string
/// (`"csv"`, `"parquet"`, `"json"`, `"sqlite"`, `"duckdb"`, ...) and the
/// connector-specific options, and we return inferred columns plus a
/// small sample for the Preview tab.
///
/// Most formats go through DuckDB's native readers — `read_csv_auto`,
/// `read_parquet`, `read_json_auto`, `sqlite_scan`. The hand-rolled
/// `CsvConnector` stays as a backup for environments where the DuckDB
/// engine fails to come up.
#[tauri::command]
async fn autodetect_schema(
    format: String,
    options: JsonValue,
) -> Result<InspectionPayload, String> {
    let inspection = match engine() {
        Ok(eng) => match eng.inspect(&format, options.clone()) {
            Ok(insp) => insp,
            Err(e) => {
                tracing::warn!(
                    "DuckDB autodetect failed for {} ({}); falling back",
                    format,
                    e
                );
                if matches!(format.as_str(), "csv" | "tsv") {
                    CsvConnector
                        .inspect(options)
                        .await
                        .map_err(format_inspect_error)?
                } else {
                    return Err(e.to_string());
                }
            }
        },
        Err(boot_err) => {
            tracing::error!("DuckDB engine failed to start: {}", boot_err);
            if matches!(format.as_str(), "csv" | "tsv") {
                CsvConnector
                    .inspect(options)
                    .await
                    .map_err(format_inspect_error)?
            } else {
                return Err(format!("DuckDB engine unavailable: {}", boot_err));
            }
        }
    };
    Ok(InspectionPayload {
        columns: inspection.schema,
        sample_rows: inspection.sample_rows,
    })
}

fn format_inspect_error(err: InspectError) -> String {
    err.to_string()
}

/// Run a pipeline through the DuckDB engine. Receives the React Flow
/// nodes + edges as JSON; compiles to SQL; executes via DuckDB; returns
/// per-node status + preview rows for any leaf node that didn't feed a
/// sink.
///
/// `on_event` is a Tauri Channel — every stage start / stage finish /
/// cancellation is pushed through it so the frontend can light up
/// status badges in real time.
#[tauri::command]
async fn run_pipeline(
    pipeline: PipelineDoc,
    on_event: Channel<PipelineEvent>,
) -> Result<RunResult, String> {
    let engine = engine()?;
    tokio::task::spawn_blocking(move || {
        engine.execute_pipeline_with_events(&pipeline, None, |evt| {
            let _ = on_event.send(evt);
        })
    })
    .await
    .map_err(|e| e.to_string())
}

/// Same as `run_pipeline` but only executes the subgraph upstream of
/// (and including) `target_node_id`. The target becomes the leaf and
/// returns a preview.
#[tauri::command]
async fn run_pipeline_partial(
    pipeline: PipelineDoc,
    target_node_id: String,
    on_event: Channel<PipelineEvent>,
) -> Result<RunResult, String> {
    let engine = engine()?;
    let target = target_node_id;
    tokio::task::spawn_blocking(move || {
        engine.execute_pipeline_with_events(&pipeline, Some(target.as_str()), |evt| {
            let _ = on_event.send(evt);
        })
    })
    .await
    .map_err(|e| e.to_string())
}

/// Signal the engine to stop at the next stage boundary. The current
/// stage (if mid-flight) still finishes; subsequent stages are
/// skipped.
#[tauri::command]
fn cancel_pipeline() -> Result<(), String> {
    let engine = engine()?;
    engine.request_cancel();
    Ok(())
}
