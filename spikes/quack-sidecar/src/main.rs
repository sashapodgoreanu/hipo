use duckdb::types::{Value, ValueRef};
use duckdb::Connection;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::env;
use std::fs;
use std::net::TcpListener;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Barrier, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const SPIKE_PROTOCOL_VERSION: u32 = 1;
const DUCKDB_VERSION: &str = "1.5.4";

type SpikeResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReadyInfo {
    protocol_version: u32,
    duckdb_version: String,
    pid: u32,
    uri: String,
    url: String,
    token: String,
    storage: String,
    temp_directory: String,
}

#[derive(Debug, Serialize)]
struct CheckResult {
    name: String,
    duration_ms: u128,
    details: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct BenchmarkResult {
    storage: String,
    startup_ms: u128,
    load_ms: u128,
    quack_transfer_ms: u128,
    parquet_export_ms: u128,
    parquet_read_ms: u128,
    shutdown_ms: u128,
    parquet_bytes: u64,
    database_bytes: u64,
    rows: i64,
    payload_bytes: i64,
}

struct QuackClient {
    connection: Connection,
    ready: ReadyInfo,
}

struct RunDirectory(PathBuf);

impl Drop for RunDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct SidecarChild(Child);

struct WarmWorker {
    // These guards intentionally have no direct callers: dropping them is the
    // LocalProcessProvider prototype's terminate-and-cleanup operation.
    _child: SidecarChild,
    _run_directory: RunDirectory,
    ready: ReadyInfo,
}

struct PoolState {
    idle: VecDeque<WarmWorker>,
    leased: usize,
    starting: usize,
}

struct PoolInner {
    target_size: usize,
    memory_limit: String,
    state: Mutex<PoolState>,
    available: Condvar,
    shutdown: AtomicBool,
}

#[derive(Clone)]
struct PoolHandle {
    inner: Arc<PoolInner>,
}

struct PrewarmPool {
    handle: PoolHandle,
}

struct WorkerLease {
    pipeline_run_id: String,
    worker: Option<WarmWorker>,
    pool: Arc<PoolInner>,
}

impl Deref for SidecarChild {
    type Target = Child;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for SidecarChild {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for SidecarChild {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

impl WarmWorker {
    fn pid(&self) -> u32 {
        self.ready.pid
    }

    fn client(&self) -> SpikeResult<QuackClient> {
        QuackClient::connect(self.ready.clone())
    }
}

impl PrewarmPool {
    fn new(target_size: usize, memory_limit: &str) -> SpikeResult<Self> {
        if target_size == 0 {
            return Err("prewarm pool size must be greater than zero".into());
        }
        let inner = Arc::new(PoolInner {
            target_size,
            memory_limit: memory_limit.into(),
            state: Mutex::new(PoolState {
                idle: VecDeque::new(),
                leased: 0,
                starting: target_size,
            }),
            available: Condvar::new(),
            shutdown: AtomicBool::new(false),
        });

        let (sender, receiver) = mpsc::channel();
        for _ in 0..target_size {
            let sender = sender.clone();
            let memory_limit = memory_limit.to_string();
            thread::spawn(move || {
                let _ = sender.send(start_warm_worker(&memory_limit));
            });
        }
        drop(sender);

        let mut first_error = None;
        for result in receiver {
            let mut state = inner
                .state
                .lock()
                .map_err(|_| "prewarm pool state lock poisoned")?;
            state.starting = state.starting.saturating_sub(1);
            match result {
                Ok(worker) => state.idle.push_back(worker),
                Err(error) if first_error.is_none() => first_error = Some(error),
                Err(_) => {}
            }
        }
        if let Some(error) = first_error {
            inner.shutdown.store(true, Ordering::Release);
            return Err(error);
        }
        Ok(Self {
            handle: PoolHandle { inner },
        })
    }

    fn handle(&self) -> PoolHandle {
        self.handle.clone()
    }

    fn checkout(
        &self,
        pipeline_run_id: impl Into<String>,
        timeout: Duration,
    ) -> SpikeResult<WorkerLease> {
        self.handle.checkout(pipeline_run_id, timeout)
    }

    fn wait_until_idle(&self, expected: usize, timeout: Duration) -> SpikeResult<Vec<u32>> {
        let deadline = Instant::now() + timeout;
        let mut state = self
            .handle
            .inner
            .state
            .lock()
            .map_err(|_| "prewarm pool state lock poisoned")?;
        loop {
            if state.idle.len() == expected && state.leased == 0 && state.starting == 0 {
                return Ok(state.idle.iter().map(WarmWorker::pid).collect());
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(format!(
                    "timed out waiting for pool idle: idle={}, leased={}, starting={}",
                    state.idle.len(),
                    state.leased,
                    state.starting
                )
                .into());
            }
            let (next, wait) = self
                .handle
                .inner
                .available
                .wait_timeout(state, remaining)
                .map_err(|_| "prewarm pool state lock poisoned")?;
            state = next;
            if wait.timed_out() && state.idle.len() != expected {
                return Err("timed out waiting for replacement workers".into());
            }
        }
    }
}

impl Drop for PrewarmPool {
    fn drop(&mut self) {
        self.handle.inner.shutdown.store(true, Ordering::Release);
        self.handle.inner.available.notify_all();
        if let Ok(mut state) = self.handle.inner.state.lock() {
            state.idle.clear();
        }
    }
}

impl PoolHandle {
    fn checkout(
        &self,
        pipeline_run_id: impl Into<String>,
        timeout: Duration,
    ) -> SpikeResult<WorkerLease> {
        let deadline = Instant::now() + timeout;
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| "prewarm pool state lock poisoned")?;
        loop {
            if self.inner.shutdown.load(Ordering::Acquire) {
                return Err("prewarm pool is shutting down".into());
            }
            if let Some(worker) = state.idle.pop_front() {
                state.leased += 1;
                return Ok(WorkerLease {
                    pipeline_run_id: pipeline_run_id.into(),
                    worker: Some(worker),
                    pool: Arc::clone(&self.inner),
                });
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err("timed out waiting for an available pipeline worker".into());
            }
            let (next, wait) = self
                .inner
                .available
                .wait_timeout(state, remaining)
                .map_err(|_| "prewarm pool state lock poisoned")?;
            state = next;
            if wait.timed_out() && state.idle.is_empty() {
                return Err("timed out waiting for an available pipeline worker".into());
            }
        }
    }
}

impl WorkerLease {
    fn pipeline_run_id(&self) -> &str {
        &self.pipeline_run_id
    }

    fn pid(&self) -> u32 {
        self.worker.as_ref().expect("live worker lease").pid()
    }

    fn client(&self) -> SpikeResult<QuackClient> {
        self.worker.as_ref().expect("live worker lease").client()
    }
}

impl Drop for WorkerLease {
    fn drop(&mut self) {
        // A pipeline worker is deliberately single-use. Killing it is the only
        // strong reset for catalog, secrets, attachments, transactions and
        // extension state. A fresh empty worker replaces it asynchronously.
        drop(self.worker.take());
        let should_replenish = if let Ok(mut state) = self.pool.state.lock() {
            state.leased = state.leased.saturating_sub(1);
            if !self.pool.shutdown.load(Ordering::Acquire) {
                state.starting += 1;
                true
            } else {
                false
            }
        } else {
            false
        };
        self.pool.available.notify_all();
        if should_replenish {
            spawn_replacement(Arc::clone(&self.pool));
        }
    }
}

impl QuackClient {
    fn connect(ready: ReadyInfo) -> SpikeResult<Self> {
        Self::connect_with_http_timeout(ready, 30)
    }

    fn connect_with_http_timeout(ready: ReadyInfo, timeout_seconds: u64) -> SpikeResult<Self> {
        validate_ready(&ready)?;
        let connection = Connection::open_in_memory()?;
        load_quack(&connection)?;
        connection.execute_batch(&format!(
            "SET httpfs_connection_caching = true; \
             SET http_timeout = {timeout_seconds}; \
             SET http_retries = 0;"
        ))?;
        Ok(Self { connection, ready })
    }

    fn invocation(&self, sql: &str) -> String {
        format!(
            "FROM quack_query({}, {}, token => {})",
            sql_literal(&self.ready.uri),
            sql_literal(sql),
            sql_literal(&self.ready.token)
        )
    }

    fn execute_remote(&self, sql: &str) -> SpikeResult<u64> {
        let query = format!("SELECT count(*) FROM ({})", self.invocation(sql));
        let count: i64 = self.connection.query_row(&query, [], |row| row.get(0))?;
        Ok(count.max(0) as u64)
    }

    fn scalar_i64(&self, sql: &str) -> SpikeResult<i64> {
        let query = self.invocation(sql);
        Ok(self.connection.query_row(&query, [], |row| row.get(0))?)
    }

    fn query_json(&self, sql: &str) -> SpikeResult<serde_json::Value> {
        let query = self.invocation(sql);
        let mut statement = self.connection.prepare(&query)?;
        let column_count = statement.column_count();
        let columns: Vec<String> = (0..column_count)
            .map(|index| {
                statement
                    .column_name(index)
                    .map_or("?", String::as_str)
                    .to_string()
            })
            .collect();
        let mut rows = statement.query([])?;
        let mut output = Vec::new();
        while let Some(row) = rows.next()? {
            let mut item = serde_json::Map::new();
            for (index, name) in columns.iter().enumerate() {
                item.insert(name.clone(), value_to_json(row.get_ref(index)?));
            }
            output.push(serde_json::Value::Object(item));
        }
        Ok(serde_json::Value::Array(output))
    }

    fn attach_remote(&self, alias: &str) -> SpikeResult<()> {
        if !alias.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err("attachment alias contains unsupported characters".into());
        }
        self.connection.execute_batch(&format!(
            "ATTACH {} AS {} (TYPE quack, TOKEN {});",
            sql_literal(&self.ready.uri),
            alias,
            sql_literal(&self.ready.token)
        ))?;
        Ok(())
    }
}

fn value_to_json(value: ValueRef<'_>) -> serde_json::Value {
    match value.to_owned() {
        Value::Null => serde_json::Value::Null,
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
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Value::Double(value) => serde_json::Number::from_f64(value)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Value::Text(value) | Value::Enum(value) => value.into(),
        other => format!("{other:?}").into(),
    }
}

fn sql_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn load_quack(connection: &Connection) -> SpikeResult<()> {
    if connection.execute_batch("LOAD quack;").is_err() {
        connection.execute_batch("INSTALL quack; LOAD quack;")?;
    }
    Ok(())
}

fn validate_ready(ready: &ReadyInfo) -> SpikeResult<()> {
    if ready.protocol_version != SPIKE_PROTOCOL_VERSION {
        return Err(format!(
            "protocol mismatch: client={}, server={}",
            SPIKE_PROTOCOL_VERSION, ready.protocol_version
        )
        .into());
    }
    if ready.duckdb_version != DUCKDB_VERSION {
        return Err(format!(
            "DuckDB mismatch: client={}, server={}",
            DUCKDB_VERSION, ready.duckdb_version
        )
        .into());
    }
    if !ready.uri.starts_with("quack:127.0.0.1:") {
        return Err(format!("sidecar is not loopback-bound: {}", ready.uri).into());
    }
    Ok(())
}

fn read_ready(path: &Path) -> SpikeResult<ReadyInfo> {
    let ready: ReadyInfo = serde_json::from_slice(&fs::read(path)?)?;
    validate_ready(&ready)?;
    Ok(ready)
}

fn write_ready_atomic(path: &Path, ready: &ReadyInfo) -> SpikeResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, serde_json::to_vec_pretty(ready)?)?;
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(temporary, path)?;
    Ok(())
}

