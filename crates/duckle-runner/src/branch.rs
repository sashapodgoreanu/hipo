//! `duckle-runner branch`: lightweight data branches over a DuckDB database
//! file. A branch is a full copy of the database stored under
//! `<db_dir>/.duckle/branches/<name>.duckdb`. A pipeline can be pointed at the
//! branch, run, and inspected without touching the live database; `promote`
//! swaps the branch into the live file with atomic renames (blue/green),
//! keeping a timestamped backup of the previous live file.
//!
//! Branch operations assume the database is not being written concurrently: a
//! present `<db>.wal` sidecar (an open or un-checkpointed database) makes
//! `create`/`promote` refuse rather than copy or swap an inconsistent file.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use duckle_duckdb_engine::DuckdbEngine;
use serde_json::{json, Value};

pub const BRANCH_USAGE: &str = "\
duckle-runner branch - data branches over a DuckDB database file

USAGE:
    duckle-runner branch <command> [name] --db <database.duckdb> [options]

COMMANDS:
    create <name>     Copy the database to a new branch.
    list              List branches of the database.
    diff <name>       Compare the branch against the live database (tables
                      added/removed and per-table row-count changes). Needs a
                      DuckDB binary.
    promote <name>    Replace the live database with the branch. The previous
                      live file is saved alongside as <stem>.bak-<ms>.duckdb and
                      the branch is consumed.
    delete <name>     Remove a branch.

OPTIONS:
    --db <path>       The DuckDB database file (required). Branches live under
                      its <dir>/.duckle/branches/ folder.
    --duckdb <path>   DuckDB CLI for `diff` (else DUCKLE_DUCKDB_BIN / PATH).
    --json            Emit machine-readable JSON (list / diff).

Branch names may contain letters, digits, '-' and '_' only.

Exit code: 0 ok, 1 diff/promote could not complete, 2 usage/IO error.";

/// Where a database's branches live: `<db_dir>/.duckle/branches/`.
fn branches_dir(db: &Path) -> PathBuf {
    db.parent().unwrap_or_else(|| Path::new(".")).join(".duckle").join("branches")
}

fn branch_path(db: &Path, name: &str) -> PathBuf {
    branches_dir(db).join(format!("{name}.duckdb"))
}

/// Branch names must be a single path-safe segment (no separators, no `..`).
fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_')
}

/// `<path>.wal` - the write-ahead log DuckDB leaves while a database is open or
/// un-checkpointed. Its presence means the file is not a consistent snapshot.
fn wal_of(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(".wal");
    PathBuf::from(s)
}

/// Move a file, falling back to copy+remove when rename crosses devices.
fn move_file(from: &Path, to: &Path) -> Result<(), String> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to)
        .map_err(|e| format!("copy {} -> {}: {e}", from.display(), to.display()))?;
    std::fs::remove_file(from).map_err(|e| format!("remove {}: {e}", from.display()))?;
    Ok(())
}

/// Copy the live database into a new branch. Refuses if a `.wal` sidecar shows
/// the database is open / un-checkpointed (the copy would be inconsistent).
fn create_branch(db: &Path, name: &str) -> Result<PathBuf, String> {
    if !valid_name(name) {
        return Err(format!("invalid branch name: {name:?} (use letters, digits, - and _)"));
    }
    if !db.is_file() {
        return Err(format!("database file does not exist: {}", db.display()));
    }
    if wal_of(db).exists() {
        return Err(format!(
            "{} has an active write-ahead log; close the database (let it checkpoint) before branching",
            db.display()
        ));
    }
    let target = branch_path(db, name);
    if target.exists() {
        return Err(format!("branch already exists: {name}"));
    }
    std::fs::create_dir_all(branches_dir(db)).map_err(|e| format!("create branches dir: {e}"))?;
    std::fs::copy(db, &target)
        .map_err(|e| format!("copy {} -> {}: {e}", db.display(), target.display()))?;
    Ok(target)
}

/// List a database's branches as (name, size-in-bytes), sorted by name.
fn list_branches(db: &Path) -> Result<Vec<(String, u64)>, String> {
    let dir = branches_dir(db);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).map_err(|e| format!("read branches dir: {e}"))?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("duckdb") {
            continue;
        }
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            let bytes = entry.metadata().map(|m| m.len()).unwrap_or(0);
            out.push((stem.to_string(), bytes));
        }
    }
    out.sort();
    Ok(out)
}

