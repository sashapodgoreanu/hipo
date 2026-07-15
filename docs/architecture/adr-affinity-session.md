# ADR: Affinity session boundary for shared Data Sources

## Status

Proposed for feature `001-shared-data-source-affinity`.

## Decision

Shared Query Sources use one DuckDB CLI process per run-local `AffinityGroup`.
The worker owns the process, temporary database, attachments and temporary
secret material until the group is closed. Existing Source components continue
to use the current per-stage or whole-pipeline batch paths.

Stages inside a group are classified as `session-preserving`,
`session-suspending` or `unsupported`. A suspended stage materializes required
outputs and resumes in the same process; process termination invalidates the
group and never silently falls back to per-stage execution.

## Alternatives rejected

- One script for the entire pipeline: cannot represent multiple affinity groups
  interleaved with RuntimeSpec stages.
- Separate script per group: does not preserve a single session across stage
  boundaries.
- Embedded DuckDB library: changes the existing CLI boundary and multiplatform
  packaging assumptions.

## Risks and validation gate

The CLI stdout framing, cancellation and stderr sanitization must be proven by
an integration test before enabling the worker for a connector. If framing is
not reliable, the connector is rejected for Data Source affinity rather than
silently changing session semantics.