fn serve(arguments: &[String]) -> SpikeResult<()> {
    let ready_path = required_path(arguments, "--ready")?;
    let port = optional_value(arguments, "--port")
        .map(|value| value.parse::<u16>())
        .transpose()?
        .unwrap_or(9494);
    let database = optional_value(arguments, "--database").unwrap_or_else(|| ":memory:".into());
    let memory_limit = optional_value(arguments, "--memory-limit").unwrap_or_else(|| "64MB".into());
    let temp_directory = optional_value(arguments, "--temp-directory")
        .map(PathBuf::from)
        .unwrap_or_else(|| ready_path.parent().unwrap_or(Path::new(".")).join("spill"));
    fs::create_dir_all(&temp_directory)?;

    let connection = if database == ":memory:" {
        Connection::open_in_memory()?
    } else {
        if let Some(parent) = Path::new(&database).parent() {
            fs::create_dir_all(parent)?;
        }
        Connection::open(&database)?
    };
    connection.execute_batch(&format!(
        "SET memory_limit = {}; SET temp_directory = {}; \
         SET threads = 2; SET preserve_insertion_order = false;",
        sql_literal(&memory_limit),
        sql_literal(&temp_directory.to_string_lossy())
    ))?;
    load_quack(&connection)?;

    let requested_uri = format!("quack:127.0.0.1:{port}");
    let call = format!("CALL quack_serve({})", sql_literal(&requested_uri));
    let (uri, url, token): (String, String, String) =
        connection.query_row(&call, [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
    let version: String = connection.query_row("SELECT version()", [], |row| row.get(0))?;
    let ready = ReadyInfo {
        protocol_version: SPIKE_PROTOCOL_VERSION,
        duckdb_version: version.trim_start_matches('v').to_string(),
        pid: std::process::id(),
        uri,
        url,
        token,
        storage: database,
        temp_directory: temp_directory.to_string_lossy().into_owned(),
    };
    write_ready_atomic(&ready_path, &ready)?;

    // The Quack extension owns the HTTP listener. Keeping this process and its
    // embedded connection alive keeps the complete run database alive. The
    // supervisor cancels by killing this process; there is no SQL-data stdin.
    loop {
        thread::park_timeout(Duration::from_secs(60));
    }
}

fn query(arguments: &[String]) -> SpikeResult<()> {
    let ready = read_ready(&required_path(arguments, "--ready")?)?;
    let sql = optional_value(arguments, "--sql").ok_or("missing --sql")?;
    let client = QuackClient::connect(ready)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&client.query_json(&sql)?)?
    );
    Ok(())
}

