//! Trust scorecard: an explainable 0-100 score for a pipeline where every lost
//! point is an itemized finding. Static by default (compile + structural risks +
//! ungoverned PII); pass a [`DuckdbEngine`] to also fold in live schema drift.
//! Shared by the MCP `trust_report` tool, the `duckle-runner` CLI surfaces, and
//! the desktop/web editor command so all of them report the same thing.
//!
//! The structural / PII helpers read the raw pipeline JSON (not the typed
//! structs) so they keep working regardless of how a node serialises.

use crate::{compile_pipeline_sql, DuckdbEngine, PipelineDoc};
use serde_json::{json, Value};

/// Cheap, deterministic graph checks that do not require execution. Reads the
/// raw pipeline JSON so it is independent of the typed engine structs.
pub fn structural_risks(pipeline: &Value) -> Vec<Value> {
    let mut risks: Vec<Value> = Vec::new();
    let (nodes, edges) = match (
        pipeline.get("nodes").and_then(|n| n.as_array()),
        pipeline.get("edges").and_then(|e| e.as_array()),
    ) {
        (Some(n), Some(e)) => (n, e),
        _ => return risks,
    };

    let str_at = |v: &Value, path: &[&str]| -> String {
        let mut cur = v;
        for p in path {
            cur = match cur.get(p) {
                Some(c) => c,
                None => return String::new(),
            };
        }
        cur.as_str().unwrap_or("").to_string()
    };
    let has_incoming =
        |id: &str| edges.iter().any(|e| e.get("target").and_then(|v| v.as_str()) == Some(id));
    let has_outgoing =
        |id: &str| edges.iter().any(|e| e.get("source").and_then(|v| v.as_str()) == Some(id));

    let mut sink_count = 0usize;
    for n in nodes {
        let id = n.get("id").and_then(|v| v.as_str()).unwrap_or("");
        if id.is_empty() {
            continue;
        }
        let cid = str_at(n, &["data", "componentId"]);
        let label = str_at(n, &["data", "label"]);
        let is_source = cid.starts_with("src.");
        let is_sink = cid.starts_with("snk.");
        if is_sink {
            sink_count += 1;
        }

        if cid.starts_with("xf.join") {
            let props = n.get("data").and_then(|d| d.get("properties"));
            let has_key = |k: &str| {
                props
                    .and_then(|p| p.get(k))
                    .and_then(|v| v.as_str())
                    .map(|s| !s.is_empty())
                    .unwrap_or(false)
            };
            if !(has_key("leftKey") && has_key("rightKey")) {
                risks.push(json!({
                    "severity": "warning", "node": id, "label": label,
                    "code": "join_without_keys",
                    "message": "join has no leftKey/rightKey, which can fan out into a cross join"
                }));
            }
        }

        if is_sink && !has_incoming(id) {
            risks.push(json!({
                "severity": "error", "node": id, "label": label,
                "code": "sink_without_input",
                "message": "sink has no incoming edge, so it would write nothing"
            }));
        }

        if !is_source && !is_sink && !has_incoming(id) && !has_outgoing(id) {
            risks.push(json!({
                "severity": "warning", "node": id, "label": label,
                "code": "orphan_node",
                "message": "node is not connected to the rest of the pipeline"
            }));
        }
    }

    if sink_count == 0 {
        risks.push(json!({
            "severity": "warning", "code": "no_sink",
            "message": "pipeline has no sink, so no output is written"
        }));
    }

    risks
}

