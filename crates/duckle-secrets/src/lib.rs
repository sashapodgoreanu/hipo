//! Encryption at rest for saved connection secrets, plus run-time resolution
//! of saved-connection references (#166 stage 2).
//!
//! Sensitive connection fields (passwords, tokens, keys) are encrypted with a
//! per-workspace AES-256-GCM key kept at `<workspace>/.duckle/keys/secret.key`
//! (owner-only on unix, excluded from version control). The connection JSON in
//! `<workspace>/connections/` therefore holds ciphertext for those fields, so
//! the folder is safe to commit or share as long as `.duckle/keys/` is not.
//! `${...}` placeholders are never encrypted - they resolve from the
//! environment at run time.
//!
//! This crate is the single decrypt path shared by the desktop app and the
//! headless runner (#166): both call [`resolve_connection_refs`] before their
//! `${ENV:...}` pass so a node that stores only a `connectionRef` gets its
//! auth fields injected in memory - the engine stays credential-agnostic and
//! secrets never land in the pipeline file.

use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use base64::Engine;
use duckle_metadata::PipelineNode;
use serde_json::Value as JsonValue;
use std::path::{Path, PathBuf};

pub const ENC_PREFIX: &str = "enc:v1:";

/// Connection-payload fields (by name) that hold a secret and get encrypted.
pub const SENSITIVE_KEYS: &[&str] = &[
    "password",
    "secretKey",
    "accessKey",
    "accountKey",
    "sessionToken",
    "pat",
    "token",
    "apiKey",
    "passphrase",
    "secret",
    // Salesforce OAuth client-credentials + bearer token (#166 stage 2).
    "clientSecret",
    "accessToken",
];

fn key_path(workspace: &Path) -> PathBuf {
    workspace.join(".duckle").join("keys").join("secret.key")
}

/// Load the workspace key. With `create`, a fresh random 32-byte key is
/// generated and persisted on first use; without it, a missing key is an
/// error (so a workspace shared without the key decrypts to nothing rather
/// than minting a wrong key).
pub fn workspace_key(workspace: &Path, create: bool) -> Result<[u8; 32], String> {
    let path = key_path(workspace);
    if let Ok(bytes) = std::fs::read(&path) {
        if bytes.len() == 32 {
            let mut k = [0u8; 32];
            k.copy_from_slice(&bytes);
            return Ok(k);
        }
    }
    if !create {
        return Err("no workspace key".into());
    }
    let mut k = [0u8; 32];
    getrandom::fill(&mut k).map_err(|e| format!("key rng: {}", e))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create keys dir: {}", e))?;
    }
    // Create the key file owner-only from the start; writing first and
    // chmod'ing after left a brief world-readable window (TOCTOU).
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
            .map_err(|e| format!("create key: {}", e))?;
        f.write_all(&k).map_err(|e| format!("write key: {}", e))?;
    }
    #[cfg(not(unix))]
    std::fs::write(&path, k).map_err(|e| format!("write key: {}", e))?;
    Ok(k)
}

pub fn is_encrypted(s: &str) -> bool {
    s.starts_with(ENC_PREFIX)
}

/// Encrypt plaintext into an `enc:v1:<base64(nonce || ciphertext)>` token.
pub fn encrypt_value(key: &[u8; 32], plaintext: &str) -> Result<String, String> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|e| format!("cipher init: {}", e))?;
    let mut nonce_bytes = [0u8; 12];
    getrandom::fill(&mut nonce_bytes).map_err(|e| format!("nonce rng: {}", e))?;
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce_bytes), plaintext.as_bytes())
        .map_err(|e| format!("encrypt: {}", e))?;
    let mut payload = Vec::with_capacity(12 + ciphertext.len());
    payload.extend_from_slice(&nonce_bytes);
    payload.extend_from_slice(&ciphertext);
    Ok(format!(
        "{}{}",
        ENC_PREFIX,
        base64::engine::general_purpose::STANDARD.encode(payload)
    ))
}

