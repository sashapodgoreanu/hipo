# Salesforce sink (`snk.salesforce`) - implementation notes

Tracks the build for a first-class Salesforce **write** target, per issue #164.
Salesforce ships today as a **source** only (`src.salesforce`, a preset over
`src.rest`); this adds the sink half so Duckle can support ETL/migration **into**
Salesforce, not just out of it.

## Status

**Tier 1 (sObject Collections) - complete: compiles, unit-tested, and validated end-to-end against a live org (including via the desktop UI).**

- `cargo check -p duckle-duckdb-engine` - **passes** (rustc 1.97.0, stable-msvc).
- `cargo clippy -p duckle-duckdb-engine` - the Salesforce code adds **zero**
  warnings (the crate's 37 pre-existing warnings are all in other files and
  predate this change; they reflect a clippy-version drift in the repo, not this
  work).
- `npm --prefix frontend run lint` (`tsc --noEmit`) - **passes clean**.
- **Validated end-to-end against a live org** (Client Credentials, `v60.0`,
  via `duckle-runner --pipeline` → the real `run_salesforce_sink`):
  - **insert** - `src.csv` (2 rows) → 2 Accounts created, verified by SOQL, deleted.
  - **upsert** by `External_ID__c` - run once creates 2; re-run with the same
    external Ids + changed names updates **in place** (still 2 records, same Ids,
    no duplicates). The migration-critical guarantee.
  - **delete** - the sink's Collections `?ids=…` path returns per-record success.
  - **update** by Id - seeded a record, updated its Name through the engine,
    verified, deleted. All four standard operations proven.
- **Integration tests written and passing** (`cargo test -p duckle-duckdb-engine
  snk_salesforce` → 3 passed): `snk_salesforce_insert_posts_collections_envelope`
  (asserts the `attributes.type` envelope + `allOrNone`), `..._upsert_targets_
  external_id_url` (PATCH `/composite/sobjects/{obj}/{extId}`), and
  `..._record_error_fails_run` (a `success:false` fails the run when
  `failOnError`). All mock-server based - no org or secrets needed in CI.

## Tiers

| Tier | Scope | State |
|------|-------|-------|
| 1 | sObject Collections API (`/composite/sobjects`), ≤200 records/request: insert / update / upsert (by external Id) / delete, Bearer auth, per-record error aggregation | **complete** (live-org + mock tests) |
| 2 | Bulk API 2.0 (`/jobs/ingest`): create → upload CSV → close → poll → fetch success/failed result sets. Migration-scale volume. | **complete** - shipped as its own node `snk.salesforce.bulk` (see below), not a mode of `snk.salesforce` |
| 3 | Migration-grade: first-class reject/error output stream, parent→child ID remapping, external-Id relationship resolution, compound-field (Address/Location) handling, API-limit retry/backoff | not started |

## Architecture / files touched

All in `crates/duckdb-engine` unless noted. The sink rides the existing
synchronous `ureq` per-stage model (no tokio), same as the Snowflake/Databricks
HTTP sinks.

| File | Change |
|------|--------|
| `src/plan/specs.rs` | `SalesforceSinkSpec` + `SalesforceWriteApi` enum |
| `src/plan/mod.rs` | `RuntimeSpec::SalesforceSink` variant; `snk.salesforce` routing block (prop parsing, validation, Bulk-API rejection); `salesforce_sink` local + `.or_else(...)` wire |
| `src/lib.rs` | dispatch arm → `run_salesforce_sink` |
| `src/connectors.rs` | `run_salesforce_sink` executor; free helpers `salesforce_record_envelope` (adds the `attributes.type` envelope the Collections API requires and generic `snk.rest` cannot emit) and `parse_salesforce_results` (per-record `{id,success,errors}` accounting) |
| `frontend/src/workflow-ui/palette-data.ts` | new `snk.saas` palette group + `snk('salesforce', …)` |
| `frontend/src/workflow-ui/fields/manifest-synth.ts` | field manifest (in `synthWarehouseSink`, id-dispatched from `dispatchManifest`) |
| `crates/duckle-mcp/catalog.json` | **generated** - do not hand-edit; regenerate via `cd frontend && node scripts/build-catalog.mjs` |

### Endpoints by operation