fn smoke() -> SpikeResult<()> {
    let run_dir = unique_run_directory();
    fs::create_dir_all(&run_dir)?;
    let _run_directory = RunDirectory(run_dir.clone());
    let ready_path = run_dir.join("ready.json");
    let spill_path = run_dir.join("spill");
    let port = reserve_loopback_port()?;

    let started = Instant::now();
    let mut child = SidecarChild(spawn_sidecar(&ready_path, &spill_path, port, ":memory:")?);
    let ready = match wait_for_ready(&ready_path, &mut child, Duration::from_secs(90)) {
        Ok(ready) => ready,
        Err(error) => {
            return Err(error);
        }
    };
    let mut checks = vec![check("startup", started, [("pid", ready.pid.into())])];

    let client = QuackClient::connect(ready.clone())?;
    let started = Instant::now();
    client.execute_remote(
        "CREATE TABLE facts AS \
         SELECT i, \
                md5(i::VARCHAR) || md5((i + 10000000)::VARCHAR) || \
                md5((i + 20000000)::VARCHAR) || md5((i + 30000000)::VARCHAR) AS payload \
         FROM range(2000000) t(i)",
    )?;
    let sum = client.scalar_i64("SELECT sum(i)::BIGINT FROM facts")?;
    if sum != 1_999_999_000_000 {
        return Err(format!("unexpected remote sum: {sum}").into());
    }
    checks.push(check(
        "remote_query_and_write",
        started,
        [("sum", sum.into())],
    ));

    let started = Instant::now();
    let barrier = Arc::new(Barrier::new(3));
    let mut readers = Vec::new();
    for _ in 0..2 {
        let ready = ready.clone();
        let barrier = Arc::clone(&barrier);
        readers.push(thread::spawn(move || -> SpikeResult<i64> {
            let client = QuackClient::connect(ready)?;
            barrier.wait();
            client.scalar_i64("SELECT sum(i)::BIGINT FROM facts")
        }));
    }
    barrier.wait();
    for reader in readers {
        if reader.join().map_err(|_| "parallel reader panicked")?? != sum {
            return Err("parallel reader returned the wrong value".into());
        }
    }
    checks.push(check("parallel_reads_2", started, []));

    client.execute_remote("CREATE TABLE appends(client INTEGER, i BIGINT)")?;
    let started = Instant::now();
    let barrier = Arc::new(Barrier::new(3));
    let mut writers = Vec::new();
    for writer_id in 0..2 {
        let ready = ready.clone();
        let barrier = Arc::clone(&barrier);
        writers.push(thread::spawn(move || -> SpikeResult<()> {
            let client = QuackClient::connect(ready)?;
            barrier.wait();
            client.execute_remote(&format!(
                "INSERT INTO appends SELECT {writer_id}, i FROM range(100000) t(i)"
            ))?;
            Ok(())
        }));
    }
    barrier.wait();
    for writer in writers {
        writer.join().map_err(|_| "parallel writer panicked")??;
    }
    let appended = client.scalar_i64("SELECT count(*)::BIGINT FROM appends")?;
    if appended != 200_000 {
        return Err(format!("parallel append lost rows: {appended}").into());
    }
    checks.push(check(
        "parallel_appends_same_table_2",
        started,
        [("rows", appended.into())],
    ));

    let started = Instant::now();
    client.attach_remote("run_db")?;
    let attached_count: i64 =
        client
            .connection
            .query_row("SELECT count(*)::BIGINT FROM run_db.facts", [], |row| {
                row.get(0)
            })?;
    if attached_count != 2_000_000 {
        return Err(format!("client attachment returned {attached_count} rows").into());
    }
    let external_path = run_dir.join("external.duckdb");
    let external = Connection::open(&external_path)?;
    external.execute_batch("CREATE TABLE external_table AS SELECT 7 AS value")?;
    drop(external);
    client.execute_remote(&format!(
        "ATTACH {} AS external; CREATE TABLE attached_copy AS SELECT * FROM external.external_table",
        sql_literal(&external_path.to_string_lossy())
    ))?;
    let attached_value = client.scalar_i64("SELECT value::BIGINT FROM attached_copy")?;
    if attached_value != 7 {
        return Err("server-side attachment returned the wrong value".into());
    }
    checks.push(check("attachments", started, []));

    let started = Instant::now();
    let (spill_sender, spill_receiver) = mpsc::channel();
    let spill_ready = ready.clone();
    thread::spawn(move || {
        let result = QuackClient::connect(spill_ready).and_then(|client| {
            client.execute_remote(
                "CREATE TABLE spill_sorted AS SELECT * FROM facts ORDER BY payload DESC, i DESC",
            )
        });
        let _ = spill_sender.send(result);
    });
    let mut peak_spill_bytes = 0_u64;
    let spill_result = loop {
        peak_spill_bytes = peak_spill_bytes.max(directory_size(&spill_path)?);
        match spill_receiver.try_recv() {
            Ok(result) => break result,
            Err(mpsc::TryRecvError::Empty) => thread::sleep(Duration::from_millis(2)),
            Err(mpsc::TryRecvError::Disconnected) => {
                return Err("spill worker disconnected before returning a result".into());
            }
        }
    };
    spill_result?;
    let spill_rows = client.scalar_i64("SELECT count(*)::BIGINT FROM spill_sorted")?;
    if spill_rows != 2_000_000 {
        return Err(format!("spill workload returned {spill_rows} rows").into());
    }
    if peak_spill_bytes == 0 {
        return Err("bounded-memory workload completed without observable spill files".into());
    }
    checks.push(check(
        "bounded_memory_sort",
        started,
        [
            ("memory_limit_mb", 128.into()),
            ("rows", spill_rows.into()),
            ("peak_spill_bytes", peak_spill_bytes.into()),
        ],
    ));

    let started = Instant::now();
    let (sender, receiver) = mpsc::channel();
    let cancelling_ready = ready.clone();
    thread::spawn(move || {
        let result =
            QuackClient::connect_with_http_timeout(cancelling_ready, 2).and_then(|client| {
                client.scalar_i64(
                    "SELECT sum(hash(a.i + b.i))::BIGINT \
                 FROM range(1000000) a(i), range(1000000) b(i)",
                )
            });
        let _ = sender.send(result.map(|_| ()));
    });
    thread::sleep(Duration::from_millis(150));
    let kill_started = Instant::now();
    child.kill()?;
    child.wait()?;
    let process_exit_ms = kill_started.elapsed().as_millis();
    let cancellation = receiver.recv_timeout(Duration::from_secs(10))?;
    let client_unblock_ms = kill_started.elapsed().as_millis();
    if cancellation.is_ok() {
        return Err("long remote query unexpectedly completed before sidecar kill".into());
    }
    checks.push(check(
        "kill_cancellation",
        started,
        [
            ("process_exit_ms", (process_exit_ms as u64).into()),
            ("client_unblock_ms", (client_unblock_ms as u64).into()),
        ],
    ));

    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "status": "ok",
            "duckdb_version": DUCKDB_VERSION,
            "protocol_version": SPIKE_PROTOCOL_VERSION,
            "checks": checks
        }))?
    );
    Ok(())
}

