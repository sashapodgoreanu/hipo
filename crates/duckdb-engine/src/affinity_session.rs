//! Persistent DuckDB CLI session used by Query Source affinity groups.
//!
//! The normal engine execution path intentionally starts one CLI process per
//! stage. An affinity group needs a different lifetime: DuckDB `ATTACH` state
//! must survive across multiple framed statements, but the worker must still
//! be cancellable and must not expose its stderr (which can contain connection
//! strings) to callers.

use crate::allow_unsigned_extensions;
use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use thiserror::Error;

const POLL_INTERVAL: Duration = Duration::from_millis(25);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Error)]
pub enum AffinitySessionError {
    #[error("DuckDB affinity worker is not installed")]
    MissingBinary,
    #[error("could not start DuckDB affinity worker")]
    Start,
    #[error("could not communicate with DuckDB affinity worker")]
    Communication,
    #[error("DuckDB affinity worker terminated before finishing its statement")]
    WorkerTerminated,
    #[error("DuckDB affinity worker was cancelled")]
    Cancelled,
}

/// Completion data for one worker statement. The sequence is monotonically
/// increasing within a session and can later be included in affinity events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatementCompletion {
    pub sequence: u64,
    pub duration_ms: u64,
}

/// One process-owned DuckDB session. Statements are serialized by the mutable
/// receiver: callers cannot issue concurrent SQL against one affinity group.
pub struct AffinitySession {
    child: Child,
    stdin: Option<ChildStdin>,
    stderr_reader: Option<JoinHandle<Vec<u8>>>,
    marker_dir: PathBuf,
    next_sequence: u64,
    attached_aliases: BTreeSet<String>,
    cancel: Arc<AtomicBool>,
}

impl std::fmt::Debug for AffinitySession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AffinitySession")
            .field("marker_dir", &self.marker_dir)
            .field("next_sequence", &self.next_sequence)
            .field("attached_aliases", &self.attached_aliases)
            .finish_non_exhaustive()
    }
}

impl AffinitySession {
    /// Spawn a persistent `duckdb -batch` process attached to the run database.
    /// Its stdout is deliberately discarded: completion is framed through
    /// run-local marker files, which avoids prompt/stdout protocol ambiguity.
    pub fn start(bin: &Path, db_path: &Path, cancel: Arc<AtomicBool>) -> Result<Self, AffinitySessionError> {
        if !bin.exists() {
            return Err(AffinitySessionError::MissingBinary);
        }
        let marker_dir = std::env::temp_dir().join(format!(
            "duckle_affinity_{}_{}_{}",
            std::process::id(),
            crate::now_nanos(),
            crate::RUN_SEQ.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&marker_dir).map_err(|_| AffinitySessionError::Start)?;

        let mut command = Command::new(bin);
        command
            .arg(db_path)
            .arg("-storage-version")
            .arg("v1.5.0")
            .arg("-no-init")
            .arg("-batch")
            .arg("-bail");
        if allow_unsigned_extensions() {
            command.arg("-unsigned");
        }
        command.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }

        let mut child = command.spawn().map_err(|_| AffinitySessionError::Start)?;
        let stdin = child.stdin.take().ok_or(AffinitySessionError::Start)?;
        let mut stderr = child.stderr.take().ok_or(AffinitySessionError::Start)?;
        let stderr_reader = std::thread::spawn(move || {
            use std::io::Read;
            let mut bytes = Vec::new();
            let _ = stderr.read_to_end(&mut bytes);
            bytes
        });

        Ok(Self {
            child,
            stdin: Some(stdin),
            stderr_reader: Some(stderr_reader),
            marker_dir,
            next_sequence: 1,
            attached_aliases: BTreeSet::new(),
            cancel,
        })
    }

