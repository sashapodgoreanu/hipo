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
9. Gate di cutover: con gate non approvato, ogni entry point produttivo resta
   sul backend di compatibilità; test/compatibility possono selezionare Quack.
10. Profilo o bundle non valido: ricevere `invalid_profile` o
    `runner_unavailable` sanitizzati e, dopo cutover, zero fallback CLI.
11. Benchmark: congelare manifest con owner, approver, hardware, build,
    dataset/seed, warm-up, ripetizioni e soglie prima di raccogliere baseline.

Il benchmark e la comparazione CLI/sidecar non sono parte dell'implementazione
né della CI della feature. Dopo il completamento integrale della feature,
l'owner esegue manualmente le prove con il precedente compilato CLI e il
sidecar ufficiale, quindi registra manifest e risultati come evidenza di
cutover. Fino a quel momento il gate prestazionale resta non approvato e il
percorso di compatibilità rimane attivo.

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