/// Delete a branch (and any leftover `.wal` sidecar).
fn delete_branch(db: &Path, name: &str) -> Result<(), String> {
    if !valid_name(name) {
        return Err(format!("invalid branch name: {name:?}"));
    }
    let target = branch_path(db, name);
    if !target.exists() {
        return Err(format!("branch does not exist: {name}"));
    }
    std::fs::remove_file(&target).map_err(|e| format!("remove branch: {e}"))?;
    let wal = wal_of(&target);
    if wal.exists() {
        let _ = std::fs::remove_file(wal);
    }
    Ok(())
}

/// Swap the branch into the live database file. Backs the current live file up
/// to `<stem>.bak-<stamp>.duckdb` (when it exists), then moves the branch into
/// its place. On failure after the backup the original is restored. Returns the
/// backup path when one was made. The branch is consumed (moved, not copied).
fn promote_branch(db: &Path, name: &str, stamp_ms: u128) -> Result<Option<PathBuf>, String> {
    if !valid_name(name) {
        return Err(format!("invalid branch name: {name:?}"));
    }
    let target = branch_path(db, name);
    if !target.exists() {
        return Err(format!("branch does not exist: {name}"));
    }
    // Refuse if either file has an open/un-checkpointed write-ahead log.
    if wal_of(db).exists() {
        return Err(format!(
            "{} has an active write-ahead log; close it before promoting",
            db.display()
        ));
    }
    if wal_of(&target).exists() {
        return Err(format!("branch {name} has an active write-ahead log; close it first"));
    }

    // Back up the current live file, if there is one.
    let mut backup: Option<PathBuf> = None;
    if db.is_file() {
        let stem = db.file_stem().and_then(|s| s.to_str()).unwrap_or("database");
        let bak = db
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(format!("{stem}.bak-{stamp_ms}.duckdb"));
        move_file(db, &bak)?;
        backup = Some(bak);
    }

    // Move the branch into the live file's place; restore on failure.
    if let Err(e) = move_file(&target, db) {
        if let Some(bak) = &backup {
            let _ = move_file(bak, db);
        }
        return Err(format!("promote failed (original restored): {e}"));
    }
    Ok(backup)
}

