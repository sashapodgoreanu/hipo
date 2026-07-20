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
9. Gate di cutover: con gate non approvato, ogni entry point produttivo resta sul backend di compatibilità; test/compatibility possono selezionare Quack.
10. Profilo o bundle non valido: ricevere `invalid_profile` o `runner_unavailable` sanitizzati e, dopo cutover, zero fallback CLI.
11. Benchmark: congelare manifest con owner, approver, hardware, build, dataset/seed, warm-up, ripetizioni e soglie prima di raccogliere baseline.

Il benchmark e la comparazione CLI/sidecar non sono parte dell'implementazione né della CI della feature. Dopo il completamento integrale della feature, l'owner esegue manualmente le prove con il precedente compilato CLI e il sidecar ufficiale, quindi registra manifest e risultati come evidenza di cutover. Fino a quel momento il gate prestazionale resta non approvato e il percorso di compatibilità rimane attivo.

## T067 — benchmark manuale del proprietario

### 1. Congelare l'identità prima delle misure

Compilare e approvare questa scheda **prima** di eseguire il primo workload. Qualunque variazione successiva di binario, estensione, dataset, hardware, soglia o metodo richiede un nuovo `benchmark_evidence_id`.

| Campo | Valore congelato |
|---|---|
| Release ID | |
| Technical owner | |
| Release approver | |
| Benchmark evidence ID | |
| Commit feature/sidecar | |
| Artifact sidecar + SHA-256 | |
| Commit/artifact baseline CLI + SHA-256 | |
| `Cargo.lock` SHA-256 | |
| DuckDB version | `1.5.4` |
| Quack version | `1.5.4` |
| Quack Windows AMD64 SHA-256 | `3274bac6becc0f750497726a73f9ae858606cec7ec1a935d83a5b84ee0402122` |
| OS e build | |
| CPU / core / thread | |
| RAM | |
| Storage, filesystem e spazio libero | |
| Piano energetico | |
| Dataset ID e seed | |
| Dataset 1M / 10M / 100M | |
| Numero warm-up per cella | |
| Numero ripetizioni misurate per cella | |
| Soglia approvata per workload | |
| Regola di aggregazione approvata | |
| Condizioni di esclusione di una misura | |

L'identità autoritativa del bundle è `crates/duckle-db-runner/src/bundle.rs`. Durante il benchmark devono essere disattivati log diagnostici temporanei:

```powershell
Remove-Item Env:DUCKLE_SIDECAR_DEBUG_LOG -ErrorAction SilentlyContinue
```

### 2. Matrice obbligatoria

Usare gli stessi input e verificare prima l'equivalenza funzionale tra baseline CLI e sidecar. Non cronometrare una cella che produce risultati, row count, schema, side effect o errori differenti.

| Workload | Scopo | Dataset | Consumer |
|---|---|---|---|
| Metadati remoti con output piccolo | SC-009: memoria main non proporzionale al database | 1M, 10M, 100M | 1 |
| SQL remoto | Decision table e baseline | dataset congelato | 1, 2, 4, 8 |
| Trasferimento Quack | Decision table e throughput | dataset congelato | 1, 2, 4, 8 |
| Snapshot Parquet | Decision table e crossover | dataset congelato | 1, 2, 4, 8 |
| Pipeline rappresentativa end-to-end | Parità e costo osservabile complessivo | dataset congelato | 1, 2, 4, 8 |

Per ogni cella registrare almeno:

- comando e configurazione esatti;
- ordine di esecuzione;
- durata di ogni ripetizione, non soltanto il valore aggregato;
- righe e byte trasferiti;
- memoria corrente e di picco del main;
- memoria e spill correnti/di picco del worker quando disponibili;
- CPU ms e `transport_kind` sanitizzati;
- esito funzionale e identificativo dell'output confrontato.

### 3. Regole di esecuzione

