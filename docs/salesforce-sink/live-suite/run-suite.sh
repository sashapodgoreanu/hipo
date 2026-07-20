#!/usr/bin/env bash
# Live Salesforce connector test suite (#166 stage 2 + Tier-1 regression).
# Runs the full record lifecycle against a REAL Salesforce org via the
# headless runner - see README.md for org prerequisites and env vars.
#
#   SF_INSTANCE_URL=... SF_CLIENT_ID=... SF_CLIENT_SECRET=... bash run-suite.sh
#
# Part 1 - lifecycle, every step through the saved connection (connections/
# sf-live.json - a connectionRef-resolved connection whose fields are
# ${ENV:} placeholders, so no credential ever lives in a file):
#   s1  INSERT    csv -> snk.salesforce insert            (SUITE-101/102)
#   s2  RETRIEVE  src.salesforce query -> csv
#   s3  UPDATE    retrieved rows, idField REMAPPED (RecId -> Id)
#   s4  UPSERT    by External_ID__c: overwrites SUITE-101, creates SUITE-201
#   s5  RETRIEVE  re-read; assert update + overwrite + new row
#   s6  DELETE    retrieved Ids; assert org clean
# Part 2 - auth matrix + failure modes:
#   t4  inline ${ENV:} bearer upsert (token minted below; back-compat path)
#   t6  wrong-kind connectionRef (postgres) -> clear error
#   t7  missing connection id              -> clear error
# Part 3 - Bulk API 2.0 sink (snk.salesforce.bulk), records BULK-*:
#   b1  bulk INSERT   csv -> ingest job; streamed success csv (sf__Id)
#   b2  bulk UPSERT   by External_ID__c; sf__Created true+false in results
#   b5  bulk FAILURE  bogus Id, no resultsPath -> sf__Error inlined in run error
#   b3  bulk DELETE   retrieved Ids -> org clean (b4 = the BULK-* retrieve)
#   q1  bulk QUERY    src.salesforce.bulk async query job -> csv (4 rows)
#   q2  bulk QUERY    same query on the clean org -> typed empty (#170)
#
# Suite records carry External_ID__c = SUITE-* / BULK-* and the delete steps +
# final cleanup remove them, so repeat runs start clean.
set -u
cd "$(dirname "$0")"
RUNNER=${DUCKLE_RUNNER:-duckle-runner}
WS=$(pwd)

: "${SF_INSTANCE_URL:?set SF_INSTANCE_URL (e.g. https://acme.my.salesforce.com)}"
: "${SF_CLIENT_ID:?set SF_CLIENT_ID (connected-app consumer key)}"
: "${SF_CLIENT_SECRET:?set SF_CLIENT_SECRET}"

pass=0; fail=0; results=()

check() { # $1 name, $2 expect ("ok" or grep pattern for the error), $3 output
  local name=$1 expect=$2 out=$3
  if [ "$expect" = "ok" ]; then
    if grep -q '^status   : ok' <<<"$out"; then results+=("PASS  $name"); ((pass++)); return 0; fi
  else
    if grep -qi "$expect" <<<"$out" && ! grep -q '^status   : ok' <<<"$out"; then results+=("PASS  $name"); ((pass++)); return 0; fi
  fi
  results+=("FAIL  $name"); ((fail++)); echo "---- $name output ----"; echo "$out" | tail -8
}

assert_file() { # $1 name, $2 file, $3 grep pattern
  if grep -q "$3" "$2" 2>/dev/null; then results+=("PASS  $1"); ((pass++));
  else results+=("FAIL  $1"); ((fail++)); fi
}

run_pipe() { "$RUNNER" --pipeline "$1" --workspace "$WS" 2>&1; }

# SOQL right after a write can briefly return stale rows; retry the read.
retry_read() { # $1 pipeline, $2 out-file, $3 pattern that must appear
  for attempt in 1 2 3; do
    rm -f "$2"; run_pipe "$1" >/dev/null 2>&1
    grep -q "$3" "$2" 2>/dev/null && return 0
    sleep 5
  done
  return 1
}

# ---- Part 1: the record lifecycle -----------------------------------------

rm -rf out/sf-results
out=$(run_pipe s1_insert.json)
check "s1 insert csv -> salesforce (saved connection)" ok "$out"
# #166 resultsPath: s1 also writes Data-Loader-style per-record result files,
# stamped {object}_{operation}_{utc} so repeat runs accumulate.
s_file=$(ls out/sf-results/Account_insert_*_success.csv 2>/dev/null | head -1)
s_rows=$(( $( [ -n "$s_file" ] && wc -l < "$s_file" || echo 0 ) - 1 ))
if [ "$s_rows" -eq 2 ] && grep -q 'sf__Id' "$s_file" 2>/dev/null; then
  results+=("PASS  s1b stamped success csv: 2 rows + sf__Id column"); ((pass++))
