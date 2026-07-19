use duckdb::types::{Value, ValueRef};
use duckdb::{params, Connection};
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
const QUACK_TOKEN_ENV: &str = "DUCKLE_QUACK_SPIKE_BOOTSTRAP_TOKEN";
const EXECUTION_SECURITY_PROFILE: &str = "execution_trusted_full_sql_v1";
const DEFAULT_QUACK_PARALLELISM: usize = 8;

type SpikeResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReadyInfo {
    protocol_version: u32,
    duckdb_version: String,
    pid: u32,
    uri: String,
    url: String,
    security_profile: String,
    storage: String,
    temp_directory: String,
}

#[derive(Debug, Clone)]
struct QuackAccess {
    ready: ReadyInfo,
    token: String,
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

#[derive(Debug, Serialize)]
struct ParallelismProbeResult {
    mode: String,
    connections: usize,
    sequential_ms: u128,
    concurrent_ms: u128,
    speedup: f64,
    quack_connection_ids: i64,
    parallel_execution: bool,
}

struct QuackClient {
    connection: Connection,
    access: QuackAccess,
}

struct RunDirectory(PathBuf);

impl Drop for RunDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct SidecarChild(Child);

struct WarmWorker {
    // The client must be dropped before the process guard. Sticky probes showed
    // that reversing this order makes close wait against an already-dead server.
    client: QuackClient,
    // These guards intentionally have no direct callers: dropping them is the
    // LocalProcessProvider prototype's terminate-and-cleanup operation.
    _child: SidecarChild,
    _run_directory: RunDirectory,
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
        self.client.access.ready.pid
    }

