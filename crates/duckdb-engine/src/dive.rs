//! Helpers for "dives" (live data views): keep a dive's SQL read-only and build
//! a schema card to ground the LLM that generates one. The authoritative safety
//! check is still running the SQL as a read-only view + EXPLAIN against DuckDB;
//! this is the cheap static gate in front of that. See docs/design/dives.md.

use duckle_metadata::Column;

/// Words that must never appear as a keyword in a dive query. A dive only ever
/// reads; anything that writes, attaches, installs, or runs a PRAGMA is rejected
/// before it reaches the engine. AI-generated SQL is treated as untrusted input
/// to this same gate.
const FORBIDDEN: &[&str] = &[
    "insert", "update", "delete", "drop", "alter", "create", "truncate", "attach",
    "detach", "copy", "install", "load", "pragma", "export", "import", "call",
    "set", "reset", "vacuum", "checkpoint", "grant", "revoke",
];

/// Strip line (`--`) and block (`/* */`) comments so keywords inside comments
/// don't trip the checks and can't hide a second statement.
fn strip_comments(sql: &str) -> String {
    let b = sql.as_bytes();
    let mut out = String::with_capacity(sql.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'-' && i + 1 < b.len() && b[i + 1] == b'-' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
        } else {
            out.push(b[i] as char);
            i += 1;
        }
    }
    out
}

/// True if `word` appears in `hay` bounded by non-identifier chars, so
/// `created_at` does not match the `create` denylist entry.
fn contains_word(hay: &str, word: &str) -> bool {
    let bytes = hay.as_bytes();
    let is_ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut from = 0;
    while let Some(pos) = hay[from..].find(word) {
        let start = from + pos;
        let end = start + word.len();
        let before_ok = start == 0 || !is_ident(bytes[start - 1]);
        let after_ok = end >= bytes.len() || !is_ident(bytes[end]);
        if before_ok && after_ok {
            return true;
        }
        from = start + 1;
    }
    false
}

/// Reject a dive query that is not a single read-only SELECT. Returns the
/// trimmed, comment-stripped SQL on success.
pub fn assert_read_only(sql: &str) -> Result<String, String> {
    let cleaned = strip_comments(sql);
    let trimmed = cleaned.trim().trim_end_matches(';').trim().to_string();
    if trimmed.is_empty() {
        return Err("Dive query is empty.".into());
    }
    if trimmed.contains(';') {
        return Err("A dive query must be a single SELECT statement.".into());
    }
    let lower = trimmed.to_ascii_lowercase();
    if !(lower.starts_with("select") || lower.starts_with("with")) {
        return Err("A dive query must start with SELECT (or WITH ... SELECT).".into());
    }
    for kw in FORBIDDEN {
        if contains_word(&lower, kw) {
            return Err(format!("Dive query may not contain `{kw}` - dives are read-only."));
        }
    }
    Ok(trimmed)
}

/// A compact schema card to ground the LLM: the target name + each column and
/// its type, capped so a wide table cannot blow the model's context window.
pub fn render_schema_card(target: &str, columns: &[Column]) -> String {
    const MAX_COLS: usize = 60;
    let mut s = format!("Table: {target}\nColumns:\n");
    for c in columns.iter().take(MAX_COLS) {
        s.push_str(&format!("  - {} ({:?})\n", c.name, c.data_type));
    }
    if columns.len() > MAX_COLS {
        s.push_str(&format!("  ... and {} more columns\n", columns.len() - MAX_COLS));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_select_and_with() {
        assert!(assert_read_only("SELECT 1").is_ok());
        assert!(assert_read_only("  with t as (select 1) select * from t ;").is_ok());
    }

    #[test]
    fn rejects_writes_and_multi_statement() {
        assert!(assert_read_only("DELETE FROM t").is_err());
        assert!(assert_read_only("SELECT 1; DROP TABLE t").is_err());
        assert!(assert_read_only("ATTACH 'x.db'").is_err());
        assert!(assert_read_only("COPY t TO 'x'").is_err());
        assert!(assert_read_only("").is_err());
    }

    #[test]
    fn word_boundary_does_not_overmatch() {
        // `created_at` / `updated_at` must not trip create / update.
        assert!(assert_read_only("SELECT created_at, updated_at FROM t").is_ok());
    }
}