else
  results+=("FAIL  s1b stamped success csv: 2 rows + sf__Id column"); ((fail++))
fi
e_file=$(ls out/sf-results/Account_insert_*_error.csv 2>/dev/null | head -1)
e_lines=$(( $( [ -n "$e_file" ] && wc -l < "$e_file" || echo 0 ) ))
if [ "$e_lines" -eq 1 ]; then
  results+=("PASS  s1c stamped error csv written header-only"); ((pass++))
else
  results+=("FAIL  s1c stamped error csv written header-only"); ((fail++))
fi

if retry_read s2_retrieve.json out/retrieved.csv 'SUITE-101'; then
  results+=("PASS  s2 retrieve inserted records"); ((pass++))
else
  results+=("FAIL  s2 retrieve inserted records"); ((fail++))
fi
assert_file "s2b both inserted rows present" out/retrieved.csv 'Suite Insert B'

out=$(run_pipe s3_update.json)
check "s3 update retrieved records (remapped idField RecId)" ok "$out"
if retry_read s5_retrieve2.json out/retrieved2.csv 'Updated Suite Insert B'; then
  results+=("PASS  s3b update visible on re-read"); ((pass++))
else
  results+=("FAIL  s3b update visible on re-read"); ((fail++))
fi

out=$(run_pipe s4_upsert.json)
check "s4 upsert by External_ID__c" ok "$out"

if retry_read s5_retrieve2.json out/retrieved2.csv 'Suite Upsert New'; then
  results+=("PASS  s5 retrieve after upsert (new row present)"); ((pass++))
else
  results+=("FAIL  s5 retrieve after upsert (new row present)"); ((fail++))
fi
assert_file "s5b upsert overwrote SUITE-101"    out/retrieved2.csv 'Suite Upsert Overwrite A'
assert_file "s5c s3 update intact on SUITE-102" out/retrieved2.csv 'Updated Suite Insert B'

out=$(run_pipe s6_delete.json)
check "s6 delete retrieved records" ok "$out"
# Emptiness check: s5's source declares a schema, so a 0-record query flows
# through as a typed empty relation (#170) and the csv lands header-only.
clean=""
for attempt in 1 2 3; do
  sleep 5; rm -f out/retrieved2.csv
  out=$(run_pipe s5_retrieve2.json)
  if [ -f out/retrieved2.csv ]; then lines=$(wc -l < out/retrieved2.csv); else lines=99; fi
  [ "$lines" -le 1 ] && { clean=yes; break; }
done
if [ -n "$clean" ]; then results+=("PASS  s6b org clean after delete"); ((pass++));
else results+=("FAIL  s6b org clean after delete"); ((fail++)); fi

# ---- Part 2: auth matrix + failure modes -----------------------------------

# Mint a bearer token from the same client-credentials app so the inline
# ${ENV:SF_TOKEN} back-compat path is exercised too.
tok_resp=$(curl -s -X POST "${SF_INSTANCE_URL%/}/services/oauth2/token" \
  -d grant_type=client_credentials -d "client_id=$SF_CLIENT_ID" -d "client_secret=$SF_CLIENT_SECRET")
SF_TOKEN=$(sed -n 's/.*"access_token" *: *"\([^"]*\)".*/\1/p' <<<"$tok_resp")
export SF_TOKEN
if [ -n "$SF_TOKEN" ]; then
  out=$(run_pipe t4_bearer_inline.json)
  check "t4 inline \${ENV:} bearer back-compat" ok "$out"
else
  results+=("FAIL  t4 inline bearer (token mint failed: ${tok_resp:0:120})"); ((fail++))
fi

out=$(run_pipe t6_wrongkind.json)
check "t6 wrong-kind connectionRef errors" "expected a Salesforce connection" "$out"

out=$(run_pipe t7_missingref.json)
check "t7 missing connection id errors" "not found" "$out"

# ---- Part 3: Bulk API 2.0 sink (snk.salesforce.bulk) -----------------------
# Same saved connection; the async job lifecycle (create -> upload CSV ->
# UploadComplete -> poll -> stream result sets). Records carry BULK-* so the
# s-steps and b-steps never interfere. Small data = one job per run here; the
# multi-part split (FILE_SIZE_BYTES) needs a >90MB load and stays a manual test.