    /// Execute SQL and wait until its marker lands. The marker statement is
    /// appended only after the caller SQL, so a DuckDB error with `-bail`
    /// terminates the worker before a false completion can be observed.
    pub fn execute(&mut self, sql: &str) -> Result<StatementCompletion, AffinitySessionError> {
        if self.cancel.load(Ordering::Relaxed) {
            self.terminate();
            return Err(AffinitySessionError::Cancelled);
        }
        if self.child.try_wait().map_err(|_| AffinitySessionError::Communication)?.is_some() {
            self.join_stderr();
            return Err(AffinitySessionError::WorkerTerminated);
        }

        let sequence = self.next_sequence;
        self.next_sequence += 1;
        let marker = self.marker_dir.join(format!("{sequence}.done"));
        let framed = frame_statement(sql, &marker, sequence);
        let stdin = self.stdin.as_mut().ok_or(AffinitySessionError::WorkerTerminated)?;
        stdin
            .write_all(framed.as_bytes())
            .and_then(|_| stdin.flush())
            .map_err(|_| AffinitySessionError::Communication)?;

        let started = Instant::now();
        loop {
            if marker_is_ready(&marker, sequence) {
                return Ok(StatementCompletion {
                    sequence,
                    duration_ms: started.elapsed().as_millis() as u64,
                });
            }
            if self.cancel.load(Ordering::Relaxed) {
                self.terminate();
                return Err(AffinitySessionError::Cancelled);
            }
            if self.child.try_wait().map_err(|_| AffinitySessionError::Communication)?.is_some() {
                self.join_stderr();
                return Err(AffinitySessionError::WorkerTerminated);
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    /// ATTACH a Data Source at most once in this process. `attach_sql` is
    /// intentionally accepted as already-resolved SQL: it may contain runtime
    /// credentials and is never recorded or returned by this module.
    pub fn attach_once(&mut self, alias: &str, attach_sql: &str) -> Result<bool, AffinitySessionError> {
        if self.attached_aliases.contains(alias) {
            return Ok(false);
        }
        self.execute(attach_sql)?;
        self.attached_aliases.insert(alias.to_string());
        Ok(true)
    }

    pub fn attached_aliases(&self) -> impl Iterator<Item = &str> {
        self.attached_aliases.iter().map(String::as_str)
    }

    /// Run a query in the worker and collect its JSON rows through a run-local
    /// file. This keeps schema/preview/count inspection in the same DuckDB
    /// process, avoiding a second CLI connection while the worker owns the
    /// run database lock.
    pub fn query_json_rows(&mut self, query: &str) -> Result<Vec<serde_json::Value>, AffinitySessionError> {
        let output = self.marker_dir.join(format!("result-{}.json", self.next_sequence));
        let output_path = output.display().to_string().replace('\\', "/").replace('\'', "''");
        self.execute(&format!(
            "COPY ({}) TO '{}' (FORMAT JSON, ARRAY false)",
            query.trim().trim_end_matches(';'),
            output_path,
        ))?;
        let contents = std::fs::read_to_string(&output).map_err(|_| AffinitySessionError::Communication)?;
        let _ = std::fs::remove_file(&output);
        contents
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).map_err(|_| AffinitySessionError::Communication))
            .collect()
    }

    /// Detach known aliases in reverse lexical order. This is best-effort at
    /// shutdown: a failed `DETACH` must not prevent process/file cleanup.
    pub fn detach_all(&mut self) {
        let aliases: Vec<String> = self.attached_aliases.iter().cloned().rev().collect();
        for alias in aliases {
            let _ = self.execute(&format!("DETACH {}", crate::plan::quote_ident(&alias)));
        }
        self.attached_aliases.clear();
    }

    /// Close stdin to let `-batch` exit naturally, then kill only if the CLI
    /// does not exit within the bounded cleanup window.
    pub fn close(&mut self) {
        self.detach_all();
        self.stdin.take();
        let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) | Err(_) => break,
                Ok(None) if Instant::now() >= deadline => {
                    self.terminate();
                    break;
                }
                Ok(None) => std::thread::sleep(POLL_INTERVAL),
            }
        }
        self.join_stderr();
        let _ = std::fs::remove_dir_all(&self.marker_dir);
    }

    fn terminate(&mut self) {
        let _ = self.stdin.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.join_stderr();
    }

    fn join_stderr(&mut self) {
        if let Some(reader) = self.stderr_reader.take() {
            // Stderr is intentionally discarded: DuckDB may echo secret-bearing
            // ATTACH parameters. A later redaction layer can emit safe details.
            let _ = reader.join();
        }
    }
}

impl Drop for AffinitySession {
    fn drop(&mut self) {
        self.close();
    }
}

fn frame_statement(sql: &str, marker: &Path, sequence: u64) -> String {
    let statement = sql.trim();
    let separator = if statement.ends_with(';') { "\n" } else { ";\n" };
    let marker_path = marker.display().to_string().replace('\\', "/").replace('\'', "''");
    format!("{statement}{separator}COPY (SELECT {sequence} AS _duckle_affinity_marker) TO '{marker_path}' (FORMAT CSV, HEADER FALSE);\n")
}

fn marker_is_ready(marker: &Path, sequence: u64) -> bool {
    std::fs::read_to_string(marker)
        .ok()
        .is_some_and(|contents| contents.trim() == sequence.to_string())
}

#[cfg(test)]
mod tests {
    use super::{frame_statement, AffinitySession};
    use std::path::PathBuf;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    #[test]
    fn statement_frame_terminates_sql_and_uses_escaped_marker_path() {
        let frame = frame_statement("SELECT 1", &PathBuf::from("C:/tmp/a'b.done"), 7);
        assert!(frame.starts_with("SELECT 1;\n"), "{frame}");
        assert!(frame.contains("SELECT 7 AS _duckle_affinity_marker"), "{frame}");
        assert!(frame.contains("a''b.done"), "{frame}");
    }

    #[test]
    fn persistent_worker_keeps_tables_between_framed_statements() {
        let Some(bin) = std::env::var_os("DUCKLE_DUCKDB_BIN").map(PathBuf::from) else {
            return;
        };
        let tmp = tempfile::tempdir().expect("tempdir");
        let db = tmp.path().join("run.duckdb");
        let cancel = Arc::new(AtomicBool::new(false));
        let mut session = AffinitySession::start(&bin, &db, cancel).expect("start worker");
        session.execute("CREATE TABLE t (id INTEGER)").expect("create table");
        session.execute("INSERT INTO t VALUES (1), (2)").expect("insert rows");
        session
            .execute("SELECT CASE WHEN (SELECT COUNT(*) FROM t) = 2 THEN 1 ELSE error('lost session state') END")
            .expect("persistent table state");
        session.close();
    }

    #[test]
    fn cancelled_worker_cleans_up_without_running_another_statement() {
        let Some(bin) = std::env::var_os("DUCKLE_DUCKDB_BIN").map(PathBuf::from) else {
            return;
        };
        let tmp = tempfile::tempdir().expect("tempdir");
        let db = tmp.path().join("run.duckdb");
        let cancel = Arc::new(AtomicBool::new(false));
        let mut session = AffinitySession::start(&bin, &db, cancel.clone()).expect("start worker");
        let marker_dir = session.marker_dir.clone();
        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(matches!(session.execute("SELECT 1"), Err(super::AffinitySessionError::Cancelled)));
        session.close();
        assert!(!marker_dir.exists(), "marker directory was not removed");
    }
}