fn benchmark() -> SpikeResult<()> {
    const ROWS: i64 = 2_000_000;
    let mut results = Vec::new();
    for storage in ["memory", "file"] {
        let run_dir = unique_run_directory();
        fs::create_dir_all(&run_dir)?;
        let _run_directory = RunDirectory(run_dir.clone());
        let ready_path = run_dir.join("ready.json");
        let spill_path = run_dir.join("spill");
        let database_path = run_dir.join("run.duckdb");
        let database = if storage == "memory" {
            ":memory:".to_string()
        } else {
            database_path.to_string_lossy().into_owned()
        };
        let port = reserve_loopback_port()?;

        let started = Instant::now();
        let mut child = SidecarChild(spawn_sidecar_with_limit(
            &ready_path,
            &spill_path,
            port,
            &database,
            "512MB",
        )?);
        let ready = wait_for_ready(&ready_path, &mut child, Duration::from_secs(90))?;
        let startup_ms = started.elapsed().as_millis();
        let client = QuackClient::connect(ready)?;

        let started = Instant::now();
        client.execute_remote(&format!(
            "CREATE TABLE bench AS \
             SELECT i, md5(i::VARCHAR) AS payload FROM range({ROWS}) t(i)"
        ))?;
        let load_ms = started.elapsed().as_millis();
        let started = Instant::now();
        let quack_query = format!(
            "SELECT count(*)::BIGINT, sum(i)::BIGINT, \
             sum(length(payload))::BIGINT FROM ({})",
            client.invocation("SELECT i, payload FROM bench")
        );
        let quack_result: (i64, i64, i64) =
            client.connection.query_row(&quack_query, [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?;
        let quack_transfer_ms = started.elapsed().as_millis();
        if quack_result.0 != ROWS {
            return Err(format!("Quack transfer lost rows: {}", quack_result.0).into());
        }

        let parquet_path = run_dir.join("snapshot.parquet");
        let started = Instant::now();
        client.execute_remote(&format!(
            "COPY bench TO {} (FORMAT PARQUET, COMPRESSION ZSTD)",
            sql_literal(&parquet_path.to_string_lossy())
        ))?;
        let parquet_export_ms = started.elapsed().as_millis();
        let parquet_bytes = fs::metadata(&parquet_path)?.len();

        let started = Instant::now();
        let parquet_query = format!(
            "SELECT count(*)::BIGINT, sum(i)::BIGINT, sum(length(payload))::BIGINT \
             FROM read_parquet({})",
            sql_literal(&parquet_path.to_string_lossy())
        );
        let parquet_result: (i64, i64, i64) =
            client.connection.query_row(&parquet_query, [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?;
        let parquet_read_ms = started.elapsed().as_millis();
        if parquet_result != quack_result {
            return Err(format!(
                "Parquet result differs from Quack: {parquet_result:?} != {quack_result:?}"
            )
            .into());
        }

        let started = Instant::now();
        child.kill()?;
        child.wait()?;
        let shutdown_ms = started.elapsed().as_millis();
        let database_bytes = fs::metadata(&database_path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);

        results.push(BenchmarkResult {
            storage: storage.into(),
            startup_ms,
            load_ms,
            quack_transfer_ms,
            parquet_export_ms,
            parquet_read_ms,
            shutdown_ms,
            parquet_bytes,
            database_bytes,
            rows: quack_result.0,
            payload_bytes: quack_result.2,
        });
    }

    let binary_bytes = fs::metadata(env::current_exe()?)?.len();
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "status": "ok",
            "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
            "duckdb_version": DUCKDB_VERSION,
            "binary_bytes": binary_bytes,
            "results": results
        }))?
    );
    Ok(())
}

