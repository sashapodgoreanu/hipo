# Quickstart Validation — Feature 003

## Scenari

1. Avvio senza profilo: verificare 3 worker warm autenticati.
2. Run normale, partial e preview: tutte acquisiscono sessione dal controller e preservano risultati/eventi.
3. 100 run con zero ready: 100 decisioni on-demand, picco 100 e target warm 120.
4. Seconda ondata da 100 entro cinque minuti: 100 lease warm e almeno 20 ready.
5. Cinque minuti senza domanda: scale-in solo ready; restart torna alla base 3.
6. Cambiare RunnerResourcesProfile durante 1/2/4/8 query: vecchie query completano, nuove usano ultima generazione atomica.
7. Token/versione errati, crash, cancel e parent death: errore sanitizzato, nessun secret, cleanup entro 10 s.
8. Query Source attachment, batch, runtime, spill e Parquet fallback: nessun routing affinity.

## Commands

```powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets --exclude duckle-lance
cargo test --workspace --exclude duckle-lance
npm --prefix frontend ci
npm --prefix frontend run lint
npm --prefix frontend run build
```

Eseguire smoke offline Windows/macOS/Linux senza DuckDB CLI. Dopo cutover, cercare riferimenti produttivi a DUCKLE_DUCKDB_BIN, --duckdb, AffinitySession, affinity_session e spike Phase 0.

