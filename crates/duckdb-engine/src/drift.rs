//! Schema-drift detection and runner-routed data tools.
//!
//! Inspect, schema drift, and branch/data diff share one routing boundary. The
//! compatibility route retains the current CLI implementation until cutover;
//! the official route acquires one opaque worker lease and performs setup and
//! queries through the same RunDatabase session.

use crate::{
    plan, DuckdbEngine, EngineError, ExecutionRoute, OfficialRunnerController, PipelineDoc,
};
use duckle_db_runner::model::{RunId, RunnerFailureReason, WorkerLease};
use duckle_metadata::Column;
use duckle_plugin_sdk::Inspection;
use serde_json::{json, Map, Value};
use std::sync::Arc;

/// Explicit inventory of data tools that must not spawn or address DuckDB
/// directly after cutover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunDatabaseTool {
    Inspect,
    SchemaDrift,
    BranchDiff,
}

pub const RUN_DATABASE_TOOL_INVENTORY: [RunDatabaseTool; 3] = [
    RunDatabaseTool::Inspect,
    RunDatabaseTool::SchemaDrift,
    RunDatabaseTool::BranchDiff,
];

/// Runner-aware data-tool operations. These names are intentionally distinct
/// from the compatibility methods while both routes coexist. T071 can remove
/// the legacy methods after CutoverEvidence is approved.
pub trait RunDatabaseDataTools {
    fn inspect_via_run_database(
        &self,
        format: &str,
        options: Value,
    ) -> Result<Inspection, EngineError>;

    fn branch_diff_rows_via_run_database(
        &self,
        setup_statements: Vec<String>,
        query: &str,
        limit: u32,
    ) -> Result<Vec<Value>, EngineError>;
}

struct DataToolLease {
    controller: Arc<dyn OfficialRunnerController>,
    lease: WorkerLease,
}

impl Drop for DataToolLease {
    fn drop(&mut self) {
        self.controller
            .release(self.lease.clone(), crate::runner_now_millis());
    }
}

impl DataToolLease {
    fn execute_setup(
        &self,
        engine: &DuckdbEngine,
        statements: Vec<String>,
    ) -> Result<(), EngineError> {
        if statements.is_empty() {
            return Ok(());
        }
        self.controller
            .execute_batch(
                &self.lease,
                statements,
                engine.official_cancellation.clone(),
            )
            .map(|_| ())
            .map_err(data_tool_failure)
    }

    fn query_rows(
        &self,
        engine: &DuckdbEngine,
        sql: &str,
        limit: u32,
    ) -> Result<Vec<Value>, EngineError> {
        let result = self
            .controller
            .preview_relation(
                &self.lease,
                sql,
                limit.max(1),
                engine.official_cancellation.clone(),
            )
            .map_err(data_tool_failure)?;
        Ok(result
            .rows
            .into_iter()
            .map(|row| Value::Object(row.into_iter().collect::<Map<String, Value>>()))
            .collect())
    }
}

impl DuckdbEngine {
    fn acquire_data_tool_lease(&self) -> Result<DataToolLease, EngineError> {
        let controller = self
            .official_runner
            .as_ref()
            .ok_or_else(|| EngineError::Other("runner_unavailable".into()))?;
        let lease = controller
            .acquire(
                RunId::new(),
                1,
                self.official_cancellation.clone(),
                crate::runner_now_millis(),
            )
            .map_err(data_tool_failure)?;
        Ok(DataToolLease {
            controller: controller.clone(),
            lease,
        })
    }
}

impl RunDatabaseDataTools for DuckdbEngine {
    fn inspect_via_run_database(
        &self,
        format: &str,
        mut options: Value,
    ) -> Result<Inspection, EngineError> {
        if self.execution_route != ExecutionRoute::OfficialRunner {
            return self.inspect(format, options);
        }

        crate::context::apply_env_to_value(&mut options);
        let select = match plan::source_select_for_format(format, &options) {
            Some(select) => select,
            None if plan::is_attach_relational_format(format) => {
                return Err(EngineError::Unsupported(format!(
                    "Format '{}' is not supported",
                    format
                )))
            }
            None => {
                return Err(EngineError::Unsupported(
                    "runner_data_tool_not_sql_addressable".into(),
                ))
            }
        };

        let lease = self.acquire_data_tool_lease()?;
        let prelude = self.source_prelude(format, &options);
        if !prelude.trim().is_empty() {
            lease.execute_setup(self, vec![prelude])?;
        }

        let describe = lease.query_rows(self, &format!("DESCRIBE {}", select), 4_096)?;
        let schema: Vec<Column> = describe
            .iter()
            .filter_map(crate::parse_describe_row)
            .collect();
        let sample_rows = lease.query_rows(
            self,
            &format!("{} LIMIT {}", select, crate::PREVIEW_LIMIT),
            crate::PREVIEW_LIMIT as u32,
        )?;

        Ok(Inspection {
            schema,
            sample_rows,
        })
    }

