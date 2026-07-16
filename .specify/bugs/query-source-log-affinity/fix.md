# Bug Fix: Log Message blocks Query Source affinity runs

- **Slug**: query-source-log-affinity
- **Fixed**: 2026-07-15
- **Assessment**: ./assessment.md
- **Status**: applied

## Summary

`ctl.log` and `ctl.warn` now remain inside a Query Source affinity session. The persistent DuckDB worker emits their log event after executing the pass-through SQL and counts the upstream relation without opening a separate session.

## Changes

| File | Change | Notes |
|------|--------|-------|
| `crates/duckdb-engine/src/plan/affinity.rs` | modified | Classifies `ctl.log` and `ctl.warn` as session-preserving. |
| `crates/duckdb-engine/src/lib.rs` | modified | Emits `RuntimeSpec::Log` events from the affinity worker. |
| `crates/duckdb-engine/tests/execution.rs` | added test | Covers Query Source → Join → Log Message → CSV and `{rows}` interpolation. |

## Tests Added or Updated

- `crates/duckdb-engine/tests/execution.rs::query_source_join_log_message_stays_in_affinity_session` — validates the screenshot's graph shape, sink output, and emitted log message.

## Local Verification

- `cargo test -p duckle-duckdb-engine query_source_join_log_message_stays_in_affinity_session -- --nocapture` → passed.
- `cargo test -p duckle-duckdb-engine --test execution query_source -- --nocapture` → 5 passed.
- `git diff --check` → passed.

## Deviations from Assessment

None.

## Follow-ups

- Other runtime control nodes remain session-suspending until explicit suspend/resume semantics are implemented.
