//! Structured, component-level run logging for external observability
//! (Splunk / Dynatrace), at log4j-style depth.
//!
//! When `DUCKLE_LOG_DIR` is set (the desktop points it at the user's
//! `<workspace>/logs`), every pipeline run appends NDJSON lines to
//! `<pipeline name>/runtime.log` under that directory - one JSON object per
//! line, which is exactly what log shippers expect, so a user can tail the
//! file straight into Splunk or Dynatrace with no transform. The per-
//! pipeline folder is created on first run.
//!
//! Each line carries the component identity (`component` = the component id,
//! e.g. `src.csv`, acting like a log4j logger name), the node id + label,
//! the lifecycle phase (run/stage start + finish), row counts, durations,
//! errors, and any ctl.log / ctl.warn / ctl.die messages. That gives a full
//! per-component trace of a run.
//!
//! Logging is best-effort: a missing dir, permission error, or write
//! failure never affects the run - the line is just dropped.

use crate::PipelineEvent;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;

const LOG_FILE: &str = "runtime.log";

/// Per-node identity used to enrich log lines with component-level detail.
#[derive(Clone)]
pub(crate) struct NodeMeta {
    pub component: String,
    pub label: String,
}

/// Appends structured run events to `$DUCKLE_LOG_DIR/duckle.jsonl`. A no-op
/// when the env var is unset (tests, headless runs without a workspace).
pub(crate) struct RunLog {
    file: Option<File>,
    run_id: String,
    nodes: HashMap<String, NodeMeta>,
}

impl RunLog {
    /// Open the run log for one run. `pipeline_name` names the per-pipeline
    /// subfolder (`<DUCKLE_LOG_DIR>/<name>/runtime.log`); `run_id` ties a
    /// run's lines together; `nodes` maps node id -> component identity for
    /// enrichment. Returns a disabled logger (writes nothing) when
    /// `DUCKLE_LOG_DIR` is absent or the file can't be opened.
    pub(crate) fn open(
        pipeline_name: Option<&str>,
        run_id: String,
        nodes: HashMap<String, NodeMeta>,
    ) -> Self {
        let folder = sanitize_segment(pipeline_name.unwrap_or("pipeline"));
        let file = std::env::var("DUCKLE_LOG_DIR")
            .ok()
            .filter(|d| !d.is_empty())
            .and_then(|dir| {
                let dir = Path::new(&dir).join(&folder);
                std::fs::create_dir_all(&dir).ok()?;
                OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(dir.join(LOG_FILE))
                    .ok()
            });
        RunLog { file, run_id, nodes }
    }

    /// Whether this logger is actually writing (env configured).
    pub(crate) fn enabled(&self) -> bool {
        self.file.is_some()
    }

    /// Append one event as a structured NDJSON line.
    pub(crate) fn record(&mut self, event: &PipelineEvent) {
        if self.file.is_none() {
            return;
        }
        let mut obj = self.event_fields(event);
        obj.insert("ts".into(), Value::String(now_rfc3339()));
        obj.insert("run_id".into(), Value::String(self.run_id.clone()));

        // Final boundary pass over the complete payload. Individual diagnostic
        // fields are already redacted below, but this recursive pass protects
        // future fields and user-controlled labels/ids from credential-shaped
        // values before the NDJSON reaches a file or external log shipper.
        let mut payload = Value::Object(obj);
        redact_json_strings(&mut payload);
        if let Ok(mut line) = serde_json::to_string(&payload) {
            line.push('\n');
            if let Some(file) = self.file.as_mut() {
                let _ = file.write_all(line.as_bytes());
            }
        }
    }