/// Heuristic, name-based PII classification. Returns a category label when a
/// column name looks like personal data, else None. Keywords are matched against
/// the name with separators removed (so first_name, firstName and FIRSTNAME all
/// hit "firstname"). Deliberately high-precision: these are SUGGESTIONS a human
/// or agent reviews, not a proof, so we favour few false positives over recall.
pub fn looks_like_pii(column: &str) -> Option<&'static str> {
    let norm: String = column
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect();
    // Ordered most-specific first; first match wins so emailAddress -> "email".
    const PATTERNS: &[(&str, &str)] = &[
        ("email", "email"),
        ("ssn", "national_id"),
        ("socialsecurity", "national_id"),
        ("passport", "national_id"),
        ("nationalid", "national_id"),
        ("taxid", "national_id"),
        ("dateofbirth", "date_of_birth"),
        ("birthdate", "date_of_birth"),
        ("birthday", "date_of_birth"),
        ("firstname", "name"),
        ("lastname", "name"),
        ("fullname", "name"),
        ("surname", "name"),
        ("maidenname", "name"),
        ("creditcard", "financial"),
        ("cardnumber", "financial"),
        ("iban", "financial"),
        ("accountnumber", "financial"),
        ("routingnumber", "financial"),
        ("driverlicense", "license"),
        ("driverslicense", "license"),
        ("licensenumber", "license"),
        ("ipaddress", "ip_address"),
        ("streetaddress", "address"),
        ("homeaddress", "address"),
        ("postalcode", "address"),
        ("zipcode", "address"),
        ("phone", "phone"),
        ("mobilenumber", "phone"),
        ("telephone", "phone"),
    ];
    PATTERNS
        .iter()
        .find(|(kw, _)| norm.contains(kw))
        .map(|(_, cat)| *cat)
}