1. Usare gli stessi binari congelati, stesso hardware, stesso dataset e stesso filesystem per entrambe le route.
2. Chiudere carichi estranei e mantenere invariati piano energetico, antivirus e configurazione del sistema.
3. Eseguire i warm-up dichiarati senza includerli nelle misure.
4. Alternare l'ordine CLI/sidecar fra le ripetizioni per ridurre bias di cache e temperatura.
5. Non modificare pool, profilo risorse, concorrenza o decision table fra le due route della stessa cella.
6. Conservare output grezzi e comandi fuori dal manifest; nel manifest entra soltanto un `evidence_id` stabile.
7. Un crash, timeout, risultato diverso o cleanup incompleto è un fallimento della cella, non una misura da scartare salvo una regola di esclusione già congelata.

### 4. Risultati per cella

| Workload | Dataset | Consumer | Route | Ripetizioni | Aggregato approvato | Picco main | Picco worker | Spill peak | Rows/bytes | Esito soglia |
|---|---|---:|---|---:|---:|---:|---:|---:|---|---|
| | | 1 | CLI | | | | | | | |
| | | 1 | Sidecar | | | | | | | |
| | | 2 | CLI | | | | | | | |
| | | 2 | Sidecar | | | | | | | |
| | | 4 | CLI | | | | | | | |
| | | 4 | Sidecar | | | | | | | |
| | | 8 | CLI | | | | | | | |
| | | 8 | Sidecar | | | | | | | |

### 5. Crossover Quack/Parquet

Il crossover deve essere una conclusione misurata, non una scelta per component ID.

| Consumer | Ultimo volume favorevole a Quack | Primo volume favorevole a Parquet | Soglia decision table proposta | Evidenza |
|---:|---:|---:|---:|---|
| 1 | | | | |
| 2 | | | | |
| 4 | | | | |
| 8 | | | | |

Se non emerge un crossover stabile, registrare esplicitamente il risultato e mantenere invariata la decision table oppure dichiarare il finding aperto. Non approvare SC-010 senza baseline, soglie pre-approvate e risultati entro soglia per ogni workload obbligatorio.

### 6. Evidenza di cutover

Registrare in questa sezione gli identificativi immutabili, non log o percorsi locali.

| Criterio | Stato | Evidence ID | Nota |
|---|---|---|---|
| SC-001 parità | | | |
| SC-002 zero riferimenti produttivi dopo cutover | | | |
| SC-003 handshake/versione | | | |
| SC-004 redazione | | | |
| SC-005 cancellation/cleanup | | | |
| SC-006 concorrenza 2/4/8 | | | |
| SC-007 isolamento/lease | | | |
| SC-008 spill bounded | | | |
| SC-009 memoria main 1M/10M/100M | | | |
| SC-010 benchmark e crossover | | | |
| SC-011 package offline | | | |

| Finding | Disposizione | Motivazione / evidence ID |
|---|---|---|
| | resolved / accepted / open | |

Il `CutoverEvidence` finale deve contenere:

- `schemaVersion: 1`;
- `releaseId`, `technicalOwner` e `releaseApprover` non vuoti;
- tutti gli SC-001..SC-011 richiesti con `evidenceId` stabile;
- `bundleEvidenceId` e `benchmarkEvidenceId` non vuoti;
- identità bundle corrispondente al target compilato;
- nessun finding `open`; ogni finding accettato deve avere motivazione non vuota.

Non impostare `Pass` sulla base della sola esistenza del test: l'evidence ID deve identificare l'esecuzione o il report congelato che dimostra il criterio.

## Commands

```powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets --exclude duckle-lance
cargo test --workspace --exclude duckle-lance
npm --prefix frontend ci
npm --prefix frontend run lint
npm --prefix frontend run build
```

Eseguire smoke offline Windows/macOS/Linux senza DuckDB CLI. Dopo cutover, cercare riferimenti produttivi a `DUCKLE_DUCKDB_BIN`, `--duckdb`, `AffinitySession`, `affinity_session` e spike Phase 0.