fn sql_str(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn sql_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

fn json_u64(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

/// Exact per-table row counts for a DuckDB file, keyed by (schema, table). Two
/// read queries: list the user tables, then one UNION ALL of `count(*)`.
fn table_counts(engine: &DuckdbEngine, db: &Path) -> Result<BTreeMap<(String, String), u64>, String> {
    let listing = engine
        .query_db(
            db,
            "SELECT schema_name, table_name FROM duckdb_tables() WHERE NOT internal ORDER BY schema_name, table_name",
            1_000_000,
        )
        .map_err(|e| e.to_string())?;
    let tables: Vec<(String, String)> = listing
        .rows
        .iter()
        .filter_map(|r| {
            let sch = r.get("schema_name").and_then(|v| v.as_str())?;
            let tbl = r.get("table_name").and_then(|v| v.as_str())?;
            Some((sch.to_string(), tbl.to_string()))
        })
        .collect();
    if tables.is_empty() {
        return Ok(BTreeMap::new());
    }
    let union = tables
        .iter()
        .map(|(s, t)| {
            format!(
                "SELECT {} AS sch, {} AS tbl, count(*) AS n FROM {}.{}",
                sql_str(s),
                sql_str(t),
                sql_ident(s),
                sql_ident(t)
            )
        })
        .collect::<Vec<_>>()
        .join(" UNION ALL ");
    let counts = engine.query_db(db, &union, 1_000_000).map_err(|e| e.to_string())?;
    let mut out = BTreeMap::new();
    for r in &counts.rows {
        let sch = r.get("sch").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let tbl = r.get("tbl").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let n = r.get("n").and_then(json_u64).unwrap_or(0);
        out.insert((sch, tbl), n);
    }
    Ok(out)
}

/// Display a (schema, table) key, dropping the `main.` prefix for readability.
fn disp(key: &(String, String)) -> String {
    if key.0 == "main" {
        key.1.clone()
    } else {
        format!("{}.{}", key.0, key.1)
    }
}

/// Compare a branch against the live database: which tables were added/removed
/// and which changed row count. Returns a JSON report.
fn diff_branch(engine: &DuckdbEngine, db: &Path, name: &str) -> Result<Value, String> {
    if !valid_name(name) {
        return Err(format!("invalid branch name: {name:?}"));
    }
    let target = branch_path(db, name);
    if !target.exists() {
        return Err(format!("branch does not exist: {name}"));
    }
    if !db.is_file() {
        return Err(format!("database file does not exist: {}", db.display()));
    }
    let main = table_counts(engine, db)?;
    let branch = table_counts(engine, &target)?;

    let keys: BTreeSet<&(String, String)> = main.keys().chain(branch.keys()).collect();
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut row_changes = Vec::new();
    let mut unchanged = 0u64;
    for k in keys {
        match (main.get(k), branch.get(k)) {
            (None, Some(_)) => added.push(disp(k)),
            (Some(_), None) => removed.push(disp(k)),
            (Some(&m), Some(&b)) => {
                if m == b {
                    unchanged += 1;
                } else {
                    row_changes.push(json!({
                        "table": disp(k),
                        "mainRows": m,
                        "branchRows": b,
                        "delta": b as i64 - m as i64,
                    }));
                }
            }
            (None, None) => unreachable!(),
        }
    }
    Ok(json!({
        "ok": true,
        "db": db.display().to_string(),
        "branch": name,
        "tablesAdded": added,
        "tablesRemoved": removed,
        "rowChanges": row_changes,
        "unchanged": unchanged,
    }))
}

/// Epoch milliseconds for the promote backup stamp.
fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// `duckle-runner branch`: parse args, dispatch, print. Returns the exit code.
pub fn run() -> Result<i32, String> {
    let mut command: Option<String> = None;
    let mut name: Option<String> = None;
    let mut db: Option<PathBuf> = None;
    let mut duckdb_arg: Option<PathBuf> = None;
    let mut as_json = false;
    let mut it = std::env::args().skip(2); // skip the exe and the "branch" verb
    while let Some(a) = it.next() {
        match a.as_str() {
            "--db" => db = Some(PathBuf::from(it.next().ok_or("--db needs a value")?)),
            "--duckdb" => {
                duckdb_arg = Some(PathBuf::from(it.next().ok_or("--duckdb needs a value")?))
            }
            "--json" => as_json = true,
            "-h" | "--help" => {
                println!("{BRANCH_USAGE}");
                return Ok(0);
            }
            other if command.is_none() && !other.starts_with('-') => command = Some(other.to_string()),
            other if name.is_none() && !other.starts_with('-') => name = Some(other.to_string()),
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    let command = command.ok_or("a command is required (create/list/diff/promote/delete)")?;
    let db = db.ok_or("--db <database.duckdb> is required")?;

    match command.as_str() {
        "create" => {
            let name = name.ok_or("create needs a branch name")?;
            let path = create_branch(&db, &name)?;
            let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            println!("created branch {name} at {} ({} bytes)", path.display(), bytes);
            Ok(0)
        }
        "list" => {
            let branches = list_branches(&db)?;
            if as_json {
                let arr: Vec<Value> =
                    branches.iter().map(|(n, b)| json!({ "name": n, "bytes": b })).collect();
                println!("{}", serde_json::to_string_pretty(&json!(arr)).unwrap_or_default());
            } else if branches.is_empty() {
                println!("no branches for {}", db.display());
            } else {
                println!("branches of {}:", db.display());
                for (n, b) in branches {
                    println!("  {n}  ({b} bytes)");
                }
            }
            Ok(0)
        }
        "delete" => {
            let name = name.ok_or("delete needs a branch name")?;
            delete_branch(&db, &name)?;
            println!("deleted branch {name}");
            Ok(0)
        }
        "promote" => {
            let name = name.ok_or("promote needs a branch name")?;
            let backup = promote_branch(&db, &name, now_ms())?;
            match backup {
                Some(bak) => println!(
                    "promoted branch {name} -> {} (previous saved as {})",
                    db.display(),
                    bak.display()
                ),
                None => println!("promoted branch {name} -> {}", db.display()),
            }
            Ok(0)
        }
        "diff" => {
            let name = name.ok_or("diff needs a branch name")?;
            let duckdb = crate::resolve_duckdb(duckdb_arg)?;
            std::env::set_var("DUCKLE_DUCKDB_BIN", &duckdb);
            let engine = DuckdbEngine::new(duckdb);
            let report = diff_branch(&engine, &db, &name)?;
            if as_json {
                println!("{}", serde_json::to_string_pretty(&report).unwrap_or_default());
            } else {
                let arr = |k: &str| report[k].as_array().cloned().unwrap_or_default();
                println!("diff: branch {name} vs {}", db.display());
                for t in arr("tablesAdded") {
                    println!("  + table {}", t.as_str().unwrap_or(""));
                }
                for t in arr("tablesRemoved") {
                    println!("  - table {}", t.as_str().unwrap_or(""));
                }
                for c in arr("rowChanges") {
                    let delta = c["delta"].as_i64().map(|d| format!("  ({d:+})")).unwrap_or_default();
                    println!(
                        "  ~ {}: {} -> {}{}",
                        c["table"].as_str().unwrap_or(""),
                        c["mainRows"].as_u64().unwrap_or(0),
                        c["branchRows"].as_u64().unwrap_or(0),
                        delta
                    );
                }
                let added = arr("tablesAdded").len();
                let removed = arr("tablesRemoved").len();
                let changed = arr("rowChanges").len();
                let unchanged = report["unchanged"].as_u64().unwrap_or(0);
                if added + removed + changed == 0 {
                    println!("  no differences ({unchanged} tables identical)");
                } else {
                    println!(
                        "  summary: +{added} tables  -{removed} tables  ~{changed} row counts  ={unchanged} unchanged"
                    );
                }
            }
            Ok(0)
        }
        other => Err(format!("unknown branch command: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, bytes: &[u8]) {
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn create_list_delete_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("app.duckdb");
        write(&db, b"live-data-v1");

        let bp = create_branch(&db, "feature").unwrap();
        assert!(bp.exists());
        assert_eq!(std::fs::read(&bp).unwrap(), b"live-data-v1");

        // Duplicate create is rejected.
        assert!(create_branch(&db, "feature").is_err());

        let listed = list_branches(&db).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].0, "feature");

        delete_branch(&db, "feature").unwrap();
        assert!(!bp.exists());
        assert!(list_branches(&db).unwrap().is_empty());
    }

    #[test]
    fn create_rejects_invalid_names_and_open_db() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("app.duckdb");
        write(&db, b"x");
        assert!(create_branch(&db, "../escape").is_err());
        assert!(create_branch(&db, "bad/slash").is_err());
        assert!(create_branch(&db, "").is_err());

        // A present .wal means the database is open / un-checkpointed.
        write(&wal_of(&db), b"wal");
        assert!(create_branch(&db, "ok").is_err());
    }

    #[test]
    fn promote_swaps_and_backs_up() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("app.duckdb");
        write(&db, b"live-v1");
        create_branch(&db, "next").unwrap();
        // Mutate the branch so it differs from live.
        write(&branch_path(&db, "next"), b"branch-v2");

        let backup = promote_branch(&db, "next", 1_700_000_000_000).unwrap().unwrap();
        // Live now holds the branch bytes; the branch file is consumed.
        assert_eq!(std::fs::read(&db).unwrap(), b"branch-v2");
        assert!(!branch_path(&db, "next").exists());
        // The old live bytes are preserved in the backup.
        assert_eq!(std::fs::read(&backup).unwrap(), b"live-v1");
    }

    #[test]
    fn promote_refuses_with_open_wal() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("app.duckdb");
        write(&db, b"live");
        create_branch(&db, "next").unwrap();
        write(&wal_of(&db), b"wal");
        assert!(promote_branch(&db, "next", 1).is_err());
        // Live file untouched after a refused promote.
        assert_eq!(std::fs::read(&db).unwrap(), b"live");
    }
}