/// Column names a node declares statically in its Schema panel (works without a
/// DuckDB binary). Reads the raw `data.schema` array of `{ name }` entries.
pub fn declared_columns(node: &Value) -> Vec<String> {
    node.get("data")
        .and_then(|d| d.get("schema"))
        .and_then(|s| s.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|c| {
                    c.get("name")
                        .and_then(|n| n.as_str())
                        .or_else(|| c.as_str())
                        .map(|s| s.to_string())
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Build an explainable trust scorecard for a pipeline. Always runs the static
/// checks (compile, structural risks, ungoverned PII). When `drift_engine` is
/// `Some`, it also reads each source's live schema and folds breaking drift into
/// the score; the full drift report is returned under `drift` (else `null`).
/// Resolve any `${...}` placeholders in the pipeline before calling if you want
/// drift to read the real source files.
pub fn trust_report(pipeline: &Value, drift_engine: Option<&DuckdbEngine>) -> Value {
    let mut findings: Vec<Value> = Vec::new();
    let mut deduction: i64 = 0;

    // 1. Does it compile at all? A non-compiling pipeline is the deepest failure.
    let doc_res = serde_json::from_value::<PipelineDoc>(pipeline.clone());
    let compile_res: Result<(), String> = match &doc_res {
        Ok(doc) => compile_pipeline_sql(doc).map(|_| ()).map_err(|e| e.to_string()),
        Err(e) => Err(format!("invalid pipeline: {e}")),
    };
    let compiles = compile_res.is_ok();
    if let Err(e) = &compile_res {
        deduction += 60;
        findings.push(json!({
            "code": "does_not_compile", "severity": "error", "deduction": 60,
            "message": format!("pipeline does not compile: {e}")
        }));
    }

    // 2. Structural risks.
    for r in structural_risks(pipeline) {
        let sev = r["severity"].as_str().unwrap_or("warning").to_string();
        let d = if sev == "error" { 15 } else { 8 };
        deduction += d;
        findings.push(json!({
            "code": r["code"], "severity": sev, "deduction": d, "message": r["message"]
        }));
    }

    // 3. Columns that look like PII but nothing governs (no contract tag, no mask).
    let nodes = pipeline.get("nodes").and_then(|n| n.as_array()).cloned().unwrap_or_default();
    let mut pii_cols: Vec<String> = Vec::new();
    for n in &nodes {
        for c in declared_columns(n) {
            if looks_like_pii(&c).is_some() && !pii_cols.contains(&c) {
                pii_cols.push(c);
            }
        }
    }
    let has_sink = nodes.iter().any(|n| {
        n.pointer("/data/componentId").and_then(|x| x.as_str()).map(|s| s.starts_with("snk.")).unwrap_or(false)
    });
    let governed = nodes.iter().any(|n| {
        let cid = n.pointer("/data/componentId").and_then(|x| x.as_str()).unwrap_or("");
        if cid.starts_with("qa.mask") {
            return true;
        }
        n.pointer("/data/properties/contracts/pii")
            .and_then(|x| x.as_array())
            .map(|a| !a.is_empty())
            .unwrap_or(false)
    });
    if !pii_cols.is_empty() && has_sink && !governed {
        deduction += 10;
        findings.push(json!({
            "code": "ungoverned_pii", "severity": "warning", "deduction": 10,
            "message": format!(
                "column(s) {:?} look like PII but no contract tag or qa.mask governs them; run suggest_contracts",
                pii_cols
            )
        }));
    }

    // 4. Optional live schema drift on the sources (needs a DuckDB binary).
    let mut drift_report: Value = Value::Null;
    if let (Some(engine), Ok(doc)) = (drift_engine, &doc_res) {
        let report = crate::drift::schema_drift(engine, doc);
        let breaking = report["summary"]["breakingSources"].as_u64().unwrap_or(0);
        if breaking > 0 {
            // Cap the count before the cast so the deduction is bounded (max 36)
            // and can never overflow on a surprising count.
            let d = (breaking.min(3) * 12) as i64;
            deduction += d;
            let drifted: Vec<String> = report["sources"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter(|s| s["breaking"] == json!(true))
                        .filter_map(|s| s["nodeId"].as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            findings.push(json!({
                "code": "schema_drift", "severity": "error", "deduction": d,
                "message": format!(
                    "source(s) {:?} drifted from their declared schema (missing columns or type changes); run schema_drift for details",
                    drifted
                )
            }));
        }
        drift_report = report;
    }

    let mut score = (100 - deduction).max(0);
    // A pipeline that does not even compile cannot score as "fine".
    if !compiles {
        score = score.min(20);
    }
    let grade = match score {
        s if s >= 90 => "A",
        s if s >= 80 => "B",
        s if s >= 70 => "C",
        s if s >= 60 => "D",
        _ => "F",
    };

    json!({
        "ok": true,
        "score": score,
        "grade": grade,
        "compiles": compiles,
        "findings": findings,
        "drift": drift_report,
        "summary": format!("{score}/100 ({grade}) from {} finding(s)", findings.len()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_pipeline_scores_high_static() {
        let p = json!({
            "nodes": [
                { "id": "s", "position": { "x": 0, "y": 0 }, "data": { "componentId": "src.csv", "label": "A",
                    "properties": { "path": "in.csv" } } },
                { "id": "k", "position": { "x": 1, "y": 0 }, "data": { "componentId": "snk.csv", "label": "Out",
                    "properties": { "path": "out.csv" } } }
            ],
            "edges": [ { "id": "e1", "source": "s", "target": "k" } ]
        });
        let out = trust_report(&p, None);
        assert_eq!(out["drift"], json!(null));
        assert_eq!(out["compiles"], json!(true));
        assert!(out["score"].as_i64().unwrap() >= 90, "{out}");
    }

    #[test]
    fn ungoverned_pii_is_flagged() {
        let p = json!({
            "nodes": [
                { "id": "s", "position": { "x": 0, "y": 0 }, "data": { "componentId": "src.csv", "label": "People",
                    "properties": { "path": "in.csv" },
                    "schema": [ { "name": "email", "type": "string" }, { "name": "amount", "type": "float64" } ] } },
                { "id": "k", "position": { "x": 1, "y": 0 }, "data": { "componentId": "snk.csv", "label": "Out",
                    "properties": { "path": "out.csv" } } }
            ],
            "edges": [ { "id": "e1", "source": "s", "target": "k" } ]
        });
        let out = trust_report(&p, None);
        let findings = out["findings"].as_array().unwrap();
        assert!(findings.iter().any(|f| f["code"] == "ungoverned_pii"), "{out}");
        assert_eq!(out["score"], json!(90));
    }

    #[test]
    fn structural_risks_flags_join_without_keys_and_no_sink() {
        let p = json!({
            "nodes": [
                { "id": "j", "data": { "componentId": "xf.join", "label": "J", "properties": {} } }
            ],
            "edges": []
        });
        let risks = structural_risks(&p);
        assert!(risks.iter().any(|r| r["code"] == "join_without_keys"), "{risks:?}");
        assert!(risks.iter().any(|r| r["code"] == "no_sink"), "{risks:?}");
    }

    #[test]
    fn pii_heuristic_normalizes_separators_and_case() {
        assert_eq!(looks_like_pii("email_address"), Some("email"));
        assert_eq!(looks_like_pii("FirstName"), Some("name"));
        assert_eq!(looks_like_pii("SSN"), Some("national_id"));
        assert_eq!(looks_like_pii("order_id"), None);
    }
}