    fn client(&self) -> &QuackClient {
        &self.client
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

    fn client(&self) -> &QuackClient {
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
    fn connect(access: QuackAccess) -> SpikeResult<Self> {
        Self::connect_with_http_timeout(access, 30)
    }

    fn connect_with_http_timeout(access: QuackAccess, timeout_seconds: u64) -> SpikeResult<Self> {
        Self::connect_profiled(access, timeout_seconds).map(|(client, _)| client)
    }

    fn connect_stateless(access: QuackAccess) -> SpikeResult<Self> {
        Self::connect_profiled_mode(access, 30, false).map(|(client, _)| client)
    }

    fn connect_profiled(
        access: QuackAccess,
        timeout_seconds: u64,
    ) -> SpikeResult<(Self, serde_json::Value)> {
        Self::connect_profiled_mode(access, timeout_seconds, true)
    }

    fn connect_profiled_mode(
        access: QuackAccess,
        timeout_seconds: u64,
        create_initial_attach: bool,
    ) -> SpikeResult<(Self, serde_json::Value)> {
        validate_ready(&access.ready)?;
        let total_started = Instant::now();
        let started = Instant::now();
        let connection = Connection::open_in_memory()?;
        let open_database_us = started.elapsed().as_micros() as u64;
        let started = Instant::now();
        load_quack(&connection)?;
        let load_quack_us = started.elapsed().as_micros() as u64;
        let started = Instant::now();
        connection.execute_batch(&format!(
            "SET httpfs_connection_caching = true; \
             SET http_timeout = {timeout_seconds}; \
             SET http_retries = 0;"
        ))?;
        let configure_http_us = started.elapsed().as_micros() as u64;
        // Parameters keep the credential out of the SQL text captured by
        // DuckDB/Quack query logging. Each client database is private, so the
        // fixed temporary secret name cannot collide with another run.
        let started = Instant::now();
        connection.execute(
            "CREATE TEMPORARY SECRET duckle_quack_credentials (\
             TYPE quack, SCOPE ?, TOKEN ?)",
            params![access.ready.uri, access.token],
        )?;
        let create_secret_us = started.elapsed().as_micros() as u64;
        let initial_attach_us = if create_initial_attach {
            let started = Instant::now();
            connection.execute_batch(&format!(
                "ATTACH {} AS run_remote (TYPE quack);",
                sql_literal(&access.ready.uri)
            ))?;
            started.elapsed().as_micros() as u64
        } else {
            0
        };
        let total_us = total_started.elapsed().as_micros() as u64;
        Ok((
            Self { connection, access },
            serde_json::json!({
                "open_database_us": open_database_us,
                "load_quack_us": load_quack_us,
                "configure_http_us": configure_http_us,
                "create_secret_us": create_secret_us,
                "initial_attach_us": initial_attach_us,
                "total_us": total_us,
            }),
        ))
    }

    fn invocation(&self, sql: &str) -> String {
        Self::invocation_for("run_remote", sql)
    }

    fn invocation_for(alias: &str, sql: &str) -> String {
        assert!(
            alias
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_'),
            "validated Quack attachment alias"
        );
        format!(
            "FROM quack_query_by_name({}, {})",
            sql_literal(alias),
            sql_literal(sql)
        )
    }

    fn stateless_invocation_for(uri: &str, sql: &str) -> String {
        format!(
            "FROM quack_query({}, {})",
            sql_literal(uri),
            sql_literal(sql)
        )
    }

    fn execute_remote(&self, sql: &str) -> SpikeResult<u64> {
        let query = format!("SELECT count(*) FROM ({})", self.invocation(sql));
        let count: i64 = self.connection.query_row(&query, [], |row| row.get(0))?;
        Ok(count.max(0) as u64)
    }

    fn execute_stateless(&self, sql: &str) -> SpikeResult<u64> {
        let invocation = Self::stateless_invocation_for(&self.access.ready.uri, sql);
        let query = format!("SELECT count(*) FROM ({invocation})");
        let connection = self.connection.try_clone()?;
        let count: i64 = connection.query_row(&query, [], |row| row.get(0))?;
        Ok(count.max(0) as u64)
    }

    fn scalar_i64(&self, sql: &str) -> SpikeResult<i64> {
        let query = self.invocation(sql);
        Ok(self.connection.query_row(&query, [], |row| row.get(0))?)
    }

    fn scalar_i64_stateless(&self, sql: &str) -> SpikeResult<i64> {
        let query = Self::stateless_invocation_for(&self.access.ready.uri, sql);
        let connection = self.connection.try_clone()?;
        Ok(connection.query_row(&query, [], |row| row.get(0))?)
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
        self.attach_remote_on(&self.connection, alias)
    }

    fn attach_remote_on(&self, connection: &Connection, alias: &str) -> SpikeResult<()> {
        if !alias.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err("attachment alias contains unsupported characters".into());
        }
        connection.execute_batch(&format!(
            "ATTACH {} AS {} (TYPE quack);",
            sql_literal(&self.access.ready.uri),
            alias
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
    if !ready.url.starts_with("http://127.0.0.1:") {
        return Err(format!("sidecar URL is not loopback HTTP: {}", ready.url).into());
    }
    if ready.security_profile != EXECUTION_SECURITY_PROFILE {
        return Err(format!(
            "unexpected Quack security profile: {}",
            ready.security_profile
        )
        .into());
    }
    Ok(())
}

fn generate_quack_token() -> SpikeResult<String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|error| format!("operating-system random generation failed: {error}"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn take_bootstrap_token() -> SpikeResult<String> {
    let token = env::var(QUACK_TOKEN_ENV)
        .map_err(|_| format!("missing protected bootstrap token in {QUACK_TOKEN_ENV}"))?;
    env::remove_var(QUACK_TOKEN_ENV);
    if token.len() < 32 {
        return Err("Quack bootstrap token is unexpectedly short".into());
    }
    Ok(token)
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
    let token = take_bootstrap_token()?;
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
    // This execution-plane profile intentionally grants the authenticated
    // supervisor the complete SQL surface needed to run a pipeline. It is
    // explicit because Quack's default authorization is permissive and must
    // never be mistaken for a future browser/publication policy.
    connection.execute_batch(
        "SET GLOBAL quack_authentication_function = 'quack_check_token'; \
         SET GLOBAL quack_authorization_function = 'quack_nop_authorization';",
    )?;

    let requested_uri = format!("quack:127.0.0.1:{port}");
    let (uri, url, returned_token): (String, String, String) = connection.query_row(
        "CALL quack_serve(?, token => ?)",
        params![requested_uri, token],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    if returned_token != token {
        return Err("Quack returned a different authentication token".into());
    }
    let version: String = connection.query_row("SELECT version()", [], |row| row.get(0))?;
    let ready = ReadyInfo {
        protocol_version: SPIKE_PROTOCOL_VERSION,
        duckdb_version: version.trim_start_matches('v').to_string(),
        pid: std::process::id(),
        uri,
        url,
        security_profile: EXECUTION_SECURITY_PROFILE.into(),
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
    let token = take_bootstrap_token()?;
    let sql = optional_value(arguments, "--sql").ok_or("missing --sql")?;
    let client = QuackClient::connect(QuackAccess { ready, token })?;
    println!(
        "{}",
        serde_json::to_string_pretty(&client.query_json(&sql)?)?
    );
    Ok(())
}

fn probe_master_attach_try_clone(
    client: &QuackClient,
    expected_sum: i64,
) -> SpikeResult<CheckResult> {
    // Characterize the production client shape proposed for Duckle: one
    // run-scoped master connection performs the Quack ATTACH, then stage
    // workers use try_clone() without repeating secret creation or ATTACH.
    // Besides catalog visibility, verify that the clones address the same
    // sticky Quack server session, can execute concurrently, and do not tear
    // down the master attachment when released.
    let started = Instant::now();
    let clone_a = client.connection.try_clone()?;
    let clone_b = client.connection.try_clone()?;
    for (name, connection) in [("clone_a", &clone_a), ("clone_b", &clone_b)] {
        let attached: i64 = connection.query_row(
            "SELECT count(*)::BIGINT FROM duckdb_databases() \
             WHERE database_name = 'run_remote'",
            [],
            |row| row.get(0),
        )?;
        if attached != 1 {
            return Err(
                format!("{name} did not inherit the master's run_remote Quack ATTACH").into(),
            );
        }
        let sticky: i64 = connection.query_row(
            &client.invocation("SELECT value::BIGINT FROM sticky_state"),
            [],
            |row| row.get(0),
        )?;
        if sticky != 42 {
            return Err(format!(
                "{name} inherited run_remote but not its sticky server-side TEMP state"
            )
            .into());
        }
    }

    clone_a.query_row(
        &client
            .invocation("CREATE OR REPLACE TABLE clone_shared_state AS SELECT 84::BIGINT AS value"),
        [],
        |_| Ok(()),
    )?;
    let shared_value: i64 = clone_b.query_row(
        &client.invocation("SELECT value::BIGINT FROM clone_shared_state"),
        [],
        |row| row.get(0),
    )?;
    if shared_value != 84 {
        return Err("a table written through clone_a was not visible through clone_b".into());
    }

    let sleep_query = client.invocation("SELECT sleep_ms(400)");
    let sequential_started = Instant::now();
    for _ in 0..2 {
        clone_a.query_row(&sleep_query, [], |_| Ok(()))?;
    }
    let sequential_ms = sequential_started.elapsed().as_millis();

    let barrier = Arc::new(Barrier::new(3));
    let mut clone_readers = Vec::new();
    for connection in [clone_a, clone_b] {
        let barrier = Arc::clone(&barrier);
        let sleep_query = sleep_query.clone();
        let aggregate_query = client.invocation("SELECT sum(i)::BIGINT FROM facts");
        clone_readers.push(thread::spawn(move || -> SpikeResult<i64> {
            barrier.wait();
            connection.query_row(&sleep_query, [], |_| Ok(()))?;
            Ok(connection.query_row(&aggregate_query, [], |row| row.get(0))?)
        }));
    }
    let parallel_started = Instant::now();
    barrier.wait();
    for reader in clone_readers {
        if reader
            .join()
            .map_err(|_| "cloned Quack reader panicked")??
            != expected_sum
        {
            return Err("a cloned Quack connection returned the wrong aggregate".into());
        }
    }
    let parallel_ms = parallel_started.elapsed().as_millis();
    let clone_parallel_execution = parallel_ms.saturating_mul(4) < sequential_ms.saturating_mul(3);

    // Both stage clones have now been dropped. The master must still own a
    // usable attachment, and a later stage clone must inherit it as well.
    if client.scalar_i64("SELECT value::BIGINT FROM sticky_state")? != 42 {
        return Err("dropping stage clones invalidated the master Quack attachment".into());
    }
    let later_clone = client.connection.try_clone()?;
    let later_value: i64 = later_clone.query_row(
        &client.invocation("SELECT value::BIGINT FROM clone_shared_state"),
        [],
        |row| row.get(0),
    )?;
    if later_value != 84 {
        return Err("a later stage clone did not inherit the live Quack attachment".into());
    }
    Ok(check(
        "master_attach_try_clone",
        started,
        [
            ("attach_per_run", true.into()),
            ("sticky_temp_visible", true.into()),
            ("concurrent_clone_calls", 2.into()),
            ("clone_parallel_execution", clone_parallel_execution.into()),
            ("sequential_probe_ms", (sequential_ms as u64).into()),
            ("parallel_probe_ms", (parallel_ms as u64).into()),
            ("later_clone_after_release", true.into()),
        ],
    ))
}

fn probe_independent_clients_parallel(access: QuackAccess) -> SpikeResult<CheckResult> {
    // Comparison control: fully independent client databases each create one
    // long-lived ATTACH. The matrix below verifies that distinct aliases on one
    // master client database provide the same server-side parallelism without
    // requiring this heavier topology in the production pool.
    let client_a = QuackClient::connect(access.clone())?;
    let client_b = QuackClient::connect(access)?;
    let sleep_query = client_a.invocation("SELECT sleep_ms(400)");
    let sequential_started = Instant::now();
    for _ in 0..2 {
        client_a
            .connection
            .query_row(&sleep_query, [], |_| Ok(()))?;
    }
    let sequential_ms = sequential_started.elapsed().as_millis();

    let barrier = Arc::new(Barrier::new(3));
    let mut workers = Vec::new();
    for client in [client_a, client_b] {
        let barrier = Arc::clone(&barrier);
        let query = client.invocation("SELECT sleep_ms(400)");
        workers.push(thread::spawn(move || -> SpikeResult<()> {
            barrier.wait();
            client.connection.query_row(&query, [], |_| Ok(()))?;
            Ok(())
        }));
    }
    let parallel_started = Instant::now();
    barrier.wait();
    for worker in workers {
        worker
            .join()
            .map_err(|_| "independent Quack client worker panicked")??;
    }
    let parallel_ms = parallel_started.elapsed().as_millis();
    let parallel_execution = parallel_ms.saturating_mul(4) < sequential_ms.saturating_mul(3);
    if !parallel_execution {
        return Err(format!(
            "independent Quack attachments did not demonstrate parallel execution: sequential={sequential_ms}ms, parallel={parallel_ms}ms"
        )
        .into());
    }
    Ok(check(
        "independent_client_control_parallel",
        parallel_started,
        [
            ("independent_client_databases", true.into()),
            ("parallel_execution", parallel_execution.into()),
            ("sequential_probe_ms", (sequential_ms as u64).into()),
            ("parallel_probe_ms", (parallel_ms as u64).into()),
        ],
    ))
}

fn measure_parallelism_group(
    inspector: &QuackClient,
    mode: &str,
    connections: Vec<(Connection, String)>,
    invocation: fn(&str, &str) -> String,
) -> SpikeResult<ParallelismProbeResult> {
    let connection_count = connections.len();
    if connection_count == 0 {
        return Err("parallelism probe needs at least one connection".into());
    }
    let label = format!("duckle_parallel_probe_{mode}_{connection_count}");
    let sequential_queries: Vec<String> = connections
        .iter()
        .enumerate()
        .map(|(index, (_, target))| {
            invocation(
                target,
                &format!("SELECT sleep_ms(250) /* {label}_sequential_{index} */"),
            )
        })
        .collect();
    let concurrent_queries: Vec<String> = connections
        .iter()
        .enumerate()
        .map(|(index, (_, target))| {
            invocation(
                target,
                &format!("SELECT sleep_ms(250) /* {label}_concurrent_{index} */"),
            )
        })
        .collect();

    let sequential_started = Instant::now();
    for ((connection, _), query) in connections.iter().zip(&sequential_queries) {
        connection.query_row(query, [], |_| Ok(()))?;
    }
    let sequential_ms = sequential_started.elapsed().as_millis();

    let barrier = Arc::new(Barrier::new(connection_count + 1));
    let mut workers = Vec::with_capacity(connection_count);
    for ((connection, _), query) in connections.into_iter().zip(concurrent_queries) {
        let barrier = Arc::clone(&barrier);
        workers.push(thread::spawn(move || -> SpikeResult<()> {
            barrier.wait();
            connection.query_row(&query, [], |_| Ok(()))?;
            Ok(())
        }));
    }
    let concurrent_started = Instant::now();
    barrier.wait();
    for worker in workers {
        worker
            .join()
            .map_err(|_| "parallelism matrix worker panicked")??;
    }
    let concurrent_ms = concurrent_started.elapsed().as_millis();
    let quack_connection_ids = inspector.scalar_i64(&format!(
        "SELECT count(DISTINCT quack_connection_id)::BIGINT \
         FROM duckdb_logs_parsed('Quack') \
         WHERE query LIKE {}",
        sql_literal(&format!("%{label}_concurrent%"))
    ))?;
    let speedup = sequential_ms as f64 / concurrent_ms.max(1) as f64;
    let parallel_execution = connection_count == 1 || speedup >= 1.5;
    Ok(ParallelismProbeResult {
        mode: mode.into(),
        connections: connection_count,
        sequential_ms,
        concurrent_ms,
        speedup,
        quack_connection_ids,
        parallel_execution,
    })
}

fn run_parallelism_matrix(
    master: &QuackClient,
    access: QuackAccess,
) -> SpikeResult<Vec<ParallelismProbeResult>> {
    master.execute_remote("CALL enable_logging('Quack')")?;
    let mut results = Vec::new();
    for connection_count in [1_usize, 2, 4, 8] {
        let shared_connections = (0..connection_count)
            .map(|_| {
                master
                    .connection
                    .try_clone()
                    .map(|connection| (connection, "run_remote".to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let shared = measure_parallelism_group(
            master,
            "shared_master_attach",
            shared_connections,
            QuackClient::invocation_for,
        )?;
        if shared.quack_connection_ids != 1 {
            return Err(format!(
                "shared master ATTACH used {} Quack connection ids instead of one",
                shared.quack_connection_ids
            )
            .into());
        }
        results.push(shared);

        let mut aliases = Vec::with_capacity(connection_count);
        for index in 0..connection_count {
            let alias = format!("pool_{connection_count}_{index}");
            master.attach_remote(&alias)?;
            aliases.push(alias);
        }
        let distinct_connections = aliases
            .into_iter()
            .map(|alias| {
                master
                    .connection
                    .try_clone()
                    .map(|connection| (connection, alias))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let distinct = measure_parallelism_group(
            master,
            "distinct_attach_same_master",
            distinct_connections,
            QuackClient::invocation_for,
        )?;
        if distinct.quack_connection_ids != connection_count as i64 {
            return Err(format!(
                "{connection_count} distinct ATTACH aliases used {} Quack connection ids",
                distinct.quack_connection_ids
            )
            .into());
        }
        results.push(distinct);

        let stateless_connections = (0..connection_count)
            .map(|_| {
                master
                    .connection
                    .try_clone()
                    .map(|connection| (connection, master.access.ready.uri.clone()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let stateless = measure_parallelism_group(
            master,
            "stateless_quack_query_same_master",
            stateless_connections,
            QuackClient::stateless_invocation_for,
        )?;
        if stateless.quack_connection_ids != connection_count as i64 {
            return Err(format!(
                "{connection_count} concurrent stateless queries used {} Quack connection ids",
                stateless.quack_connection_ids
            )
            .into());
        }
        results.push(stateless);

        let independent_connections = (0..connection_count)
            .map(|_| {
                QuackClient::connect(access.clone())
                    .map(|client| (client.connection, "run_remote".to_string()))
            })
            .collect::<SpikeResult<Vec<_>>>()?;
        let independent = measure_parallelism_group(
            master,
            "independent_client_database",
            independent_connections,
            QuackClient::invocation_for,
        )?;
        if independent.quack_connection_ids != connection_count as i64 {
            return Err(format!(
                "{connection_count} independent clients used {} Quack connection ids",
                independent.quack_connection_ids
            )
            .into());
        }
        results.push(independent);
    }
    Ok(results)
}

fn latency_summary_microseconds(mut samples: Vec<u64>) -> serde_json::Value {
    samples.sort_unstable();
    let percentile_index = |percent: usize| {
        ((samples.len() * percent).div_ceil(100))
            .saturating_sub(1)
            .min(samples.len().saturating_sub(1))
    };
    serde_json::json!({
        "samples": samples.len(),
        "min_us": samples[0],
        "median_us": samples[samples.len() / 2],
        "p95_us": samples[percentile_index(95)],
        "max_us": samples[samples.len() - 1],
    })
}

fn probe_attach_bootstrap_latency(client: &QuackClient) -> SpikeResult<serde_json::Value> {
    let mut attach_statement = Vec::with_capacity(20);
    let mut first_remote_query = Vec::with_capacity(20);
    let mut warm_remote_query = Vec::with_capacity(20);
    for index in 0..20 {
        let alias = format!("attach_latency_{index}");
        let started = Instant::now();
        client.attach_remote(&alias)?;
        attach_statement.push(started.elapsed().as_micros() as u64);

        let query = QuackClient::invocation_for(&alias, "SELECT 1");
        let started = Instant::now();
        client.connection.query_row(&query, [], |_| Ok(()))?;
        first_remote_query.push(started.elapsed().as_micros() as u64);

        let started = Instant::now();
        client.connection.query_row(&query, [], |_| Ok(()))?;
        warm_remote_query.push(started.elapsed().as_micros() as u64);

        client
            .connection
            .execute_batch(&format!("DETACH {alias};"))?;
    }
    Ok(serde_json::json!({
        "attach_statement": latency_summary_microseconds(attach_statement),
        "first_remote_query": latency_summary_microseconds(first_remote_query),
        "warm_remote_query": latency_summary_microseconds(warm_remote_query),
    }))
}

fn probe_slave_connection_bootstrap(client: &QuackClient) -> SpikeResult<serde_json::Value> {
    const SAMPLE_COUNT: usize = 20;
    const QUERY_MARKER: &str = "duckle_slave_connection_bootstrap";

    client.execute_remote("CALL enable_logging('Quack')")?;
    let mut clone_connection = Vec::with_capacity(SAMPLE_COUNT);
    let mut attach_on_clone = Vec::with_capacity(SAMPLE_COUNT);
    let mut first_remote_query = Vec::with_capacity(SAMPLE_COUNT);
    let mut warm_remote_query = Vec::with_capacity(SAMPLE_COUNT);
    let mut total_to_first_query = Vec::with_capacity(SAMPLE_COUNT);

    for index in 0..SAMPLE_COUNT {
        let total_started = Instant::now();
        let started = Instant::now();
        let connection = client.connection.try_clone()?;
        clone_connection.push(started.elapsed().as_micros() as u64);

        let alias = format!("slave_bootstrap_{index}");
        let started = Instant::now();
        client.attach_remote_on(&connection, &alias)?;
        attach_on_clone.push(started.elapsed().as_micros() as u64);

        let remote_sql = format!("SELECT 1 /* {QUERY_MARKER}_{index} */");
        let query = QuackClient::invocation_for(&alias, &remote_sql);
        let started = Instant::now();
        connection.query_row(&query, [], |_| Ok(()))?;
        first_remote_query.push(started.elapsed().as_micros() as u64);
        total_to_first_query.push(total_started.elapsed().as_micros() as u64);

        let started = Instant::now();
        connection.query_row(&query, [], |_| Ok(()))?;
        warm_remote_query.push(started.elapsed().as_micros() as u64);

        connection.execute_batch(&format!("DETACH {alias};"))?;
    }

    let quack_connection_ids = client.scalar_i64(&format!(
        "SELECT count(DISTINCT quack_connection_id)::BIGINT \
         FROM duckdb_logs_parsed('Quack') \
         WHERE query LIKE {}",
        sql_literal(&format!("%{QUERY_MARKER}%"))
    ))?;
    if quack_connection_ids != SAMPLE_COUNT as i64 {
        return Err(format!(
            "{SAMPLE_COUNT} cloned ATTACH operations used {quack_connection_ids} Quack connection ids"
        )
        .into());
    }

    Ok(serde_json::json!({
        "samples": SAMPLE_COUNT,
        "clone_connection": latency_summary_microseconds(clone_connection),
        "attach_on_clone": latency_summary_microseconds(attach_on_clone),
        "first_remote_query": latency_summary_microseconds(first_remote_query),
        "warm_remote_query": latency_summary_microseconds(warm_remote_query),
        "total_to_first_query": latency_summary_microseconds(total_to_first_query),
        "distinct_quack_connection_ids": quack_connection_ids,
        "master_temporary_secret_reused_by_clone": true,
    }))
}

fn probe_stateless_server_attach_persistence(
    client: &QuackClient,
    source_path: &Path,
) -> SpikeResult<serde_json::Value> {
    {
        let source = Connection::open(source_path)?;
        source.execute_batch(
            "CREATE TABLE source_values(value BIGINT); \
             INSERT INTO source_values VALUES (42);",
        )?;
    }

    let alias = "server_attached_source";
    client.execute_stateless(&format!(
        "ATTACH {} AS {alias} (READ_ONLY);",
        sql_literal(&source_path.display().to_string())
    ))?;
    let value = client.scalar_i64_stateless(&format!(
        "SELECT value::BIGINT FROM {alias}.main.source_values"
    ))?;
    if value != 42 {
        return Err(format!(
            "stateless server-side ATTACH was not visible to a later request: got {value}"
        )
        .into());
    }
    client.execute_stateless(&format!("DETACH {alias};"))?;

    Ok(serde_json::json!({
        "transport": "stateless_quack_query",
        "server_side_attach_persists_across_requests": true,
        "value_from_later_request": value,
    }))
}

fn clone_attach_smoke() -> SpikeResult<()> {
    let run_dir = unique_run_directory();
    fs::create_dir_all(&run_dir)?;
    let _run_directory = RunDirectory(run_dir.clone());
    let ready_path = run_dir.join("ready.json");
    let spill_path = run_dir.join("spill");
    let port = reserve_loopback_port()?;
    let token = generate_quack_token()?;
    let mut child = SidecarChild(spawn_sidecar(
        &ready_path,
        &spill_path,
        port,
        ":memory:",
        &token,
    )?);
    let ready = wait_for_ready(&ready_path, &mut child, Duration::from_secs(90))?;
    let (client, master_client_bootstrap) =
        QuackClient::connect_profiled(QuackAccess { ready, token }, 30)?;
    let server_attach_persistence = probe_stateless_server_attach_persistence(
        &client,
        &run_dir.join("server-attach-source.duckdb"),
    )?;
    let slave_connection_bootstrap = probe_slave_connection_bootstrap(&client)?;
    let attach_bootstrap_latency = probe_attach_bootstrap_latency(&client)?;
    client.execute_remote("CREATE TEMP TABLE sticky_state AS SELECT 42 AS value")?;
    client.execute_remote("CREATE TABLE facts AS SELECT i FROM range(100000) t(i)")?;
    let batch_output_rows = client.execute_remote(
        "CREATE TABLE batch_stage_a AS SELECT i FROM range(5) t(i); \
         CREATE TABLE batch_stage_b AS SELECT sum(i)::BIGINT AS value FROM batch_stage_a; \
         SELECT value FROM batch_stage_b",
    )?;
    let batch_value = client.scalar_i64("SELECT value::BIGINT FROM batch_stage_b")?;
    if batch_output_rows != 1 || batch_value != 10 {
        return Err(format!(
            "remote multi-statement batch returned {batch_output_rows} rows and value {batch_value}"
        )
        .into());
    }
    let stateless_batch_output_rows = client.execute_stateless(
        "CREATE TABLE stateless_batch_stage_a AS SELECT i FROM range(5) t(i); \
         CREATE TABLE stateless_batch_stage_b AS \
             SELECT sum(i)::BIGINT AS value FROM stateless_batch_stage_a; \
         SELECT value FROM stateless_batch_stage_b",
    )?;
    let stateless_batch_value =
        client.scalar_i64_stateless("SELECT value::BIGINT FROM stateless_batch_stage_b")?;
    if stateless_batch_output_rows != 1 || stateless_batch_value != 10 {
        return Err(format!(
            "stateless remote batch returned {stateless_batch_output_rows} rows and value {stateless_batch_value}"
        )
        .into());
    }
    let expected_sum = 4_999_950_000_i64;
    let clone_result = probe_master_attach_try_clone(&client, expected_sum)?;
    let independent_result = probe_independent_clients_parallel(client.access.clone())?;
    let matrix = run_parallelism_matrix(&client, client.access.clone())?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "master_client_bootstrap": master_client_bootstrap,
            "server_attach_persistence": server_attach_persistence,
            "slave_connection_bootstrap": slave_connection_bootstrap,
            "attach_bootstrap_latency": attach_bootstrap_latency,
            "multi_statement_batch": {
                "attached": {
                    "output_rows": batch_output_rows,
                    "value": batch_value,
                },
                "stateless": {
                    "output_rows": stateless_batch_output_rows,
                    "value": stateless_batch_value,
                },
            },
            "sticky_semantics": [clone_result, independent_result],
            "matrix": matrix,
        }))?
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
    let token = generate_quack_token()?;

    let started = Instant::now();
    let mut child = SidecarChild(spawn_sidecar(
        &ready_path,
        &spill_path,
        port,
        ":memory:",
        &token,
    )?);
    let ready = match wait_for_ready(&ready_path, &mut child, Duration::from_secs(90)) {
        Ok(ready) => ready,
        Err(error) => {
            return Err(error);
        }
    };
    let ready_payload = fs::read_to_string(&ready_path)?;
    if ready_payload.contains(&token) || ready_payload.contains("auth_token") {
        return Err("ready metadata persisted the Quack credential".into());
    }
    let access = QuackAccess { ready, token };
    let mut checks = vec![check(
        "startup",
        started,
        [("pid", access.ready.pid.into())],
    )];

    let client = QuackClient::connect(access.clone())?;
    let started = Instant::now();
    if client.invocation("SELECT 1").contains(&access.token) {
        return Err("Quack credential leaked into client query text".into());
    }
    let configured_hooks = client.scalar_i64(
        "SELECT count(*)::BIGINT FROM duckdb_settings() \
         WHERE (name = 'quack_authentication_function' AND value = 'quack_check_token') \
            OR (name = 'quack_authorization_function' AND value = 'quack_nop_authorization')",
    )?;
    if configured_hooks != 2 {
        return Err("execution-plane authentication/authorization hooks were not explicit".into());
    }
    let mut invalid_access = access.clone();
    let invalid_token = generate_quack_token()?;
    invalid_access.token = invalid_token.clone();
    let invalid_result = QuackClient::connect(invalid_access)
        .and_then(|client| client.scalar_i64("SELECT 1"))
        .map(|_| ());
    if invalid_result.is_ok() {
        return Err("Quack accepted an invalid authentication token".into());
    }
    let invalid_error = invalid_result.unwrap_err().to_string();
    if invalid_error.contains(&invalid_token) || invalid_error.contains(&access.token) {
        return Err("Quack authentication error disclosed a credential".into());
    }
    checks.push(check("security_profile", started, []));

    let started = Instant::now();
    client.execute_remote("CREATE TEMP TABLE sticky_state AS SELECT 42 AS value")?;
    if client.scalar_i64("SELECT value::BIGINT FROM sticky_state")? != 42 {
        return Err("sticky ATTACH did not retain server-side TEMP state".into());
    }
    if QuackClient::connect(access.clone())?
        .scalar_i64("SELECT value::BIGINT FROM sticky_state")
        .is_ok()
    {
        return Err("server-side TEMP state leaked across Quack connections".into());
    }
    checks.push(check("sticky_attach_session", started, []));

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

    checks.push(probe_master_attach_try_clone(&client, sum)?);

    let started = Instant::now();
    let barrier = Arc::new(Barrier::new(3));
    let mut readers = Vec::new();
    for _ in 0..2 {
        let access = access.clone();
        let barrier = Arc::clone(&barrier);
        readers.push(thread::spawn(move || -> SpikeResult<i64> {
            let client = QuackClient::connect(access)?;
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
        let access = access.clone();
        let barrier = Arc::clone(&barrier);
        writers.push(thread::spawn(move || -> SpikeResult<()> {
            let client = QuackClient::connect(access)?;
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
    let spill_access = access.clone();
    thread::spawn(move || {
        let result = QuackClient::connect(spill_access).and_then(|client| {
            client.execute_remote(
                "SET memory_limit = '96MB'; \
                 CREATE TABLE spill_sorted AS SELECT * FROM facts ORDER BY payload DESC, i DESC",
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
            ("memory_limit_mb", 96.into()),
            ("rows", spill_rows.into()),
            ("peak_spill_bytes", peak_spill_bytes.into()),
        ],
    ));

    let started = Instant::now();
    let (sender, receiver) = mpsc::channel();
    let cancelling_access = access.clone();
    thread::spawn(move || {
        let result =
            QuackClient::connect_with_http_timeout(cancelling_access, 2).and_then(|client| {
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
        let token = generate_quack_token()?;

        let started = Instant::now();
        let mut child = SidecarChild(spawn_sidecar_with_limit(
            &ready_path,
            &spill_path,
            port,
            &database,
            "512MB",
            &token,
        )?);
        let ready = wait_for_ready(&ready_path, &mut child, Duration::from_secs(90))?;
        let startup_ms = started.elapsed().as_millis();
        let client = QuackClient::connect(QuackAccess { ready, token })?;

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
    let client_a = pipeline_a.client();
    if client_a.scalar_i64_stateless("SELECT 42::BIGINT")? != 42 {
        return Err("prewarmed pipeline worker failed its first query".into());
    }
    client_a.execute_stateless("CREATE TABLE pipeline_private(value INTEGER)")?;

    let pipeline_b = pool.checkout("pipeline-b/run-1", Duration::from_secs(2))?;
    if pipeline_a.pid() == pipeline_b.pid() {
        return Err("two pipelines were assigned the same worker".into());
    }
    let client_b = pipeline_b.client();
    let leaked_tables = client_b.scalar_i64_stateless(
        "SELECT count(*)::BIGINT FROM information_schema.tables \
         WHERE table_name = 'pipeline_private'",
    )?;
    if leaked_tables != 0 {
        return Err("pipeline state leaked into another worker".into());
    }
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
    if queued_pipeline
        .client()
        .scalar_i64_stateless("SELECT 99::BIGINT")?
        != 99
    {
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
            "quack_parallelism": DEFAULT_QUACK_PARALLELISM,
            "precreated_clones_per_ready_worker": 0,
            "attach_aliases_per_ready_worker": 0,
            "stateless_quack_query": true,
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

fn ready_worker_smoke() -> SpikeResult<()> {
    let started = Instant::now();
    let worker = start_warm_worker("128MB")?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "status": "ok",
            "worker_pid": worker.pid(),
            "quack_parallelism": DEFAULT_QUACK_PARALLELISM,
            "precreated_clones": 0,
            "attach_aliases": 0,
            "complete_worker_ready_ms": started.elapsed().as_millis(),
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
    let token = generate_quack_token()?;
    let mut child = SidecarChild(spawn_sidecar_with_limit(
        &ready_path,
        &spill_path,
        port,
        ":memory:",
        memory_limit,
        &token,
    )?);
    let ready = wait_for_ready(&ready_path, &mut child, Duration::from_secs(90))?;
    let client = QuackClient::connect_stateless(QuackAccess { ready, token })?;
    if client.scalar_i64_stateless("SELECT 1::BIGINT")? != 1 {
        return Err("worker Quack authentication handshake returned an invalid result".into());
    }
    Ok(WarmWorker {
        client,
        _child: child,
        _run_directory: run_directory,
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
                        // Publication happens only after the sidecar and its
                        // authenticated, query-tested master client are ready.
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
    token: &str,
) -> SpikeResult<Child> {
    spawn_sidecar_with_limit(ready_path, spill_path, port, database, "128MB", token)
}

fn spawn_sidecar_with_limit(
    ready_path: &Path,
    spill_path: &Path,
    port: u16,
    database: &str,
    memory_limit: &str,
    token: &str,
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
        // This is a bootstrap transport for the isolated PoC, not the
        // production secret channel. It avoids command-line and ready-file
        // exposure; the child removes it before DuckDB/Quack startup.
        .env(QUACK_TOKEN_ENV, token)
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
        "usage:\n  duckle-quack-sidecar-spike server --ready PATH [--port N] [--database :memory:|PATH] [--memory-limit 64MB] [--temp-directory PATH]\n  duckle-quack-sidecar-spike query --ready PATH --sql SQL\n  duckle-quack-sidecar-spike smoke\n  duckle-quack-sidecar-spike clone-attach-smoke\n  duckle-quack-sidecar-spike benchmark\n  duckle-quack-sidecar-spike ready-worker-smoke\n  duckle-quack-sidecar-spike pool-smoke"
    );
}

fn main() {
    let arguments: Vec<String> = env::args().skip(1).collect();
    let result = match arguments.first().map(String::as_str) {
        Some("server") => serve(&arguments[1..]),
        Some("query") => query(&arguments[1..]),
        Some("smoke") => smoke(),
        Some("clone-attach-smoke") => clone_attach_smoke(),
        Some("benchmark") => benchmark(),
        Some("ready-worker-smoke") => ready_worker_smoke(),
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
            security_profile: EXECUTION_SECURITY_PROFILE.into(),
            storage: ":memory:".into(),
            temp_directory: "spill".into(),
        };
        assert!(validate_ready(&ready)
            .unwrap_err()
            .to_string()
            .contains("protocol mismatch"));
    }
}