/// Decrypt an `enc:v1:` token back to plaintext.
pub fn decrypt_value(key: &[u8; 32], blob: &str) -> Result<String, String> {
    let b64 = blob
        .strip_prefix(ENC_PREFIX)
        .ok_or("not an encrypted value")?;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| format!("base64: {}", e))?;
    if raw.len() < 12 {
        return Err("ciphertext too short".into());
    }
    let (nonce_bytes, ciphertext) = raw.split_at(12);
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|e| format!("cipher init: {}", e))?;
    let plain = cipher
        .decrypt(Nonce::from_slice(nonce_bytes), ciphertext)
        .map_err(|e| format!("decrypt: {}", e))?;
    String::from_utf8(plain).map_err(|e| format!("utf8: {}", e))
}

/// Walk a JSON value, encrypting or decrypting the sensitive string fields in
/// place. Already-encrypted values and `${...}` placeholders are left alone.
fn transform(value: &mut JsonValue, key: &[u8; 32], encrypting: bool) -> Result<(), String> {
    match value {
        JsonValue::Object(map) => {
            for (k, v) in map.iter_mut() {
                if let Some(s) = v.as_str() {
                    if encrypting {
                        if SENSITIVE_KEYS.contains(&k.as_str())
                            && !s.is_empty()
                            && !is_encrypted(s)
                            && !s.starts_with("${")
                        {
                            // Propagate: never silently leave a secret in
                            // plaintext (the file is meant to hold ciphertext).
                            let enc = encrypt_value(key, s)?;
                            *v = JsonValue::String(enc);
                        }
                    } else if is_encrypted(s) {
                        // Decrypt stays lenient: a missing/legacy value loads
                        // unchanged rather than failing the read.
                        if let Ok(dec) = decrypt_value(key, s) {
                            *v = JsonValue::String(dec);
                        }
                    }
                } else {
                    transform(v, key, encrypting)?;
                }
            }
        }
        JsonValue::Array(arr) => {
            for v in arr.iter_mut() {
                transform(v, key, encrypting)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Encrypt the sensitive fields of a connection payload JSON before it is
/// written to disk. Creates the workspace key on first use.
pub fn encrypt_payload_json(workspace: &Path, payload_json: &str) -> Result<String, String> {
    let key = workspace_key(workspace, true)?;
    let mut v: JsonValue =
        serde_json::from_str(payload_json).map_err(|e| format!("json: {}", e))?;
    transform(&mut v, &key, true)?;
    serde_json::to_string(&v).map_err(|e| format!("json: {}", e))
}

/// Decrypt the sensitive fields of a connection payload JSON after it is read
/// from disk. If the workspace key is missing, the payload is returned
/// unchanged so plaintext / legacy values still load. (Editor-facing and
/// deliberately LENIENT - the run-time path is [`load_connection`], which is
/// strict, because executing with `enc:v1:` ciphertext as a credential is a
/// confusing downstream auth failure.)
pub fn decrypt_payload_json(workspace: &Path, payload_json: &str) -> Result<String, String> {
    let key = match workspace_key(workspace, false) {
        Ok(k) => k,
        Err(_) => return Ok(payload_json.to_string()),
    };
    let mut v: JsonValue =
        serde_json::from_str(payload_json).map_err(|e| format!("json: {}", e))?;
    transform(&mut v, &key, false)?;
    serde_json::to_string(&v).map_err(|e| format!("json: {}", e))
}

/// Any string field anywhere in the value still carrying the `enc:v1:` prefix?
fn any_encrypted(value: &JsonValue) -> bool {
    match value {
        JsonValue::String(s) => is_encrypted(s),
        JsonValue::Object(map) => map.values().any(any_encrypted),
        JsonValue::Array(arr) => arr.iter().any(any_encrypted),
        _ => false,
    }
}

/// Run-time load of `<workspace>/connections/<id>.json`, decrypted. STRICT:
/// a missing file, or an `enc:v1:` field that cannot be decrypted (missing or
/// wrong workspace key), is an error - unlike the lenient editor-facing
/// [`decrypt_payload_json`].
pub fn load_connection(workspace: &Path, id: &str) -> Result<JsonValue, String> {
    let path = workspace.join("connections").join(format!("{}.json", id));
    let txt = std::fs::read_to_string(&path).map_err(|e| {
        format!(
            "connection '{}' not found under {} ({})",
            id,
            workspace.display(),
            e
        )
    })?;
    let mut v: JsonValue = serde_json::from_str(&txt)
        .map_err(|e| format!("connection '{}': invalid JSON: {}", id, e))?;
    if any_encrypted(&v) {
        let key = workspace_key(workspace, false).map_err(|_| {
            format!(
                "connection '{}' holds encrypted fields but {} is missing - \
                 copy the workspace key or re-enter the secrets",
                id,
                key_path(workspace).display()
            )
        })?;
        transform(&mut v, &key, false)?;
        if any_encrypted(&v) {
            return Err(format!(
                "connection '{}' could not be decrypted with the workspace key \
                 (wrong key?)",
                id
            ));
        }
    }
    Ok(v)
}

/// The payload holds a connection JSON object; read a non-empty string field.
fn conn_str<'a>(conn: &'a JsonValue, key: &str) -> Option<&'a str> {
    conn.get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
}

/// Expand a `connectionRef` on a node's properties into the fields the engine
/// reads. Salesforce nodes (#166 stage 2) get their auth fields; every other
/// kind merges its credential/config fields (#185). No-op when no ref is set.
/// The connection WINS over node-level auth props - "ref set => the saved
/// connection defines auth" keeps rotation in one place and avoids half-states
/// mixing stale node fields with connection credentials.
pub fn resolve_connection_ref_props(
    workspace: &Path,
    component_id: &str,
    props: &mut JsonValue,
) -> Result<(), String> {
    let Some(ref_id) = props
        .get("connectionRef")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
    else {
        return Ok(());
    };
    // src.salesforce rides the generic REST source: its form keys the mode as
    // `authType`, the token as `authToken`, and its `url` is user-authored so no
    // instanceUrl is injected. Every other Salesforce node - the Collections sink
    // and both Bulk API 2.0 nodes - owns its own endpoint and uses the sink's
    // `authMode` / `instanceUrl` / `accessToken` keys.
    let is_rest_source = component_id == "src.salesforce";
    let is_salesforce = is_rest_source
        || matches!(
            component_id,
            "snk.salesforce" | "snk.salesforce.bulk" | "src.salesforce.bulk"
        );
    // Salesforce demands its connection (auth is the whole node); every other
    // kind falls back to the node's inline props if the connection can no longer
    // be loaded, so a since-removed connection never hard-fails a pipeline that
    // still carries usable credentials.
    let conn = match load_connection(workspace, &ref_id) {
        Ok(c) => c,
        Err(e) => return if is_salesforce { Err(e) } else { Ok(()) },
    };
    let kind = conn.get("kind").and_then(|v| v.as_str()).unwrap_or("");
    if !is_salesforce {
        // Generic connection (S3 / Postgres / GCS / Azure / ...): merge its
        // credential and config fields onto the node, exactly as the desktop
        // connection picker does, so headless / scheduled / web runs and
        // ref-only pipelines resolve credentials the same way (#185).
        return merge_generic_connection(component_id, kind, &conn, props);
    }
    if kind != "salesforce" {
        return Err(format!(
            "{}: connection '{}' is kind '{}', expected a Salesforce connection",
            component_id, ref_id, kind
        ));
    }
    // Same aliases the engine's salesforce_oauth_from_props accepts.
    let client_credentials = matches!(
        conn.get("authMode")
            .and_then(|v| v.as_str())
            .unwrap_or("bearer"),
        "clientCredentials" | "client_credentials" | "oauth" | "oauth_client_credentials"
    );
    let map = props
        .as_object_mut()
        .ok_or_else(|| format!("{}: node properties are not an object", component_id))?;
    // The sink and Bulk forms key the mode as `authMode`; the REST-shaped source
    // form keys it as `authType` (stage 1, 11af9fb).
    if !is_rest_source {
        map.insert(
            "authMode".into(),
            JsonValue::String(
                if client_credentials {
                    "clientCredentials"
                } else {
                    "bearer"
                }
                .into(),
            ),
        );
    } else {
        map.insert(
            "authType".into(),
            JsonValue::String(
                if client_credentials {
                    "oauth_client_credentials"
                } else {
                    "bearer"
                }
                .into(),
            ),
        );
    }
    for (conn_key, prop_key) in [
        ("loginUrl", "loginUrl"),
        ("clientId", "clientId"),
        ("clientSecret", "clientSecret"),
    ] {
        if let Some(v) = conn_str(&conn, conn_key) {
            map.insert(prop_key.into(), JsonValue::String(v.into()));
        }
    }
    // instanceUrl feeds the sink and Bulk endpoints, which build their own URLs;
    // the REST source's `url` is user-authored (full query URL), so it is never
    // injected there.
    if !is_rest_source {
        if let Some(v) = conn_str(&conn, "instanceUrl") {
            map.insert("instanceUrl".into(), JsonValue::String(v.into()));
        }
    }
    // Bearer-mode saved connection: the sink and Bulk nodes read `accessToken`,
    // the REST-shaped source reads `authToken` (push_rest_auth).
    if let Some(v) = conn_str(&conn, "accessToken") {
        map.insert(
            if is_rest_source { "authToken" } else { "accessToken" }.into(),
            JsonValue::String(v.into()),
        );
    }
    Ok(())
}

/// Merge a saved connection's credential/config fields onto a node's props for
/// any non-Salesforce component (S3, Postgres, GCS, Azure, Snowflake, ...). The
/// connection field names already match what each engine connector reads, so
/// this is the run-time equivalent of the desktop UI's "pick a connection"
/// action - and, unlike that, it also covers headless / scheduled / web runs
/// and pipelines that carry only a `connectionRef`. The connection wins over any
/// stale inline value so credential rotation lives in one place. Runs before the
/// `${ENV:}` pass, so a field stored as `${ENV:...}` still resolves afterwards.
fn merge_generic_connection(
    component_id: &str,
    kind: &str,
    conn: &JsonValue,
    props: &mut JsonValue,
) -> Result<(), String> {
    // The same fields the desktop connection picker copies (PropertiesPanel
    // onPickConnection). Node-specific props (path, format, object, ...) are
    // left untouched.
    const KEYS: &[&str] = &[
        "host",
        "port",
        "database",
        "username",
        "password",
        "bucket",
        "region",
        "accessKey",
        "secretKey",
        "sessionToken",
        "accountName",
        "accountKey",
        "brokers",
        "url",
        "endpoint",
        "urlStyle",
        "sslmode",
        "sslrootcert",
        "sslcert",
        "sslkey",
        "connectTimeout",
        "options",
        "connParams",
    ];
    let map = props
        .as_object_mut()
        .ok_or_else(|| format!("{}: node properties are not an object", component_id))?;
    for key in KEYS {
        let Some(v) = conn.get(*key) else {
            continue;
        };
        // Skip nulls and empty strings so a blank connection field never
        // clobbers a node default.
        if v.is_null() || v.as_str() == Some("") {
            continue;
        }
        if *key == "urlStyle" {
            // Normalize legacy free-text URL styles to DuckDB's canonical
            // 'path' / 'vhost' (matches the UI picker); leave the node default
            // for an unrecognized value.
            if let Some(s) = v.as_str() {
                let low = s.to_lowercase();
                let canon = if low.starts_with("path") {
                    "path"
                } else if low.starts_with("vhost") || low.contains("virtual") {
                    "vhost"
                } else {
                    continue;
                };
                map.insert("urlStyle".into(), JsonValue::String(canon.into()));
                continue;
            }
        }
        map.insert((*key).to_string(), v.clone());
    }
    // Snowflake keys the account identifier as `account`, but the connection
    // stores it in `host` (matches the UI picker).
    if kind == "snowflake" {
        if let Some(h) = conn_str(conn, "host") {
            map.insert("account".into(), JsonValue::String(h.into()));
        }
    }
    Ok(())
}

/// Resolve saved-connection references on every node in a pipeline document, in
/// place. Call BEFORE the `${ENV:...}` pass so a connection field stored as a
/// placeholder still expands afterwards.
pub fn resolve_connection_refs(workspace: &Path, nodes: &mut [PipelineNode]) -> Result<(), String> {
    for node in nodes.iter_mut() {
        let Some(component_id) = node.data.component_id.clone() else {
            continue;
        };
        if let Some(props) = node.data.properties.as_mut() {
            resolve_connection_ref_props(workspace, &component_id, props)?;
        }
    }
    Ok(())
}

/// Does any node in the document carry a non-empty `connectionRef`? Lets a host
/// that has no workspace path fail with a clear message instead of silently
/// running with unresolved credentials.
pub fn has_connection_refs(nodes: &[PipelineNode]) -> bool {
    nodes.iter().any(|node| {
        node.data
            .properties
            .as_ref()
            .and_then(|p| p.get("connectionRef"))
            .and_then(|v| v.as_str())
            .map(|s| !s.is_empty())
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_ws(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("duckle_sec_{}_{}", tag, std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    fn write_connection(ws: &Path, id: &str, payload: &str) {
        let dir = ws.join("connections");
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join(format!("{}.json", id)), payload).unwrap();
    }

    fn sf_node(component_id: &str, props: serde_json::Value) -> PipelineNode {
        serde_json::from_value(serde_json::json!({
            "id": "n1",
            "position": {"x": 0.0, "y": 0.0},
            "data": {
                "label": "sf",
                "componentId": component_id,
                "properties": props,
            }
        }))
        .unwrap()
    }

    #[test]
    fn round_trip_encrypts_only_sensitive_fields() {
        let ws = temp_ws("rt");

        let payload = r#"{"kind":"postgres","host":"db.local","username":"u","password":"s3cr3t","port":5432}"#;
        let enc = encrypt_payload_json(&ws, payload).unwrap();
        // Non-secret fields stay readable; the password becomes ciphertext.
        assert!(
            enc.contains("\"host\":\"db.local\""),
            "host should be plaintext: {}",
            enc
        );
        assert!(
            enc.contains("\"username\":\"u\""),
            "username should be plaintext: {}",
            enc
        );
        assert!(
            enc.contains("enc:v1:"),
            "password should be encrypted: {}",
            enc
        );
        assert!(!enc.contains("s3cr3t"), "plaintext secret leaked: {}", enc);

        let dec = decrypt_payload_json(&ws, &enc).unwrap();
        let v: JsonValue = serde_json::from_str(&dec).unwrap();
        assert_eq!(v["password"], "s3cr3t");
        assert_eq!(v["host"], "db.local");
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn env_placeholders_are_not_encrypted() {
        let ws = temp_ws("env");
        let payload = r#"{"password":"${ENV:PGPASSWORD}"}"#;
        let enc = encrypt_payload_json(&ws, payload).unwrap();
        assert!(
            enc.contains("${ENV:PGPASSWORD}"),
            "placeholder must survive: {}",
            enc
        );
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn salesforce_secret_fields_are_encrypted() {
        let ws = temp_ws("sfenc");
        let payload = r#"{"kind":"salesforce","authMode":"clientCredentials","clientId":"cid","clientSecret":"csecret","accessToken":"atok"}"#;
        let enc = encrypt_payload_json(&ws, payload).unwrap();
        assert!(!enc.contains("csecret"), "clientSecret leaked: {}", enc);
        assert!(!enc.contains("atok"), "accessToken leaked: {}", enc);
        assert!(
            enc.contains("\"clientId\":\"cid\""),
            "clientId is not a secret: {}",
            enc
        );
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn resolve_client_credentials_on_source() {
        let ws = temp_ws("cc_src");
        let enc = encrypt_payload_json(
            &ws,
            r#"{"kind":"salesforce","authMode":"clientCredentials","loginUrl":"https://acme.my.salesforce.com","clientId":"cid","clientSecret":"csecret"}"#,
        )
        .unwrap();
        write_connection(&ws, "sf-prod", &enc);

        let mut node = sf_node(
            "src.salesforce",
            serde_json::json!({"connectionRef": "sf-prod", "authType": "bearer", "url": "https://x/services/data/v60.0/query"}),
        );
        resolve_connection_refs(&ws, std::slice::from_mut(&mut node)).unwrap();
        let props = node.data.properties.unwrap();
        // Connection wins over the stale node-level bearer mode.
        assert_eq!(props["authType"], "oauth_client_credentials");
        assert_eq!(props["loginUrl"], "https://acme.my.salesforce.com");
        assert_eq!(props["clientId"], "cid");
        assert_eq!(props["clientSecret"], "csecret");
        // The user-authored query URL is untouched.
        assert_eq!(props["url"], "https://x/services/data/v60.0/query");
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn resolve_bearer_on_sink_and_source() {
        let ws = temp_ws("bearer");
        let enc = encrypt_payload_json(
            &ws,
            r#"{"kind":"salesforce","authMode":"bearer","instanceUrl":"https://acme.my.salesforce.com","accessToken":"tok123"}"#,
        )
        .unwrap();
        write_connection(&ws, "sf-b", &enc);

        let mut sink = sf_node(
            "snk.salesforce",
            serde_json::json!({"connectionRef": "sf-b"}),
        );
        resolve_connection_refs(&ws, std::slice::from_mut(&mut sink)).unwrap();
        let props = sink.data.properties.unwrap();
        assert_eq!(props["authMode"], "bearer");
        assert_eq!(props["instanceUrl"], "https://acme.my.salesforce.com");
        assert_eq!(props["accessToken"], "tok123");

        let mut src = sf_node(
            "src.salesforce",
            serde_json::json!({"connectionRef": "sf-b"}),
        );
        resolve_connection_refs(&ws, std::slice::from_mut(&mut src)).unwrap();
        let props = src.data.properties.unwrap();
        assert_eq!(props["authType"], "bearer");
        // The REST-shaped source reads the token as authToken.
        assert_eq!(props["authToken"], "tok123");
        assert!(
            props.get("instanceUrl").is_none(),
            "source url is user-authored"
        );
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn resolve_bulk_nodes_use_the_sink_key_shape() {
        let ws = temp_ws("bulk");
        let enc = encrypt_payload_json(
            &ws,
            r#"{"kind":"salesforce","authMode":"clientCredentials","instanceUrl":"https://acme.my.salesforce.com","loginUrl":"https://acme.my.salesforce.com","clientId":"cid","clientSecret":"csecret"}"#,
        )
        .unwrap();
        write_connection(&ws, "sf-bulk", &enc);

        // Both Bulk nodes own their endpoint, so both take the sink's key shape
        // - unlike src.salesforce, which rides the REST form. Without the
        // component-id gate these would fall through to merge_generic_connection
        // and silently resolve with no authMode at all.
        for id in ["snk.salesforce.bulk", "src.salesforce.bulk"] {
            let mut node = sf_node(id, serde_json::json!({"connectionRef": "sf-bulk"}));
            resolve_connection_refs(&ws, std::slice::from_mut(&mut node)).unwrap();
            let props = node.data.properties.unwrap();
            assert_eq!(props["authMode"], "clientCredentials", "{}", id);
            assert_eq!(props["clientId"], "cid", "{}", id);
            assert_eq!(props["clientSecret"], "csecret", "{}", id);
            assert_eq!(props["instanceUrl"], "https://acme.my.salesforce.com", "{}", id);
            assert!(
                props.get("authType").is_none(),
                "{}: authType is the REST source's key",
                id
            );
        }
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn bulk_node_rejects_a_non_salesforce_connection() {
        let ws = temp_ws("bulk_kind");
        let enc =
            encrypt_payload_json(&ws, r#"{"kind":"s3","accessKey":"ak","secretKey":"sk"}"#).unwrap();
        write_connection(&ws, "s3-conn", &enc);
        let mut node = sf_node(
            "snk.salesforce.bulk",
            serde_json::json!({"connectionRef": "s3-conn"}),
        );
        let err = resolve_connection_refs(&ws, std::slice::from_mut(&mut node)).unwrap_err();
        assert!(err.contains("expected a Salesforce connection"), "{}", err);
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn env_placeholder_in_connection_survives_resolution() {
        let ws = temp_ws("cc_env");
        let enc = encrypt_payload_json(
            &ws,
            r#"{"kind":"salesforce","authMode":"clientCredentials","loginUrl":"https://a.my.salesforce.com","clientId":"cid","clientSecret":"${ENV:SF_CLIENT_SECRET}"}"#,
        )
        .unwrap();
        write_connection(&ws, "sf-env", &enc);
        let mut node = sf_node(
            "snk.salesforce",
            serde_json::json!({"connectionRef": "sf-env"}),
        );
        resolve_connection_refs(&ws, std::slice::from_mut(&mut node)).unwrap();
        let props = node.data.properties.unwrap();
        // Injected verbatim; the host's ${ENV:} pass runs after resolution.
        assert_eq!(props["clientSecret"], "${ENV:SF_CLIENT_SECRET}");
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn missing_connection_errors_with_id() {
        let ws = temp_ws("missing");
        let mut node = sf_node(
            "snk.salesforce",
            serde_json::json!({"connectionRef": "nope"}),
        );
        let err = resolve_connection_refs(&ws, std::slice::from_mut(&mut node)).unwrap_err();
        assert!(
            err.contains("'nope'"),
            "error should name the connection: {}",
            err
        );
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn missing_key_is_strict_at_run_time_but_lenient_in_editor() {
        let ws = temp_ws("strict");
        let enc = encrypt_payload_json(
            &ws,
            r#"{"kind":"salesforce","authMode":"clientCredentials","clientId":"cid","clientSecret":"csecret"}"#,
        )
        .unwrap();
        write_connection(&ws, "sf-s", &enc);
        // Simulate a workspace copied without .duckle/keys/.
        std::fs::remove_file(ws.join(".duckle").join("keys").join("secret.key")).unwrap();

        let err = load_connection(&ws, "sf-s").unwrap_err();
        assert!(
            err.contains("secret.key"),
            "run-time load must be strict: {}",
            err
        );
        // Editor load stays lenient: ciphertext passes through unchanged.
        let out = decrypt_payload_json(&ws, &enc).unwrap();
        assert!(
            out.contains("enc:v1:"),
            "editor load stays lenient: {}",
            out
        );
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn refless_nodes_are_untouched() {
        let ws = temp_ws("noop");
        // No connectionRef -> nothing to resolve, both SF and non-SF.
        let mut pg = sf_node("src.postgres", serde_json::json!({"host": "inline"}));
        resolve_connection_refs(&ws, std::slice::from_mut(&mut pg)).unwrap();
        assert_eq!(pg.data.properties.unwrap(), serde_json::json!({"host": "inline"}));

        let mut sf = sf_node("snk.salesforce", serde_json::json!({"object": "Account"}));
        resolve_connection_refs(&ws, std::slice::from_mut(&mut sf)).unwrap();
        assert_eq!(sf.data.properties.unwrap(), serde_json::json!({"object": "Account"}));
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn missing_generic_connection_falls_back_to_inline_props() {
        // A non-SF node whose ref points at a since-removed connection keeps its
        // inline props rather than hard-failing the run (#185).
        let ws = temp_ws("miss");
        let mut node = sf_node(
            "src.s3",
            serde_json::json!({"connectionRef": "gone", "accessKey": "AKINLINE"}),
        );
        resolve_connection_refs(&ws, std::slice::from_mut(&mut node)).unwrap();
        assert_eq!(node.data.properties.unwrap()["accessKey"], "AKINLINE");
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn s3_connection_ref_merges_credentials() {
        // #185: an S3 node carrying only a connectionRef gets its credentials
        // merged from the saved connection, urlStyle normalized, and node-only
        // props (path) preserved. A ${ENV:} value survives for the later pass.
        let ws = temp_ws("s3");
        write_connection(
            &ws,
            "minio",
            r#"{"kind":"s3","accessKey":"AKIA123","secretKey":"${ENV:MASSIVE_SECRET}","region":"eu-west-1","endpoint":"minio.local:9000","urlStyle":"Path (MinIO / B2)","bucket":"flatfiles"}"#,
        );
        let mut node = sf_node(
            "src.s3",
            serde_json::json!({"connectionRef": "minio", "path": "s3://flatfiles/a.csv"}),
        );
        resolve_connection_refs(&ws, std::slice::from_mut(&mut node)).unwrap();
        let p = node.data.properties.unwrap();
        assert_eq!(p["accessKey"], "AKIA123");
        assert_eq!(p["secretKey"], "${ENV:MASSIVE_SECRET}"); // resolved by the env pass later
        assert_eq!(p["region"], "eu-west-1");
        assert_eq!(p["endpoint"], "minio.local:9000");
        assert_eq!(p["urlStyle"], "path"); // legacy label normalized
        assert_eq!(p["path"], "s3://flatfiles/a.csv"); // node-only prop preserved
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn snowflake_connection_ref_maps_host_to_account() {
        let ws = temp_ws("snow");
        write_connection(
            &ws,
            "sf",
            r#"{"kind":"snowflake","host":"acme-xy12345","username":"u","password":"p"}"#,
        );
        let mut node = sf_node("src.snowflake", serde_json::json!({"connectionRef": "sf"}));
        resolve_connection_refs(&ws, std::slice::from_mut(&mut node)).unwrap();
        let p = node.data.properties.unwrap();
        assert_eq!(p["account"], "acme-xy12345");
        assert_eq!(p["username"], "u");
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn wrong_kind_connection_errors() {
        let ws = temp_ws("kind");
        write_connection(&ws, "pg", r#"{"kind":"postgres","host":"db"}"#);
        let mut node = sf_node("snk.salesforce", serde_json::json!({"connectionRef": "pg"}));
        let err = resolve_connection_refs(&ws, std::slice::from_mut(&mut node)).unwrap_err();
        assert!(
            err.contains("kind 'postgres'"),
            "error should name the kind: {}",
            err
        );
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn has_connection_refs_detects_any_ref() {
        let sf = sf_node("snk.salesforce", serde_json::json!({"connectionRef": "x"}));
        let s3 = sf_node("src.s3", serde_json::json!({"connectionRef": "y"}));
        let bare = sf_node("snk.salesforce", serde_json::json!({"object": "Account"}));
        assert!(has_connection_refs(&[sf]));
        assert!(has_connection_refs(&[s3])); // #185: any kind of ref now counts
        assert!(!has_connection_refs(&[bare]));
    }
}
