//! Controlled per-run Quack database operations.
//!
//! A `RunDatabase` never exposes a DuckDB connection, Quack endpoint, or
//! credential. The provider-private transport owns those details; callers can
//! only submit planner-produced SQL batches and receive sanitized results.

use crate::model::{RunCancellation, RunnerFailureReason, TransportKind};
use duckdb::types::{Value, ValueRef};
use duckdb::Connection;
use serde_json::Value as JsonValue;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlBatchResult {
    pub rows: u64,
    pub transport: TransportKind,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PreviewResult {
    pub columns: Vec<String>,
    pub rows: Vec<BTreeMap<String, JsonValue>>,
    pub truncated: bool,
    pub transport: TransportKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferResult {
    pub rows: u64,
    pub transport: TransportKind,
}

/// The provider-private implementation behind a run database. The trait does
/// not reveal its connection, endpoint, attachment name, or credential.
pub(crate) trait RunDatabaseTransport: Send + Sync {
    fn execute_batch(
        &self,
        statements: &[String],
        cancellation: &RunCancellation,
    ) -> Result<SqlBatchResult, RunnerFailureReason>;

    fn preview(
        &self,
        sql: &str,
        limit: u32,
        cancellation: &RunCancellation,
    ) -> Result<PreviewResult, RunnerFailureReason>;

    fn transfer(
        &self,
        sql: &str,
        transport: TransportKind,
        cancellation: &RunCancellation,
    ) -> Result<TransferResult, RunnerFailureReason>;
}

/// Per-run database facade used by the executor. It accepts only operations
/// already planned by Duckle and maps every transport failure to a sanitized
/// reason code.
pub struct RunDatabase {
    transport: Arc<dyn RunDatabaseTransport>,
    cancellation: RunCancellation,
}

impl RunDatabase {
    pub(crate) fn new(
        transport: Arc<dyn RunDatabaseTransport>,
        cancellation: RunCancellation,
    ) -> Self {
        Self {
            transport,
            cancellation,
        }
    }

    pub fn execute_batch(
        &self,
        statements: Vec<String>,
    ) -> Result<SqlBatchResult, RunnerFailureReason> {
        self.require_active()?;
        self.transport.execute_batch(&statements, &self.cancellation)
    }

    /// Server-side setup is intentionally a normal batch so TEMP/SET state
    /// stays in the same planned request as the stage that consumes it.
    pub fn setup(&self, statements: Vec<String>) -> Result<SqlBatchResult, RunnerFailureReason> {
        self.execute_batch(statements)
    }

    pub fn preview(&self, sql: &str, limit: u32) -> Result<PreviewResult, RunnerFailureReason> {
        self.require_active()?;
        self.transport.preview(sql, limit, &self.cancellation)
    }

    pub fn transfer(
        &self,
        sql: &str,
        transport: TransportKind,
    ) -> Result<TransferResult, RunnerFailureReason> {
        self.require_active()?;
        self.transport.transfer(sql, transport, &self.cancellation)
    }

    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    pub(crate) fn cancellation(&self) -> &RunCancellation {
        &self.cancellation
    }

    fn require_active(&self) -> Result<(), RunnerFailureReason> {
        if self.cancellation.is_cancelled() {
            Err(RunnerFailureReason::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// Provider-private Quack implementation. Construction occurs only after the
/// worker has authenticated and created its private secret and attachment.
pub(crate) struct QuackTransport {
    connection: Mutex<Connection>,
    attachment_alias: String,
}

impl QuackTransport {
    pub(crate) fn from_attached_connection(
        connection: Connection,
        attachment_alias: String,
    ) -> Result<Self, RunnerFailureReason> {
        if !is_safe_identifier(&attachment_alias) {
            return Err(RunnerFailureReason::RunnerUnavailable);
        }
        Ok(Self {
            connection: Mutex::new(connection),
            attachment_alias,
        })
    }

    fn remote_relation(&self, sql: &str) -> String {
        format!(
            "FROM quack_query_by_name({}, {})",
            sql_literal(&self.attachment_alias),
            sql_literal(sql)
        )
    }

    fn connection(&self) -> Result<std::sync::MutexGuard<'_, Connection>, RunnerFailureReason> {
        self.connection
            .lock()
            .map_err(|_| RunnerFailureReason::RunnerUnavailable)
    }
}

#[cfg(debug_assertions)]
fn debug_duckdb_error(operation: &str, error: &duckdb::Error) {
    if std::env::var("DUCKLE_DEBUG_RUNNER_ERRORS").as_deref() == Ok("1") {
        eprintln!("[duckle-runner-debug] {operation}: {error}");
    }
}

#[cfg(not(debug_assertions))]
fn debug_duckdb_error(_operation: &str, _error: &duckdb::Error) {}

impl RunDatabaseTransport for QuackTransport {
    fn execute_batch(
        &self,
        statements: &[String],
        cancellation: &RunCancellation,
    ) -> Result<SqlBatchResult, RunnerFailureReason> {
        let mut rows = 0_u64;
        for statement in statements {
            if cancellation.is_cancelled() {
                return Err(RunnerFailureReason::Cancelled);
            }
            let query = format!("SELECT count(*)::BIGINT FROM ({})", self.remote_relation(statement));
            let count: i64 = match self
                .connection()?
                .query_row(&query, [], |row| row.get(0))
            {
                Ok(count) => count,
                Err(error) => {
                    debug_duckdb_error("execute_batch.query_row", &error);
                    return Err(RunnerFailureReason::RunnerCrashed);
                }
            };
            rows = rows.saturating_add(count.max(0) as u64);
        }
        Ok(SqlBatchResult {
            rows,
            transport: TransportKind::Quack,
        })
    }

    fn preview(
        &self,
        sql: &str,
        limit: u32,
        cancellation: &RunCancellation,
    ) -> Result<PreviewResult, RunnerFailureReason> {
        if cancellation.is_cancelled() {
            return Err(RunnerFailureReason::Cancelled);
        }
        let query = format!(
            "SELECT * FROM ({}) LIMIT {}",
            self.remote_relation(sql),
            limit.saturating_add(1)
        );
        let connection = self.connection()?;
        let mut statement = match connection.prepare(&query) {
            Ok(statement) => statement,
            Err(error) => {
                debug_duckdb_error("preview.prepare", &error);
                return Err(RunnerFailureReason::RunnerCrashed);
            }
        };
        let columns = (0..statement.column_count())
            .map(|index| statement.column_name(index).map_or("?", String::as_str).to_string())
            .collect::<Vec<_>>();
        let mut result_rows = match statement.query([]) {
            Ok(rows) => rows,
            Err(error) => {
                debug_duckdb_error("preview.query", &error);
                return Err(RunnerFailureReason::RunnerCrashed);
            }
        };
        let mut rows = Vec::new();
        loop {
            let next = match result_rows.next() {
                Ok(next) => next,
                Err(error) => {
                    debug_duckdb_error("preview.next", &error);
                    return Err(RunnerFailureReason::RunnerCrashed);
                }
            };
            let Some(row) = next else {
                break;
            };
            if cancellation.is_cancelled() {
                return Err(RunnerFailureReason::Cancelled);
            }
            if rows.len() == limit as usize {
                return Ok(PreviewResult {
                    columns,
                    rows,
                    truncated: true,
                    transport: TransportKind::Quack,
                });
            }
            let mut values = BTreeMap::new();
            for (index, column) in columns.iter().enumerate() {
                let value = match row.get_ref(index) {
                    Ok(value) => value,
                    Err(error) => {
                        debug_duckdb_error("preview.get_ref", &error);
                        return Err(RunnerFailureReason::RunnerCrashed);
                    }
                };
                values.insert(column.clone(), value_to_json(value));
            }
            rows.push(values);
        }
        Ok(PreviewResult {
            columns,
            rows,
            truncated: false,
            transport: TransportKind::Quack,
        })
    }

    fn transfer(
        &self,
        sql: &str,
        transport: TransportKind,
        cancellation: &RunCancellation,
    ) -> Result<TransferResult, RunnerFailureReason> {
        let result = self.execute_batch(&[sql.to_string()], cancellation)?;
        Ok(TransferResult {
            rows: result.rows,
            transport,
        })
    }
}

fn is_safe_identifier(value: &str) -> bool {
    !value.is_empty() && value.chars().all(|character| character.is_ascii_alphanumeric() || character == '_')
}

fn sql_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn value_to_json(value: ValueRef<'_>) -> JsonValue {
    match value.to_owned() {
        Value::Null => JsonValue::Null,
        Value::Boolean(value) => value.into(),
        Value::TinyInt(value) => value.into(),
        Value::SmallInt(value) => value.into(),
        Value::Int(value) => value.into(),
        Value::BigInt(value) => value.into(),
        Value::UTinyInt(value) => value.into(),
        Value::USmallInt(value) => value.into(),
        Value::UInt(value) => value.into(),
        Value::UBigInt(value) => value.into(),
        Value::Float(value) => serde_json::Number::from_f64(value as f64)
            .map(JsonValue::Number)
            .unwrap_or(JsonValue::Null),
        Value::Double(value) => serde_json::Number::from_f64(value)
            .map(JsonValue::Number)
            .unwrap_or(JsonValue::Null),
        Value::Text(value) | Value::Enum(value) => value.into(),
        other => format!("{other:?}").into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct RecordingTransport {
        batches: Mutex<Vec<Vec<String>>>,
    }

    impl RunDatabaseTransport for RecordingTransport {
        fn execute_batch(
            &self,
            statements: &[String],
            _cancellation: &RunCancellation,
        ) -> Result<SqlBatchResult, RunnerFailureReason> {
            self.batches.lock().unwrap().push(statements.to_vec());
            Ok(SqlBatchResult {
                rows: statements.len() as u64,
                transport: TransportKind::Quack,
            })
        }

        fn preview(
            &self,
            _sql: &str,
            _limit: u32,
            _cancellation: &RunCancellation,
        ) -> Result<PreviewResult, RunnerFailureReason> {
            Ok(PreviewResult {
                columns: vec!["value".to_string()],
                rows: vec![BTreeMap::new()],
                truncated: false,
                transport: TransportKind::Quack,
            })
        }

        fn transfer(
            &self,
            _sql: &str,
            transport: TransportKind,
            _cancellation: &RunCancellation,
        ) -> Result<TransferResult, RunnerFailureReason> {
            Ok(TransferResult { rows: 1, transport })
        }
    }

    #[test]
    fn setup_keeps_its_statements_in_one_controlled_batch() {
        let transport = Arc::new(RecordingTransport::default());
        let database = RunDatabase::new(transport.clone(), RunCancellation::default());

        let result = database
            .setup(vec!["SET threads = 2".to_string(), "CREATE TEMP TABLE t AS SELECT 1".to_string()])
            .unwrap();

        assert_eq!(result.rows, 2);
        assert_eq!(transport.batches.lock().unwrap().as_slice(), &[vec![
            "SET threads = 2".to_string(),
            "CREATE TEMP TABLE t AS SELECT 1".to_string(),
        ]]);
    }

    #[test]
    fn cancellation_prevents_database_dispatch() {
        let transport = Arc::new(RecordingTransport::default());
        let database = RunDatabase::new(transport.clone(), RunCancellation::default());
        database.cancel();

        assert_eq!(
            database.execute_batch(vec!["SELECT 1".to_string()]),
            Err(RunnerFailureReason::Cancelled)
        );
        assert!(transport.batches.lock().unwrap().is_empty());
    }
}
