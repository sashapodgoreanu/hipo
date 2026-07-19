//! Controller-backed implementation of the MCP `run_pipeline` tool.
//!
//! The legacy implementation remains in `tools.rs` for source compatibility,
//! but production JSON-RPC dispatch intercepts this tool here so every MCP run
//! acquires through the workspace controller and receives an independent run
//! cancellation scope.

use crate::runner_controller;
use duckle_duckdb_engine::PipelineDoc;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub(crate) fn handles(params: &Value) -> bool {
    params.get("name").and_then(Value::as_str) == Some("run_pipeline")
}

pub(crate) fn call(params: Value) -> Result<Value, (i64, String)> {
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    Ok(match run_pipeline(&args) {
        Ok(value) => content_ok(value),
        Err(error) => content_error(&error),
    })
}

fn run_pipeline(args: &Value) -> Result<Value, String> {
    let (value, name, source_path) = load_pipeline(args)?;
    let doc: PipelineDoc = serde_json::from_value(value)
        .map_err(|error| format!("invalid pipeline: {error}"))?;
    let workspace = resolve_workspace(args, source_path.as_deref());
    let duckdb = resolve_duckdb(args);

    std::env::set_var("DUCKLE_DUCKDB_BIN", &duckdb);
    std::env::set_var("DUCKLE_WORKSPACE", &workspace);
    std::env::set_var("DUCKLE_LOG_DIR", workspace.join("logs"));

    let engine = runner_controller::engine_for_workspace(duckdb, &workspace);
    let result = engine.execute_pipeline_named(&doc, &name);
    let mut output = serde_json::to_value(&result).map_err(|error| error.to_string())?;

    // Preserve the existing MCP response-size contract.
    if let Some(preview) = output.get_mut("preview").and_then(Value::as_array_mut) {
        for node in preview {
            if let Some(rows) = node.get_mut("rows").and_then(Value::as_array_mut) {
                rows.truncate(20);
            }
        }
    }
    Ok(output)
}

fn load_pipeline(args: &Value) -> Result<(Value, String, Option<PathBuf>), String> {
    if let Some(pipeline) = args.get("pipeline").filter(|value| value.is_object()) {
        let name = pipeline
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.trim().is_empty())
            .unwrap_or("mcp")
            .to_string();
        return Ok((pipeline.clone(), name, None));
    }

    let path = args
        .get("path")
        .and_then(Value::as_str)
        .filter(|path| !path.trim().is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| "provide 'pipeline' (object) or 'path' (string)".to_string())?;
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|error| format!("parse {}: {error}", path.display()))?;
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .map(str::to_string)
        .or_else(|| {
            path.file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "mcp".to_string());
    Ok((value, name, Some(path)))
}

fn resolve_workspace(args: &Value, pipeline_path: Option<&Path>) -> PathBuf {
    if let Some(workspace) = args
        .get("workspace")
        .and_then(Value::as_str)
        .filter(|workspace| !workspace.trim().is_empty())
    {
        return PathBuf::from(workspace);
    }
    if let Some(parent) = pipeline_path.and_then(Path::parent) {
        return parent.to_path_buf();
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn resolve_duckdb(args: &Value) -> PathBuf {
    args.get("duckdb")
        .and_then(Value::as_str)
        .filter(|path| !path.trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("DUCKLE_DUCKDB_BIN")
                .filter(|path| !path.is_empty())
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| PathBuf::from("duckdb"))
}

fn content_ok(mut value: Value) -> Value {
    redact_diagnostics(&mut value);
    let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
    json!({ "content": [{ "type": "text", "text": text }], "isError": false })
}

fn content_error(error: &str) -> Value {
    let safe = duckle_duckdb_engine::redact_untrusted_text(error);
    json!({ "content": [{ "type": "text", "text": safe }], "isError": true })
}

fn redact_diagnostics(value: &mut Value) {
    match value {
        Value::Array(values) => {
            for value in values {
                redact_diagnostics(value);
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                if matches!(key.as_str(), "error" | "message" | "sql" | "stderr") {
                    if let Some(text) = value.as_str() {
                        *value = Value::String(
                            duckle_duckdb_engine::redact_untrusted_text(text),
                        );
                    } else {
                        redact_diagnostics(value);
                    }
                } else {
                    redact_diagnostics(value);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_pipeline_run_is_intercepted() {
        assert!(handles(&json!({ "name": "run_pipeline" })));
        assert!(!handles(&json!({ "name": "validate_pipeline" })));
    }

    #[test]
    fn workspace_defaults_to_pipeline_parent() {
        let path = PathBuf::from("workspace/pipelines/example.json");
        assert_eq!(
            resolve_workspace(&json!({}), Some(&path)),
            PathBuf::from("workspace/pipelines")
        );
    }
}