fn pool_smoke() -> SpikeResult<()> {
    const POOL_SIZE: usize = 3;
    const WORKER_MEMORY_MB: u64 = 128;

    let started = Instant::now();
    let cold_worker = start_warm_worker("128MB")?;
    let cold_start_ms = started.elapsed().as_millis();
    let cold_pid = cold_worker.pid();
    drop(cold_worker);

    let started = Instant::now();
    let pool = PrewarmPool::new(POOL_SIZE, "128MB")?;
    let prewarm_all_ready_ms = started.elapsed().as_millis();
    let initial_pids = pool.wait_until_idle(POOL_SIZE, Duration::from_secs(10))?;
    if initial_pids.contains(&cold_pid) {
        return Err("cold worker unexpectedly survived into the prewarm pool".into());
    }

    let started = Instant::now();
    let pipeline_a = pool.checkout("pipeline-a/run-1", Duration::from_secs(2))?;
    let warm_checkout_us = started.elapsed().as_micros();
    if pipeline_a.pipeline_run_id() != "pipeline-a/run-1" {
        return Err("pipeline run identity was not retained by the lease".into());
    }
    let client_a = pipeline_a.client()?;
    if client_a.scalar_i64("SELECT 42::BIGINT")? != 42 {
        return Err("prewarmed pipeline worker failed its first query".into());
    }
    client_a.execute_remote("CREATE TABLE pipeline_private(value INTEGER)")?;

    let pipeline_b = pool.checkout("pipeline-b/run-1", Duration::from_secs(2))?;
    if pipeline_a.pid() == pipeline_b.pid() {
        return Err("two pipelines were assigned the same worker".into());
    }
    let client_b = pipeline_b.client()?;
    let leaked_tables = client_b.scalar_i64(
        "SELECT count(*)::BIGINT FROM information_schema.tables \
         WHERE table_name = 'pipeline_private'",
    )?;
    if leaked_tables != 0 {
        return Err("pipeline state leaked into another worker".into());
    }
    drop(client_a);
    drop(client_b);
    drop(pipeline_a);
    drop(pipeline_b);
    pool.wait_until_idle(POOL_SIZE, Duration::from_secs(10))?;

    let mut active = Vec::new();
    for index in 0..POOL_SIZE {
        active.push(pool.checkout(
            format!("pipeline-{index}/run-queue-test"),
            Duration::from_secs(2),
        )?);
    }
    let active_pids: Vec<u32> = active.iter().map(WorkerLease::pid).collect();
    let (sender, receiver) = mpsc::channel();
    let handle = pool.handle();
    thread::spawn(move || {
        let started = Instant::now();
        let result = handle.checkout("pipeline-queued/run-1", Duration::from_secs(10));
        let _ = sender.send((started.elapsed(), result));
    });
    match receiver.recv_timeout(Duration::from_millis(150)) {
        Err(mpsc::RecvTimeoutError::Timeout) => {}
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            return Err("queued checkout worker disconnected".into());
        }
        Ok(_) => return Err("fourth pipeline bypassed the bounded worker queue".into()),
    }

    let replacement_started = Instant::now();
    drop(active.pop());
    let (queued_wait, queued_result) = receiver.recv_timeout(Duration::from_secs(10))?;
    let queued_pipeline = queued_result?;
    let replacement_ready_ms = replacement_started.elapsed().as_millis();
    if queued_pipeline.pipeline_run_id() != "pipeline-queued/run-1" {
        return Err("queued request received the wrong pipeline lease".into());
    }
    if active_pids.contains(&queued_pipeline.pid()) {
        return Err("queued pipeline received a previously used worker".into());
    }
    if queued_pipeline.client()?.scalar_i64("SELECT 99::BIGINT")? != 99 {
        return Err("replacement worker was published before it was query-ready".into());
    }
    drop(queued_pipeline);
    drop(active);

    let final_pids = pool.wait_until_idle(POOL_SIZE, Duration::from_secs(10))?;
    if final_pids.iter().any(|pid| active_pids.contains(pid)) {
        return Err("a consumed pipeline worker was recycled instead of restarted".into());
    }
    let target_size = pool.handle.inner.target_size;
    drop(pool);

    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "status": "ok",
            "duckdb_version": DUCKDB_VERSION,
            "pool_size": target_size,
            "worker_memory_limit_mb": WORKER_MEMORY_MB,
            "aggregate_active_memory_limit_mb": target_size as u64 * WORKER_MEMORY_MB,
            "cold_start_ms": cold_start_ms,
            "prewarm_all_ready_ms": prewarm_all_ready_ms,
            "warm_checkout_us": warm_checkout_us,
            "queued_pipeline_wait_ms": queued_wait.as_millis(),
            "replacement_ready_ms": replacement_ready_ms,
            "exclusive_pipeline_isolation": true,
            "single_use_workers": true,
            "initial_worker_pids": initial_pids,
            "active_worker_pids": active_pids,
            "replacement_worker_pids": final_pids
        }))?
    );
    Ok(())
}

