//! Self-contained Arrow Flight SQL client for GizmoSQL (and any Flight SQL
//! server). Pure Rust: it speaks the base Flight gRPC protocol (via the
//! `arrow-flight` crate + tonic) and hand-rolls the handful of Flight SQL command
//! messages with prost, so we avoid arrow-flight's `flight-sql-experimental`
//! feature (which drags in an arrow-arith that doesn't build against our chrono).
//!
//! Source: Handshake (HTTP Basic -> Bearer) -> GetFlightInfo(CommandStatementQuery)
//! -> DoGet -> Arrow batches written to Parquet (materialized via DuckDB
//! read_parquet, like the ADBC source). Sink: DoPut(CommandStatementUpdate) to
//! CREATE then INSERT.

use arrow_array::RecordBatch;
use base64::Engine as _;
use prost::Message as _;
use std::path::Path;

const QUERY_TYPE_URL: &str = "type.googleapis.com/arrow.flight.protocol.sql.CommandStatementQuery";
const UPDATE_TYPE_URL: &str = "type.googleapis.com/arrow.flight.protocol.sql.CommandStatementUpdate";

// Minimal protobuf shapes we need (the rest of Flight SQL is unused here).
#[derive(Clone, PartialEq, prost::Message)]
struct ProtoAny {
    #[prost(string, tag = "1")]
    type_url: String,
    #[prost(bytes = "vec", tag = "2")]
    value: Vec<u8>,
}

#[derive(Clone, PartialEq, prost::Message)]
struct CommandStatement {
    #[prost(string, tag = "1")]
    query: String,
}

#[derive(Clone, PartialEq, prost::Message)]
struct DoPutUpdateResult {
    #[prost(int64, tag = "1")]
    record_count: i64,
}

/// Connection settings for a Flight SQL endpoint.
pub struct GizmoConn {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub tls: bool,
    /// Accept a self-signed / mismatched TLS certificate (GizmoSQL's default TLS
    /// mode generates a self-signed cert).
    pub tls_skip_verify: bool,
}

impl GizmoConn {
    fn uri(&self) -> String {
        let scheme = if self.tls { "https" } else { "http" };
        format!("{}://{}:{}", scheme, self.host, self.port)
    }
}

/// Wrap a SQL command in the Flight SQL Any envelope.
fn command_any(type_url: &str, query: &str) -> Vec<u8> {
    let inner = CommandStatement { query: query.to_string() };
    let any = ProtoAny {
        type_url: type_url.to_string(),
        value: inner.encode_to_vec(),
    };
    any.encode_to_vec()
}

fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("gizmosql: tokio runtime: {}", e))
}

type Client = arrow_flight::flight_service_client::FlightServiceClient<tonic::transport::Channel>;

/// Connect, run the Basic-auth handshake, and return the client plus the Bearer
/// token to attach to every later call.
async fn connect(conn: &GizmoConn) -> Result<(Client, String), String> {
    use arrow_flight::flight_service_client::FlightServiceClient;
    use arrow_flight::HandshakeRequest;
    use tonic::transport::{Channel, ClientTlsConfig};

    let mut endpoint =
        Channel::from_shared(conn.uri()).map_err(|e| format!("gizmosql: bad uri: {}", e))?;
    if conn.tls {
        let tls = ClientTlsConfig::new()
            .domain_name(conn.host.clone())
            .with_native_roots();
        endpoint = endpoint
            .tls_config(tls)
            .map_err(|e| format!("gizmosql: tls config: {}", e))?;
    }
    let channel = endpoint
        .connect()
        .await
        .map_err(|e| format!("gizmosql: connect {}: {}", conn.uri(), e))?;
    let mut client = FlightServiceClient::new(channel)
        .max_decoding_message_size(usize::MAX)
        .max_encoding_message_size(usize::MAX);

    // HTTP Basic on the Handshake call; the server returns a Bearer token in the
    // response metadata that authorizes subsequent RPCs.
    let basic = base64::engine::general_purpose::STANDARD
        .encode(format!("{}:{}", conn.username, conn.password));
    let mut req = tonic::Request::new(futures_util::stream::once(async {
        HandshakeRequest { protocol_version: 0, payload: Default::default() }
    }));
    req.metadata_mut().insert(
        "authorization",
        format!("Basic {}", basic)
            .parse()
            .map_err(|_| "gizmosql: bad auth header".to_string())?,
    );
    let resp = client
        .handshake(req)
        .await
        .map_err(|e| format!("gizmosql: authentication failed: {}", e))?;
    let token = resp
        .metadata()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .ok_or_else(|| "gizmosql: server did not return an authorization token".to_string())?;
    Ok((client, token))
}

fn authed<T>(token: &str, msg: T) -> Result<tonic::Request<T>, String> {
    let mut req = tonic::Request::new(msg);
    req.metadata_mut().insert(
        "authorization",
        token.parse().map_err(|_| "gizmosql: bad token".to_string())?,
    );
    Ok(req)
}