rm -rf out/sf-bulk-results
out=$(run_pipe b1_bulk_insert.json)
check "b1 bulk insert csv -> ingest job (saved connection)" ok "$out"
# Result sets stream verbatim from Salesforce: success carries sf__Id/sf__Created.
b_file=$(ls out/sf-bulk-results/Account_insert_*_success.csv 2>/dev/null | head -1)
b_rows=$(( $( [ -n "$b_file" ] && wc -l < "$b_file" || echo 0 ) - 1 ))
if [ "$b_rows" -eq 3 ] && grep -q 'sf__Id' "$b_file" 2>/dev/null; then
  results+=("PASS  b1b bulk success csv: 3 rows + sf__Id column"); ((pass++))
else
  results+=("FAIL  b1b bulk success csv: 3 rows + sf__Id column"); ((fail++))
fi

out=$(run_pipe b2_bulk_upsert.json)
check "b2 bulk upsert by External_ID__c" ok "$out"
# The upsert's success csv distinguishes create vs update per record.
b2_file=$(ls -t out/sf-bulk-results/Account_upsert_*_success.csv 2>/dev/null | head -1)
if grep -q '"false"' "$b2_file" 2>/dev/null && grep -q '"true"' "$b2_file" 2>/dev/null; then
  results+=("PASS  b2b upsert csv shows sf__Created true+false"); ((pass++))
else
  results+=("FAIL  b2b upsert csv shows sf__Created true+false"); ((fail++))
fi
if retry_read b4_bulk_retrieve.json out/bulk_retrieved.csv 'Bulk Upsert New'; then
  results+=("PASS  b2c retrieve after bulk upsert (new row present)"); ((pass++))
else
  results+=("FAIL  b2c retrieve after bulk upsert (new row present)"); ((fail++))
fi
assert_file "b2d bulk upsert overwrote BULK-101" out/bulk_retrieved.csv 'Bulk Upsert Overwrite A'

# q1 while the BULK-* records exist: src.salesforce.bulk runs the SOQL as an
# async query job and walks the paged CSV result sets (Sforce-Locator).
rm -f out/bulk_query.csv
out=$(run_pipe q1_bulk_query.json)
check "q1 bulk query source -> csv" ok "$out"
q_lines=$(( $( [ -f out/bulk_query.csv ] && wc -l < out/bulk_query.csv || echo 0 ) ))
if [ "$q_lines" -eq 5 ] && grep -q 'Bulk Upsert New' out/bulk_query.csv; then
  results+=("PASS  q1b bulk query: header + 4 rows incl. the upserted one"); ((pass++))
else
  results+=("FAIL  q1b bulk query: header + 4 rows incl. the upserted one (got $q_lines lines)"); ((fail++))
fi

# b5 BEFORE the delete: failed records must surface IN the run error (sampled
# sf__Error) even with no resultsPath configured on the node.
out=$(run_pipe b5_bulk_badid.json)
check "b5 bulk failed record inlines sf__Error" "INVALID_CROSS_REFERENCE_KEY" "$out"

out=$(run_pipe b3_bulk_delete.json)
check "b3 bulk delete retrieved ids" ok "$out"
bclean=""
for attempt in 1 2 3; do
  sleep 5; rm -f out/bulk_retrieved.csv
  out=$(run_pipe b4_bulk_retrieve.json)
  if [ -f out/bulk_retrieved.csv ]; then blines=$(wc -l < out/bulk_retrieved.csv); else blines=99; fi
  [ "$blines" -le 1 ] && { bclean=yes; break; }
done
if [ -n "$bclean" ]; then results+=("PASS  b3b org clean after bulk delete"); ((pass++));
else results+=("FAIL  b3b org clean after bulk delete"); ((fail++)); fi

# q2: the same bulk query on the now-clean org - 0 records + the declared
# schema must land as a typed empty relation (#170) and a header-only csv.
rm -f out/bulk_query.csv
out=$(run_pipe q1_bulk_query.json)
check "q2 bulk query on 0 records (typed empty)" ok "$out"
q2_lines=$(( $( [ -f out/bulk_query.csv ] && wc -l < out/bulk_query.csv || echo 0 ) ))
if [ "$q2_lines" -eq 1 ]; then
  results+=("PASS  q2b typed-empty csv is header-only"); ((pass++))
else
  results+=("FAIL  q2b typed-empty csv is header-only (got $q2_lines lines)"); ((fail++))
fi

# Final cleanup: t4 upserted SUITE-301 after s6's delete; remove whatever
# SUITE-* and BULK-* remains so repeat runs start clean.
out=$(run_pipe s6_delete.json)
check "final cleanup delete" ok "$out"
out=$(run_pipe b3_bulk_delete.json)
check "final bulk cleanup delete" ok "$out"

echo; echo "==== suite results ===="
printf '%s\n' "${results[@]}"
echo "==== $pass passed, $fail failed ===="
exit $((fail > 0))