fn start_warm_worker(memory_limit: &str) -> SpikeResult<WarmWorker> {
    let run_dir = unique_run_directory();
    fs::create_dir_all(&run_dir)?;
    let run_directory = RunDirectory(run_dir.clone());
    let ready_path = run_dir.join("ready.json");
    let spill_path = run_dir.join("spill");
    let port = reserve_loopback_port()?;
    let mut child = SidecarChild(spawn_sidecar_with_limit(
        &ready_path,
        &spill_path,
        port,
        ":memory:",
        memory_limit,
    )?);
    let ready = wait_for_ready(&ready_path, &mut child, Duration::from_secs(90))?;
    Ok(WarmWorker {
        _child: child,
        _run_directory: run_directory,
        ready,
    })
}

fn spawn_replacement(pool: Arc<PoolInner>) {
    thread::spawn(move || loop {
        if pool.shutdown.load(Ordering::Acquire) {
            if let Ok(mut state) = pool.state.lock() {
                state.starting = state.starting.saturating_sub(1);
            }
            pool.available.notify_all();
            return;
        }
        match start_warm_worker(&pool.memory_limit) {
            Ok(worker) => {
                if let Ok(mut state) = pool.state.lock() {
                    state.starting = state.starting.saturating_sub(1);
                    if !pool.shutdown.load(Ordering::Acquire) {
                        // Publication happens only after DuckDB, Quack and the
                        // readiness handshake have all succeeded.
                        state.idle.push_back(worker);
                    }
                }
                pool.available.notify_all();
                return;
            }
            Err(_) => thread::sleep(Duration::from_millis(100)),
        }
    });
}