```
insert  POST   {instance}/services/data/{ver}/composite/sobjects
update  PATCH  {instance}/services/data/{ver}/composite/sobjects
upsert  PATCH  {instance}/services/data/{ver}/composite/sobjects/{object}/{externalIdField}
delete  DELETE {instance}/services/data/{ver}/composite/sobjects?ids=…&allOrNone=…
```

Body (insert/update/upsert): `{ "allOrNone": <bool>, "records": [ { "attributes": {"type": "<object>"}, …row… }, … ] }`
Response: array of `{ "id", "success", "errors": [{statusCode, message, fields}] }`.

### Config surface (see catalog manifest)

`authMode` (`bearer` default | `clientCredentials`),
`instanceUrl` (required in Bearer mode), `accessToken` (required in Bearer
mode - use `${ENV:SF_TOKEN}`), `loginUrl` / `clientId` / `clientSecret`
(required in Client-Credentials mode - #166), `apiVersion` (default `v60.0`),
`object` (required), `operation` (insert|update|upsert|delete),
`externalIdField` (required for upsert), `idField` (default `Id`, for
update/delete), `api` (collections|bulk), `batchSize` (clamped to 200),
`allOrNone` (default false), `failOnError` (default true),
`resultsPath` (optional directory - #166).

### Per-record result files (`resultsPath`)

When `resultsPath` is set, every run writes two Data-Loader-style files into
that directory (created if missing), stamped with the job and UTC run time so
repeat runs accumulate side by side instead of overwriting:

- `{object}_{operation}_{yyyymmddThhmmssZ}_success.csv` - input columns +
  `sf__Id` (the created/updated record Id)
- `{object}_{operation}_{yyyymmddThhmmssZ}_error.csv` - input columns +
  `sf__StatusCode` + `sf__Message`

(e.g. `Account_insert_20260715T033801Z_success.csv`)

Semantics:

- **Both files are always written**, header-only when a side is empty (Data
  Loader parity), and they land **even when the stage errors** - `failOnError`
  aborts, an HTTP error, a cancel - so the reject stream survives a failed run.
- Rows appear positionally: the header is the first row's column order,
  union-extended with later rows' extra columns in first-seen order; input
  columns named `sf__Id`/`sf__StatusCode`/`sf__Message` are skipped so the
  report values win.
- A chunk rejected wholesale (HTTP status/transport error) fails every one of
  its rows with `HTTP_<code>` / `HTTP_TRANSPORT`; an API-level error body
  fails the whole chunk with `API_ERROR` (previously such a body counted as a
  single failure regardless of chunk size). Chunks never attempted because an
  earlier chunk aborted the run appear in **neither** file.
- Cells: strings verbatim, nulls empty, other scalars/nested values in compact
  JSON form; RFC 4180 quoting.

`resultsPath` resolves `${workspace}`/`${ENV:...}` on the host like other path
props (e.g. `${workspace}/out/sf-results`).

In Client-Credentials mode the engine POSTs `grant_type=client_credentials` to
`{loginUrl}/services/oauth2/token` once per run and uses the returned
`{access_token, instance_url}`, so a fresh short-lived token is minted each run
instead of a pasted ~2h Bearer token. `src.salesforce` gains the same mode via
its `authType=oauth_client_credentials` option, so read-Org-A/write-Org-B
migrations work with a connection each side.

`instanceUrl` doubles as the endpoint base a mock server points at in tests
(same trick as the Snowflake sink's `endpoint`).

A runnable **live test suite** (insert → retrieve → update with a remapped
idField → upsert → retrieve → delete, plus auth-matrix and failure-mode
checks, all against a real org via the headless runner) lives in
[`live-suite/`](live-suite/README.md) - credential-free, `${ENV:}`-driven.

## Bulk API 2.0 sink (`snk.salesforce.bulk`)

Tier 2 shipped as a **separate node**, not an `api:"bulk"` mode of `snk.salesforce`.
Its config diverges enough (job polling, a 100 MB upload cap, `hardDelete`, no
`allOrNone`) that one form carrying both would be crowded and misleading. The
never-implemented `api:"bulk"` flag on `snk.salesforce` is retired - the planner
now points that value at this node.

**Data path.** Unlike the Collections sink, which reads the whole view into a
`Vec<JsonValue>`, the Bulk sink lets DuckDB do the CSV I/O:
`COPY (SELECT * FROM <view>) TO '<tmpdir>' (FORMAT CSV, HEADER, FILE_SIZE_BYTES <90 MB>)`.
DuckDB writes numbered parts, each with its own header row, so one part = one
job and a multi-GB load never lands in memory (only one ≤90 MB part is held, at
upload). DuckDB's CSV writer emits LF on every platform (verified on Windows),
so the job declares `lineEnding: LF`.

**The 90 MB target.** Bulk 2.0 accepts ≤150 MB of *base64-encoded* CSV per job;
base64 inflates raw CSV by ~33-50%, so Salesforce advises keeping the raw upload
under 100 MB. `FILE_SIZE_BYTES` is a soft cap (flushes on row-group boundaries,
overshoots a few percent), so the target is **90 MB** and each part is
hard-checked against the 100 MB line before upload. (`BULK_SPLIT_TARGET_BYTES` /
`BULK_UPLOAD_MAX_BYTES` in `connectors.rs`.)

**Lifecycle** (per part, `connectors.rs`): `POST /jobs/ingest` → `PUT
/jobs/ingest/{id}/batches` (text/csv) → `PATCH {state: UploadComplete}` → poll
`GET /jobs/ingest/{id}` until `JobComplete` / `Failed` / `Aborted` → fetch
`successfulResults` / `failedResults` / `unprocessedRecords` (CSV, streamed
verbatim to the stamped result files). `bulk_poll_ingest_job` is a method on the
engine (not a free fn like the Snowflake/Databricks pollers) so it can
`check_cancelled()` every iteration - a Bulk job can run for hours. On timeout or
cancel the in-flight job is aborted (`PATCH {state: Aborted}`).

**Config.** `pollIntervalSecs` (default 5) / `timeoutSecs` (default 3600),
`assignmentRuleId`, and the same auth block as `snk.salesforce`. `resultsPath`
writes `{object}_{operation}_{utc}_success.csv` / `_error.csv` /
`_unprocessed.csv`, accumulating across parts (first part writes the header,
later parts append data rows only). On `failOnError` the run error also inlines
the first 5 sampled `sf__Error` values (Collections-sink parity), so failures
are diagnosable even without a `resultsPath`.

Bulk API 2.0 exposes no `concurrencyMode` or batch-size control (both are Bulk
1.0 concepts): the create-job request accepts only `object` / `operation` /
`contentType` / `columnDelimiter` / `lineEnding` / `externalIdFieldName` /
`assignmentRuleId`, and Salesforce manages internal batching itself, in
parallel. Loads prone to record-lock contention (many children of one parent)
should sort upstream by the parent Id so related records land in the same
internal batch.

**Abort is not rollback.** Salesforce processes an ingest job as internal
batches, each its own transaction - and *in parallel, not in file order*
(verified live: a stalled job had committed row 206,000 while row 203,492 was
still queued). Aborting a job (timeout, cancel) only stops unprocessed batches;
already-committed records stay in the org. So a timed-out run leaves a partial,
non-contiguous load. Salesforce does still serve the result sets of an aborted
job (verified live: 203k success rows fetched from an Aborted job), and the
sink fetches them before surfacing the error, so `resultsPath` captures what
committed - but for recovery-grade certainty query the org (e.g. on the
external-id prefix of the load). Same story on a mid-run failure between parts:
parts that reached JobComplete are committed.

**Result sets stream to disk.** A completed 200k-record job's `successfulResults`
is ~100 MB of CSV. `ureq`'s `into_string()` silently caps at 10 MB - the original
implementation lost exactly this (empty success file on a completed 210k-row
job, found live) - so the result fetch uses `into_reader()` and streams to the
results file without ever buffering the body.

## Bulk API 2.0 query source (`src.salesforce.bulk`)

The read half of migration-scale: a SOQL statement runs as an **async query
job** (`POST /jobs/query`, `operation: query|queryAll` - queryAll includes
deleted and archived records), the same shared poller drives it to
`JobComplete` (cancellable, configurable `pollIntervalSecs`/`timeoutSecs`,
abort on timeout), and the paged CSV result sets stream to a private staging
file that DuckDB `read_csv`s into the node's table - so a multi-GB result set
never lands in memory on either leg.

**Pagination.** Result pages walk `GET /jobs/query/{id}/results` with an
optional `maxRecords` page size. The next page's handle arrives in the
`Sforce-Locator` response header; the last page is signalled by the **literal
string `"null"`** in that header, not by its absence. Pages append through the
same per-file-header logic as the sink's result files, so the staging file
carries exactly one header.

**Typed empty results (#170).** A 0-record query with a declared node schema
materializes a typed empty relation (`materialize_empty_result`); without a
schema it fails with a clear source-level error rather than the bare `json`
column of old. With rows and a declared schema, the columns are pinned via
`all_varchar` + `TRY_CAST` (a stray unparseable cell becomes NULL rather than
failing the load); without one, `read_csv` inference applies.

**SOQL restrictions.** Bulk 2.0 queries reject GROUP BY, OFFSET, TYPEOF,
aggregates and parent-to-child subqueries at job creation; compound fields
must be queried by component. The API's own message surfaces in the run error
(`MALFORMED_QUERY: ...`).

**Auth.** Identical to `snk.salesforce.bulk` - the sink-shaped keys
(`authMode`/`instanceUrl`/`accessToken`, client-credentials mint per run,
saved-connection resolution) - NOT the REST-form `src.salesforce`'s
`authType`/`authToken`.

## Remaining work

Tier 1 and Tier 2 are complete (see Status). What's left is follow-up:

1. **Salesforce auth: OAuth Client-Credentials** - *shipped (#166).* Both `src.salesforce` and both sinks offer a client-credentials `authMode`: the engine mints a fresh short-lived token per run from `clientId`/`clientSecret`/`loginUrl` (`{loginUrl}/services/oauth2/token`) instead of a pasted ~2h Bearer token. A saved encrypted Salesforce connection kind also shipped (#166 stage 2, `duckle-secrets`). Follow-up: JWT-bearer + 401-retry/refresh.
2. **Bulk query source** - *shipped.* `src.salesforce.bulk` (see the section above).
3. **Tier 3** - reject/error output stream (*partially shipped as `resultsPath` success/error files - #166; a first-class reject output port remains*), parent→child ID remapping, external-Id relationship resolution, compound fields (Address/Location), API-limit retry/backoff.

## Contribution checklist (per CONTRIBUTING.md)

Required before opening a PR (fork off `main`, keep it focused; CI runs on
Linux/macOS/Windows; no required-reviewer gate):

- [x] `cargo check -p duckle-duckdb-engine` - **passes** (rustc 1.97.0)
- [~] `cargo clippy` - Salesforce code is warning-free; the crate has 37
      pre-existing warnings unrelated to this change (repo has clippy-version
      drift under stable 1.97, so a blanket `--workspace -- -D warnings` does not
      currently pass on `main` either)
- [x] `cargo test -p duckle-duckdb-engine snk_salesforce` - **3 passed**
      (`DUCKLE_DUCKDB_BIN` pointed at the vendored `.duckdb-cli-v1.5.3/duckdb.exe`)
- [~] `cargo fmt --check` - do NOT run repo-wide: stable rustfmt 1.97 reformats
      ~11k lines of pre-existing code (the maintainers run a different rustfmt
      build). The Salesforce additions were hand-matched to neighbouring style so
      they pass the maintainers' formatter; leave the rest untouched.
- [x] `npm --prefix frontend run lint` (`tsc --noEmit`) - **passes clean**
- [x] Commits: imperative/conventional subject (`feat(salesforce): …`, matches
      real history e.g. `feat(pg):`, `feat(fe):`)
- [x] Written from scratch, no incompatibly-licensed code (dual MIT/Apache-2.0);
      no CLA/DCO sign-off required

**Note - this PR also corrects two stale contributor guides.** `CONTRIBUTING.md`
previously said to add a module under `crates/connectors/src/`, implement a
`Connector`/`Transform` trait from `plugin-sdk`, and add a node under
`frontend/src/canvas/nodes/` - none of which matches reality (`crates/connectors`
and `crates/transform-engine` are legacy stubs; every actual component lives in
`crates/duckdb-engine/` with the frontend wired via `palette-data.ts` +
`manifest-synth.ts`). `docs/roadmap.md` still referenced `plan.rs`. Both were
rewritten to the real pattern this sink follows.

## Build / regenerate

```bash
# backend
cargo build -p duckle-duckdb-engine
cargo test  -p duckle-duckdb-engine salesforce

# catalog (after any palette-data.ts / manifest-synth.ts change)
cd frontend && npm ci && node scripts/build-catalog.mjs
```