    fn branch_diff_rows_via_run_database(
        &self,
        setup_statements: Vec<String>,
        query: &str,
        limit: u32,
    ) -> Result<Vec<Value>, EngineError> {
        if self.execution_route != ExecutionRoute::OfficialRunner {
            let sql = if setup_statements.is_empty() {
                query.to_string()
            } else {
                format!("{}; {}", setup_statements.join("; "), query)
            };
            return self.run_rows(None, &sql);
        }

        let lease = self.acquire_data_tool_lease()?;
        lease.execute_setup(self, setup_statements)?;
        lease.query_rows(self, query, limit)
    }
}

fn data_tool_failure(reason: RunnerFailureReason) -> EngineError {
    EngineError::Other(crate::runner_failure_code(reason).to_string())
}

/// The format string the engine's inspect path expects for a source component.
/// Mirrors `plan::source_select_for_format`: the component id minus the `src.`
/// prefix, with the S3-compatible aliases routed through the `s3` reader.
fn source_format(component_id: &str) -> String {
    let s = component_id.strip_prefix("src.").unwrap_or(component_id);
    match s {
        "minio" | "r2" | "b2" => "s3".to_string(),
        other => other.to_string(),
    }
}

/// Compare a node's declared columns against its live columns. Returns
/// (missing, added, typeChanges): missing = declared but absent from the source,
/// added = present in the source but not declared, typeChanges = same name with
/// a different type. Both sides carry the normalised `DataType`, so a type
/// change is a plain `!=` (no string juggling).
fn compare_columns(declared: &[Column], live: &[Column]) -> (Vec<String>, Vec<String>, Vec<Value>) {
    use std::collections::{HashMap, HashSet};
    let live_by_name: HashMap<&str, &Column> = live.iter().map(|c| (c.name.as_str(), c)).collect();
    let declared_names: HashSet<&str> = declared.iter().map(|c| c.name.as_str()).collect();

    let mut missing = Vec::new();
    let mut type_changes = Vec::new();
    for d in declared {
        match live_by_name.get(d.name.as_str()) {
            None => missing.push(d.name.clone()),
            Some(l) => {
                if l.data_type != d.data_type {
                    type_changes.push(json!({
                        "column": d.name,
                        "declared": d.data_type.name(),
                        "live": l.data_type.name(),
                    }));
                }
            }
        }
    }
    let added: Vec<String> = live
        .iter()
        .filter(|l| !declared_names.contains(l.name.as_str()))
        .map(|l| l.name.clone())
        .collect();
    (missing, added, type_changes)
}