fn check<const N: usize>(
    name: &str,
    started: Instant,
    details: [(&str, serde_json::Value); N],
) -> CheckResult {
    CheckResult {
        name: name.into(),
        duration_ms: started.elapsed().as_millis(),
        details: details
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect(),
    }
}

fn spawn_sidecar(
    ready_path: &Path,
    spill_path: &Path,
    port: u16,
    database: &str,
) -> SpikeResult<Child> {
    spawn_sidecar_with_limit(ready_path, spill_path, port, database, "128MB")
}

fn spawn_sidecar_with_limit(
    ready_path: &Path,
    spill_path: &Path,
    port: u16,
    database: &str,
    memory_limit: &str,
) -> SpikeResult<Child> {
    Ok(Command::new(env::current_exe()?)
        .arg("server")
        .arg("--ready")
        .arg(ready_path)
        .arg("--port")
        .arg(port.to_string())
        .arg("--database")
        .arg(database)
        .arg("--memory-limit")
        .arg(memory_limit)
        .arg("--temp-directory")
        .arg(spill_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()?)
}

fn wait_for_ready(path: &Path, child: &mut Child, timeout: Duration) -> SpikeResult<ReadyInfo> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if path.exists() {
            return read_ready(path);
        }
        if let Some(status) = child.try_wait()? {
            return Err(format!("sidecar exited before readiness: {status}").into());
        }
        thread::sleep(Duration::from_millis(25));
    }
    Err("timed out waiting for sidecar readiness".into())
}

