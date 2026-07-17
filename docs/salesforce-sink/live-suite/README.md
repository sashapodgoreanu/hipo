# Salesforce connector live test suite

Runs the full record lifecycle against a **real Salesforce org** through the
headless runner, exercising the saved-connection resolution added in #166
stage 2 plus the Tier-1 write paths:

| step | pipeline | proves |
|---|---|---|
| s1 | `s1_insert.json` | csv → **insert** via a saved connection (node holds only `connectionRef`) |
| s2 | `s2_retrieve.json` | `src.salesforce` **query** via the same connection → csv |
| s3 | `s3_update.json` | **update** with a *remapped* id column (`Id AS RecId`, `idField: RecId`) |
| s4 | `s4_upsert.json` | **upsert** by `External_ID__c` (overwrites one row, creates another) |
| s5 | `s5_retrieve2.json` | re-read asserting the update, the overwrite, and the new row |
| s6 | `s6_delete.json` | **delete** by retrieved Ids; org left clean |
| t4 | `t4_bearer_inline.json` | inline `${ENV:SF_TOKEN}` Bearer back-compat (no connection) |
| t6/t7 | `t6_wrongkind.json` / `t7_missingref.json` | wrong-kind / missing `connectionRef` fail with clear errors |
| b1 | `b1_bulk_insert.json` | csv → **`snk.salesforce.bulk` insert**: the async ingest-job lifecycle via the same saved connection; streamed success csv carries `sf__Id`/`sf__Created` |
| b2 | `b2_bulk_upsert.json` | bulk **upsert** by `External_ID__c` (overwrites one row, creates another; results csv shows `sf__Created` true+false) |
| b5 | `b5_bulk_badid.json` | a failed record with **no `resultsPath`** — the sampled `sf__Error` must appear in the run error itself |
| b3 | `b3_bulk_delete.json` | bulk **delete** by retrieved Ids; org left clean (`b4_bulk_retrieve.json` is the BULK-\* re-read) |

All suite records carry `External_ID__c = SUITE-*` (Collections steps) or
`BULK-*` (Bulk steps); the delete steps and a final cleanup remove them, so
repeat runs start clean. The Bulk multi-part split (`FILE_SIZE_BYTES`, one
job per ≤90 MB CSV part) needs a >90 MB load and stays a manual test — see
IMPLEMENTATION.md.

## Org prerequisites

1. A Salesforce org you can write test data into (a Developer Edition org or
   sandbox — the suite creates and deletes a handful of `Account` records).
2. **`Account.External_ID__c`**: a custom Text field on Account with
   **External ID** checked (Unique recommended). The upsert step (s4) targets
   it; without the field, s1/s4 fail with `INVALID_FIELD`.
3. An **External Client App** (or Connected App) with the OAuth
   **Client Credentials** flow enabled and a run-as user that can CRUD
   Accounts.

## Credentials

No credential lives in any file here. `connections/sf-live.json` holds
`${ENV:...}` placeholders — the host resolves the `connectionRef` first and the
normal env pass then expands the placeholders (that ordering is part of what
the suite verifies). Supply:

```bash
export SF_INSTANCE_URL=https://yourorg.my.salesforce.com   # My Domain base
export SF_CLIENT_ID=...                                    # consumer key
export SF_CLIENT_SECRET=...
```

(In a real workspace you would instead create the connection in the app and
let the sensitive fields encrypt at rest as `enc:v1:` — the suite uses
placeholders so the repo carries no key material.)

## Run

```bash
export DUCKLE_DUCKDB_BIN=/path/to/duckdb          # as for any headless run
export DUCKLE_RUNNER=/path/to/duckle-runner       # optional; defaults to PATH
bash run-suite.sh
```

Prints a PASS/FAIL line per assertion and exits non-zero on any failure.
This suite talks to a live org, so it is not part of `cargo test`.

## CI

The `salesforce-integration` job in `.github/workflows/ci.yml` runs this suite,
but only when opted in: set the repository **variable** `SF_LIVE_TESTS=true`
and the **secrets** `SF_INSTANCE_URL` / `SF_CLIENT_ID` / `SF_CLIENT_SECRET`.
Without them the job is skipped (fork PRs never see repository secrets, so it
cannot leak into external contributions). Mock-level Salesforce coverage —
including the encrypted-connection → resolution → mint chain
(`crates/duckle-secrets/tests/connection_e2e.rs`) — runs unconditionally in
the main rust matrix.

Note: the post-delete emptiness check treats the source's `ok (0 rows)` as the
clean signal because a query returning 0 records currently materializes a bare
`json` relation that breaks the downstream SQL (#170).