/// Run `sql`, stream the result, write it to a Parquet file. Returns row count.
pub fn query_to_parquet(conn: &GizmoConn, sql: &str, out: &Path) -> Result<usize, String> {
    use arrow_flight::utils::flight_data_to_batches;
    use arrow_flight::{FlightData, FlightDescriptor};
    use futures_util::StreamExt;
    use parquet::arrow::ArrowWriter;

    let rt = runtime()?;
    rt.block_on(async {
        let (mut client, token) = connect(conn).await?;

        let descriptor = FlightDescriptor {
            r#type: arrow_flight::flight_descriptor::DescriptorType::Cmd as i32,
            cmd: command_any(QUERY_TYPE_URL, sql).into(),
            path: vec![],
        };
        let info = client
            .get_flight_info(authed(&token, descriptor)?)
            .await
            .map_err(|e| format!("gizmosql: get_flight_info: {}", e))?
            .into_inner();

        let mut batches: Vec<RecordBatch> = Vec::new();
        for ep in info.endpoint.iter() {
            let ticket = ep
                .ticket
                .clone()
                .ok_or_else(|| "gizmosql: endpoint without a ticket".to_string())?;
            let mut stream = client
                .do_get(authed(&token, ticket)?)
                .await
                .map_err(|e| format!("gizmosql: do_get: {}", e))?
                .into_inner();
            // Collect this endpoint's FlightData (first message is the schema),
            // then decode to RecordBatches.
            let mut data: Vec<FlightData> = Vec::new();
            while let Some(fd) = stream.next().await {
                data.push(fd.map_err(|e| format!("gizmosql: stream: {}", e))?);
            }
            if !data.is_empty() {
                let mut b = flight_data_to_batches(&data)
                    .map_err(|e| format!("gizmosql: decode arrow: {}", e))?;
                batches.append(&mut b);
            }
        }

        let schema = batches
            .first()
            .map(|b| b.schema())
            .or_else(|| {
                info.try_decode_schema().ok().map(std::sync::Arc::new)
            })
            .ok_or_else(|| "gizmosql: response carried no schema".to_string())?;

        let file = std::fs::File::create(out)
            .map_err(|e| format!("gizmosql: create {}: {}", out.display(), e))?;
        let mut writer = ArrowWriter::try_new(file, schema, None)
            .map_err(|e| format!("gizmosql: parquet: {}", e))?;
        let mut rows = 0usize;
        for batch in &batches {
            rows += batch.num_rows();
            writer
                .write(batch)
                .map_err(|e| format!("gizmosql: write parquet: {}", e))?;
        }
        writer
            .close()
            .map_err(|e| format!("gizmosql: close parquet: {}", e))?;
        Ok(rows)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Live test against a local GizmoSQL container. Gated on GIZMOSQL_LIVE so CI
    // (no server) skips it. Run: GIZMOSQL_LIVE=1 cargo test -p duckle-duckdb-engine gizmosql
    #[test]
    fn live_source_and_sink() {
        if std::env::var("GIZMOSQL_LIVE").is_err() {
            return;
        }
        let conn = GizmoConn {
            host: "127.0.0.1".into(),
            port: 31337,
            username: "admin".into(),
            password: "DuckleSecret123".into(),
            tls: false,
            tls_skip_verify: false,
        };
        let dir = std::env::temp_dir().join("duckle_gizmo_live");
        std::fs::create_dir_all(&dir).ok();
        // Source: a literal query.
        let n = query_to_parquet(&conn, "SELECT 42 AS answer, 'hi' AS greeting", &dir.join("a.parquet"))
            .expect("query");
        assert_eq!(n, 1);
        // Sink: create + insert.
        let affected = execute_updates(
            &conn,
            &[
                "CREATE OR REPLACE TABLE duckle_live (a INTEGER, b VARCHAR)".to_string(),
                "INSERT INTO duckle_live VALUES (1, 'x'), (2, 'y'), (3, 'z')".to_string(),
            ],
        )
        .expect("update");
        assert_eq!(affected, 3, "insert should report 3 rows");
        // Read it back.
        let n2 = query_to_parquet(&conn, "SELECT * FROM duckle_live ORDER BY a", &dir.join("b.parquet"))
            .expect("read back");
        assert_eq!(n2, 3);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Run DDL/DML statements (one DoPut each), returning total affected rows.
pub fn execute_updates(conn: &GizmoConn, statements: &[String]) -> Result<i64, String> {
    use arrow_flight::{FlightData, FlightDescriptor};

    let rt = runtime()?;
    rt.block_on(async {
        let (mut client, token) = connect(conn).await?;
        let mut total = 0i64;
        for sql in statements {
            if sql.trim().is_empty() {
                continue;
            }
            let fd = FlightData {
                flight_descriptor: Some(FlightDescriptor {
                    r#type: arrow_flight::flight_descriptor::DescriptorType::Cmd as i32,
                    cmd: command_any(UPDATE_TYPE_URL, sql).into(),
                    path: vec![],
                }),
                data_header: Default::default(),
                app_metadata: Default::default(),
                data_body: Default::default(),
            };
            let stream = futures_util::stream::once(async move { fd });
            let mut resp = client
                .do_put(authed(&token, stream)?)
                .await
                .map_err(|e| format!("gizmosql: do_put: {}", e))?
                .into_inner();
            // First PutResult carries DoPutUpdateResult { record_count } in
            // app_metadata.
            if let Some(put) = resp
                .message()
                .await
                .map_err(|e| format!("gizmosql: do_put result: {}", e))?
            {
                if !put.app_metadata.is_empty() {
                    if let Ok(r) = DoPutUpdateResult::decode(put.app_metadata.as_ref()) {
                        total += r.record_count;
                    }
                }
            }
            // Drain the rest so the server finalizes the statement.
            while let Some(_m) = resp
                .message()
                .await
                .map_err(|e| format!("gizmosql: do_put drain: {}", e))?
            {}
        }
        Ok(total)
    })
}
