# Research: shared Data Source and Query Source affinity

## Decision 1: workspace persistence

**Decision**: extend the existing `RepoItemType`/`RepoPayload` model with a
`data_source` item and `DataSourcePayload`. Persist its payload through the
existing per-item workspace mechanism.

**Rationale**: `frontend/src/workspace.ts` already separates `repository.json`
from per-item payload directories and already handles `connection`, `context`,
`routine`, and other item types. This avoids a parallel registry and preserves
the v2 workspace format pattern.

**Required changes**: add the type and payload union in
`frontend/src/repo-types.ts`; add a `data-sources/` payload directory mapping
and load/save/delete handling in `workspace.ts`; add editor/tree branches in
`App.tsx` and `ProjectTree.tsx`.

**Alternative rejected**: a separate Data Source registry. It would introduce
a second identity/persistence path and complicate dependency lookup and
workspace migration.

## Decision 2: alias lifecycle

**Decision**: an alias rename requires explicit confirmation and propagates the
new alias into dependent Query Source SQL. Deleting a referenced Data Source is
allowed only after explicit confirmation and leaves dependents visibly invalid.

**Rationale**: the user selected automatic SQL propagation for rename and
explicitly accepted invalid references after deletion. Existing `App.tsx`
delete/rename flows do not currently perform dependency analysis, so these
operations must become dependency-aware for `data_source` items.

**Compatibility**: no automatic conversion of legacy Source nodes is performed.

## Decision 3: Query Source contract

**Decision**: introduce component ID `src.query` with properties containing
`dataSourceRefs: string[]`, read-only SQL text, optional preview limit/schema,
and execution metadata only where needed. Query Source accepts `SELECT`, `WITH`
and DuckDB table/function reads; DDL, DML and multi-statement input are rejected.

**Rationale**: the feature is a Source, not a general SQL task. Read-only SQL
keeps remote Data Source side effects outside scope and makes validation and
secret handling testable.

**Current architecture constraint**: component contracts are distributed among
`palette-data.ts`, manifests, planner branches and executor dispatch; there is
no central Component registry to extend.

## Decision 4: affinity calculation

**Decision**: compute connected components of the bipartite graph
`QuerySourceNode ↔ DataSourceId` after resolving the selected execution
subgraph. Give each component a stable run-local affinity group identifier.

**Rationale**: this directly implements the transitive-sharing requirement and
does not depend on canvas order. `compile_partial` already provides the
upstream-subgraph boundary; affinity must run after that filtering.

**Validation**: reject missing Data Source ids, duplicate aliases and
incompatible Connection/Data Source types before execution.

## Decision 5: shared DuckDB session

**Decision**: add an internal affinity-session worker that preserves the
existing DuckDB CLI boundary. A worker owns one DuckDB process/session, loads
extensions, creates temporary secrets, attaches each Data Source once, executes
the group’s Query Source statements, and remains alive while DAG scheduling
coordinates external stages.

**Rationale**: current `DuckdbEngine::run` starts a fresh CLI process per SQL
invocation; attach-backed sources therefore cannot rely on process-local
attachments across stages. A worker/session is the smallest design that can
honor the hard same-session invariant while retaining the current CLI-based
engine and avoiding a new embedded DuckDB dependency.

**Alternatives considered**:

- Extend the existing whole-pipeline batch script: insufficient when a shared
  Query Source group is interleaved with external/runtime stages and when the
  current `-bail` behavior must allow independent branches to continue.
- Recompile each group as separate scripts/processes: preserves attached state
  only by process-local coincidence and does not satisfy the same-session rule.
- Replace the CLI with an embedded DuckDB library: larger dependency and
  multiplatform/licensing/build impact; outside the current engine boundary.

**Implementation risk**: the worker protocol must provide statement/result
framing, cancellation and sanitized errors. The plan includes a focused
prototype/contract test before broad executor changes; if the CLI cannot offer
safe framing, the plan must stop and revisit the chosen boundary before
shipping.

## Decision 6: error policy

**Decision**: a Query Source error fails that node and its downstream data
subgraph; independent branches, including independent Query Sources in the
same affinity context, may continue. Context initialization errors block all
dependent Query Sources.

**Rationale**: this matches the clarified requirement and DAG semantics. The
current sequential executor breaks on the first stage error, so the feature
requires an explicit scheduler/error-state change rather than a documentation
only adjustment.

## Decision 7: first-release Data Source types

**Decision**: support DuckDB, SQLite, PostgreSQL, MySQL/MariaDB and DuckLake
where the existing DuckDB extensions and Connection kinds can represent them.
Keep REST, Kafka, Oracle, SQL Server and other non-catalog Source behavior in
their existing components.

**Rationale**: these initial types can be attached as DuckDB catalogs; the
feature explicitly excludes connectors that cannot be represented by an
attachment/catalog in the first release.

## Confirmed gaps and follow-up

- No current Data Source, Query Source or affinity/session type exists.
- Current `Stage` has no affinity-group or session requirement metadata.
- Current frontend has no Data Source editor/tree entry.
- Current engine stops the sequential pipeline on the first error.
- Frontend tests are not configured; Rust planner/engine integration tests are
  the primary validation layer.
