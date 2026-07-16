# Bug Assessment: Log Message blocks Query Source affinity runs

- **Slug**: query-source-log-affinity
- **Created**: 2026-07-15
- **Source**: pasted screenshot and error message
- **Verdict**: valid
- **Severity**: high

## Report (verbatim or summarized)

A pipeline with two Query Sources feeding a Join, then a `Log Message` pass-through node and CSV sink fails before any node runs. The run reports: `Log Message: SessionSuspending stages cannot yet cross a Query Source affinity session`.

## Symptom

Adding `ctl.log` or `ctl.warn` after a Query Source prevents the pipeline from running, even though the node only logs the upstream row count and passes rows through. The same node works in ordinary non-affinity pipelines.

## Reproduction

1. Create two Query Sources backed by a shared Data Source.
2. Join their outputs.
3. Connect the Join to `ctl.log`, then connect the log node to a CSV sink.
4. Run the pipeline.

## Suspected Code Paths

- `crates/duckdb-engine/src/plan/affinity.rs:160` — classifies every control runtime stage as `SessionSuspending`.
- `crates/duckdb-engine/src/lib.rs:878` — rejects a Query Source pipeline containing a non-preserving affinity stage before execution.
- `crates/duckdb-engine/src/lib.rs:2240` — affinity worker executes SQL stages but does not currently emit the `RuntimeSpec::Log` event.

## Root Cause Hypothesis

High confidence. `ctl.log` and `ctl.warn` have a runtime specification only to emit an event; their data operation is a pure pass-through SQL view. The generic control-node affinity classification marks them as session-suspending, so the executor fails in its preflight check rather than executing the pass-through and logging within the already-owned worker.

## Proposed Remediation

**Preferred**: classify `ctl.log` and `ctl.warn` as session-preserving, and teach the affinity worker to count the upstream relation and emit their `PipelineEvent::Log` after their pass-through SQL has run. This preserves the normal-executor behavior without opening a separate DuckDB connection.

**Files likely to change**:

- `crates/duckdb-engine/src/plan/affinity.rs`
- `crates/duckdb-engine/src/lib.rs`
- `crates/duckdb-engine/tests/execution.rs`

**Tests to add or update**:

- A Query Source → Join → Log Message → CSV integration test, including an assertion that the log event substitutes `{rows}` correctly.

## Risks & Considerations

- Do not treat other control runtimes as preserving: they may need a real suspend/resume boundary.
- Count the upstream relation through the affinity worker so the persistent attachment/session is retained.

## Open Questions

- None.
