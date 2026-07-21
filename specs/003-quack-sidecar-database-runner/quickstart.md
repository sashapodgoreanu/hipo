# Quickstart Validation — Feature 003

## Decisione finale del proprietario

Il 21 luglio 2026 il proprietario della feature ha approvato il passaggio diretto a un solo runtime:

- tutte le pipeline usano il controller e il sidecar Quack;
- non esistono classi `production`, `test`, `compatibility` o `release-ci` configurabili dall'utente;
- CutoverEvidence e il benchmark CLI/sidecar non abilitano o disabilitano più il runtime;
- non esiste fallback alla DuckDB CLI;
- un bundle Quack assente, incompleto o con checksum errato blocca la build oppure restituisce `runner_unavailable`;
- SlothDB e xf.dbt restano leggibili ma disabilitati, senza selezionare un motore alternativo.

Il precedente gate di cutover era stato introdotto per mantenere CLI e Quack in parallelo durante una migrazione graduale. Questa strategia è stata ritirata perché aumentava configurazione, percorsi di esecuzione e possibilità di errore. La parità e i test già implementati restano validi; il confronto prestazionale con il vecchio compilato CLI potrà essere svolto in futuro come analisi, ma non è un prerequisito di attivazione.

## Percorso di build unico

La build desktop prepara automaticamente il frontend, compila `duckle-runner` e `duckle-db-sidecar`, verifica l'estensione Quack inclusa nel repository e incorpora la coppia nel pacchetto Tauri.

Dalla root del repository è sufficiente il normale script:

```bat
@echo off
cd /d "%~dp0apps\desktop"
cargo tauri build
```

Non sono richieste variabili `DUCKLE_ENTRY_POINT_CLASS`, manifest di cutover, percorsi DuckDB CLI o script di preparazione separati.

## Scenari obbligatori

1. Avvio senza profilo: il controller applica i valori predefiniti e gestisce una capacità base di 3 worker.
2. Run normale, partial e preview: ogni run acquisisce una lease dal controller e usa la propria sessione Quack.
3. Cento run senza worker ready: cento assegnazioni on-demand, picco 100 e target warm 120.
4. Seconda ondata entro cinque minuti: utilizzo dei worker warm e mantenimento dell'headroom.
5. Scale-in: termina soltanto worker ready e non interrompe worker leased.
6. Modifica del profilo durante query attive: drain, coalescing e applicazione atomica dell'ultima generazione.
7. Token o versione errati, crash, cancellazione e parent death: errore sanitizzato e cleanup deterministico.
8. Query Source, catalogo condiviso, batch, runtime, spill e trasferimenti: nessuna classificazione affinity.
9. Package offline: sidecar ed estensione sono adiacenti, pin verificato e nessun DuckDB CLI di sistema richiesto.
10. Bundle o profilo non valido: `runner_unavailable` o `invalid_profile`, senza fallback.

## Evidenze già coperte dalla feature

- isolamento per-run e lease esclusive;
- pool warm/on-demand e autoscaling 5 secondi / picco 5 minuti / headroom 20%;
- profilo risorse versionato e applicato prima della readiness;
- handshake autenticato e credenziali non esposte;
- redazione di SQL, token, endpoint, PID e percorsi;
- cancellazione, crash, sweeper e containment;
- preview, partial run, Query Source e concorrenza 2/4/8;
- pin e smoke del package offline Windows/macOS/Linux;
- frontend install, lint e build.

## Criterio di completamento

La feature è completata quando:

- `cargo tauri build` produce il pacchetto desktop senza configurazioni aggiuntive;
- la pipeline desktop usa il sidecar Quack incorporato;
- il repository non contiene il Phase 0 spike;
- non esistono selettori runtime o fallback produttivi verso la DuckDB CLI;
- clippy e test workspace risultano verdi, con `cargo fmt` esplicitamente escluso su decisione del proprietario.