    /// Flatten a `PipelineEvent` to a JSON object (level + event + fields),
    /// enriching any node-scoped event with its component id + label.
    fn event_fields(&self, event: &PipelineEvent) -> Map<String, Value> {
        let mut m = Map::new();
        let mut set = |k: &str, v: Value| {
            m.insert(k.to_string(), v);
        };
        match event {
            PipelineEvent::Started { total_stages } => {
                set("event", json!("run_started"));
                set("level", json!("info"));
                set("total_stages", json!(total_stages));
            }
            PipelineEvent::StageStarted { node_id, label, kind } => {
                set("event", json!("stage_started"));
                set("level", json!("info"));
                set("node_id", json!(node_id));
                set("label", json!(label));
                set("kind", json!(kind));
                self.enrich(&mut m, node_id);
            }
            PipelineEvent::StageFinished {
                node_id, kind, status, rows, duration_ms, error, sql,
            } => {
                set("event", json!("stage_finished"));
                set("level", json!(if status == "error" { "error" } else { "info" }));
                set("node_id", json!(node_id));
                set("kind", json!(kind));
                set("status", json!(status));
                set("rows", json!(rows));
                set("duration_ms", json!(duration_ms));
                set("error", json!(error.as_deref().map(crate::redact_untrusted_text)));
                if let Some(err) = error {
                    set(
                        "category",
                        json!(crate::error_category::categorize_error(err)),
                    );
                }
                // The failing statement (present only on error) so the NDJSON
                // trace shows exactly what each component ran.
                if let Some(s) = sql {
                    set("sql", json!(crate::redact_untrusted_text(s)));
                }
                self.enrich(&mut m, node_id);
            }
            PipelineEvent::Cancelled => {
                set("event", json!("cancelled"));
                set("level", json!("warn"));
            }
            PipelineEvent::Log { node_id, level, message } => {
                set("event", json!("log"));
                set("level", json!(level));
                set("node_id", json!(node_id));
                set("message", json!(crate::redact_untrusted_text(message)));
                self.enrich(&mut m, node_id);
            }
            PipelineEvent::Finished { status, duration_ms } => {
                set("event", json!("run_finished"));
                set("level", json!(if status == "error" { "error" } else { "info" }));
                set("status", json!(status));
                set("duration_ms", json!(duration_ms));
            }
        }
        m
    }

    /// Add the component id + label for a node, when known. `component` is
    /// the log4j-style logger name (e.g. "src.csv").
    fn enrich(&self, m: &mut Map<String, Value>, node_id: &str) {
        if let Some(meta) = self.nodes.get(node_id) {
            m.insert("component".into(), Value::String(meta.component.clone()));
            if !meta.label.is_empty() {
                m.insert("label".into(), Value::String(meta.label.clone()));
            }
        }
    }
}

fn redact_json_strings(value: &mut Value) {
    match value {
        Value::String(text) => {
            *text = crate::redact_untrusted_text(text);
        }
        Value::Array(values) => {
            for value in values {
                redact_json_strings(value);
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                redact_json_strings(value);
            }
        }
        _ => {}
    }
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Turn a pipeline name into a safe single path segment for the log folder:
/// keep alphanumerics, space, dash, underscore and dot; replace anything
/// else (path separators, control chars) with '_'. Falls back to "pipeline"
/// when the result is empty.
fn sanitize_segment(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').trim();
    if cleaned.is_empty() {
        "pipeline".to_string()
    } else {
        cleaned.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn final_log_boundary_redacts_diagnostics_and_user_controlled_strings() {
        let canary = "TOP_SECRET_CANARY";
        let mut nodes = HashMap::new();
        nodes.insert(
            "source".to_string(),
            NodeMeta {
                component: format!("token={canary}"),
                label: format!("password={canary}"),
            },
        );
        let log = RunLog {
            file: None,
            run_id: format!("secret={canary}"),
            nodes,
        };
        let event = PipelineEvent::StageFinished {
            node_id: "source".to_string(),
            kind: "view".to_string(),
            status: "error".to_string(),
            rows: None,
            duration_ms: 1,
            error: Some(format!("Authorization: Bearer {canary}")),
            sql: Some(format!("SET token={canary}")),
        };
        let mut payload = Value::Object(log.event_fields(&event));
        payload
            .as_object_mut()
            .unwrap()
            .insert("run_id".into(), Value::String(log.run_id.clone()));
        redact_json_strings(&mut payload);
        let serialized = payload.to_string();

        assert!(!serialized.contains(canary));
        assert!(serialized.contains("Bearer ***"));
        assert!(serialized.contains("token=***"));
        assert!(serialized.contains("password=***"));
        assert!(serialized.contains("secret=***"));
    }
}