/// Detect schema drift across a pipeline's source nodes. For each `src.*` node
/// that carries a declared schema, read the source's live schema and diff it.
/// Returns `{ ok, hasDrift, hasBreaking, sources: [...], summary: {...} }`.
///
/// A source is "breaking" when a declared column is missing from the source or
/// changed type (the pipeline expects something the data no longer provides).
/// Added columns are reported but non-breaking. Sources whose type cannot be
/// introspected (databases, REST, streams) or cannot be read (missing file,
/// unreachable) are reported but do not fail the drift verdict.
pub fn schema_drift(engine: &DuckdbEngine, doc: &PipelineDoc) -> Value {
    let mut sources = Vec::new();
    let mut checked = 0u64;
    let mut with_drift = 0u64;
    let mut breaking = 0u64;
    let mut not_introspectable = 0u64;
    let mut unreadable = 0u64;
    let mut no_declared = 0u64;

    for node in &doc.nodes {
        let cid = node.data.component_id.as_deref().unwrap_or("");
        if !cid.starts_with("src.") {
            continue;
        }
        let label = node.data.label.clone();
        let declared = match node.data.schema.as_deref() {
            Some(s) if !s.is_empty() => s,
            _ => {
                no_declared += 1;
                sources.push(json!({
                    "nodeId": node.id, "label": label, "componentId": cid,
                    "status": "no_declared_schema",
                    "note": "no declared schema to compare against (run Autodetect to capture one)",
                }));
                continue;
            }
        };
        let fmt = source_format(cid);
        let props = node.data.properties.clone().unwrap_or(Value::Null);
        match engine.inspect_via_run_database(&fmt, props) {
            Ok(insp) => {
                checked += 1;
                let (missing, added, type_changes) = compare_columns(declared, &insp.schema);
                let is_breaking = !missing.is_empty() || !type_changes.is_empty();
                let drift = is_breaking || !added.is_empty();
                if drift {
                    with_drift += 1;
                }
                if is_breaking {
                    breaking += 1;
                }
                sources.push(json!({
                    "nodeId": node.id, "label": label, "componentId": cid,
                    "status": if drift { "drift" } else { "match" },
                    "breaking": is_breaking,
                    "missingColumns": missing,
                    "addedColumns": added,
                    "typeChanges": type_changes,
                }));
            }
            Err(EngineError::Unsupported(_)) => {
                not_introspectable += 1;
                sources.push(json!({
                    "nodeId": node.id, "label": label, "componentId": cid,
                    "status": "not_introspectable",
                    "note": "live schema introspection is not available for this source type",
                }));
            }
            Err(e) => {
                unreadable += 1;
                sources.push(json!({
                    "nodeId": node.id, "label": label, "componentId": cid,
                    "status": "unreadable",
                    "note": format!("could not read the source: {e}"),
                }));
            }
        }
    }

    json!({
        "ok": true,
        "hasDrift": with_drift > 0,
        "hasBreaking": breaking > 0,
        "sources": sources,
        "summary": {
            "sourcesChecked": checked,
            "sourcesWithDrift": with_drift,
            "breakingSources": breaking,
            "notIntrospectable": not_introspectable,
            "unreadable": unreadable,
            "noDeclaredSchema": no_declared,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use duckle_metadata::DataType;

    fn col(name: &str, dt: DataType) -> Column {
        Column { name: name.into(), data_type: dt, nullable: true, primary_key: None, format: None }
    }

    #[test]
    fn inventory_contains_every_runner_routed_data_tool() {
        assert_eq!(
            RUN_DATABASE_TOOL_INVENTORY,
            [
                RunDatabaseTool::Inspect,
                RunDatabaseTool::SchemaDrift,
                RunDatabaseTool::BranchDiff,
            ]
        );
    }

    #[test]
    fn compare_detects_missing_added_and_type_change() {
        let declared = vec![
            col("id", DataType::Int64),
            col("email", DataType::String),
            col("gone", DataType::Date),
        ];
        let live = vec![
            col("id", DataType::Int32),
            col("email", DataType::String),
            col("extra", DataType::Bool),
        ];
        let (missing, added, changes) = compare_columns(&declared, &live);
        assert_eq!(missing, vec!["gone".to_string()]);
        assert_eq!(added, vec!["extra".to_string()]);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0]["column"], json!("id"));
        assert_eq!(changes[0]["declared"], json!("int64"));
        assert_eq!(changes[0]["live"], json!("int32"));
    }

    #[test]
    fn compare_identical_is_clean() {
        let cols = vec![col("a", DataType::String), col("b", DataType::Int64)];
        let (missing, added, changes) = compare_columns(&cols, &cols);
        assert!(missing.is_empty(), "{missing:?}");
        assert!(added.is_empty(), "{added:?}");
        assert!(changes.is_empty(), "{changes:?}");
    }

    #[test]
    fn source_format_strips_prefix_and_maps_aliases() {
        assert_eq!(source_format("src.csv"), "csv");
        assert_eq!(source_format("src.parquet"), "parquet");
        assert_eq!(source_format("src.ducklake"), "ducklake");
        assert_eq!(source_format("src.minio"), "s3");
        assert_eq!(source_format("src.r2"), "s3");
        assert_eq!(source_format("src.b2"), "s3");
    }
}