fn reserve_loopback_port() -> SpikeResult<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    Ok(listener.local_addr()?.port())
}

fn directory_size(path: &Path) -> SpikeResult<u64> {
    if !path.exists() {
        return Ok(0);
    }
    let mut bytes = 0_u64;
    let mut pending = vec![path.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if metadata.is_dir() {
                pending.push(entry.path());
            } else {
                bytes = bytes.saturating_add(metadata.len());
            }
        }
    }
    Ok(bytes)
}

fn unique_run_directory() -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    env::temp_dir().join(format!(
        "duckle-quack-spike-{}-{timestamp}",
        std::process::id()
    ))
}

fn optional_value(arguments: &[String], name: &str) -> Option<String> {
    arguments
        .windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
}

fn required_path(arguments: &[String], name: &str) -> SpikeResult<PathBuf> {
    optional_value(arguments, name)
        .map(PathBuf::from)
        .ok_or_else(|| format!("missing {name}").into())
}

fn usage() {
    eprintln!(
        "usage:\n  duckle-quack-sidecar-spike server --ready PATH [--port N] [--database :memory:|PATH] [--memory-limit 64MB] [--temp-directory PATH]\n  duckle-quack-sidecar-spike query --ready PATH --sql SQL\n  duckle-quack-sidecar-spike smoke\n  duckle-quack-sidecar-spike benchmark\n  duckle-quack-sidecar-spike pool-smoke"
    );
}

fn main() {
    let arguments: Vec<String> = env::args().skip(1).collect();
    let result = match arguments.first().map(String::as_str) {
        Some("server") => serve(&arguments[1..]),
        Some("query") => query(&arguments[1..]),
        Some("smoke") => smoke(),
        Some("benchmark") => benchmark(),
        Some("pool-smoke") => pool_smoke(),
        _ => {
            usage();
            Err("missing or unknown command".into())
        }
    };
    if let Err(error) = result {
        eprintln!("quack-sidecar-spike: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sql_literal_escapes_quotes() {
        assert_eq!(sql_literal("a'b"), "'a''b'");
    }

    #[test]
    fn version_mismatch_fails_before_connection() {
        let ready = ReadyInfo {
            protocol_version: SPIKE_PROTOCOL_VERSION + 1,
            duckdb_version: DUCKDB_VERSION.into(),
            pid: 1,
            uri: "quack:127.0.0.1:9494".into(),
            url: "http://127.0.0.1:9494".into(),
            token: "synthetic-token".into(),
            storage: ":memory:".into(),
            temp_directory: "spill".into(),
        };
        assert!(validate_ready(&ready)
            .unwrap_err()
            .to_string()
            .contains("protocol mismatch"));
    }
}
