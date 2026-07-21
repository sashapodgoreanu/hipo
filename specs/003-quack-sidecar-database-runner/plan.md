# Implementation Plan: Quack Sidecar Database Runner

**Branch**: `003-quack-sidecar-database-runner`  
**Decisione finale**: runtime unico Quack, approvato dal proprietario il 21 luglio 2026.

## Summary

Duckle sostituisce il backend DuckDB CLI con il runner Quack incorporato. Ogni pipeline run entra in `WorkerPoolControl`: il controller assegna atomicamente un worker warm ready oppure crea un worker on-demand. Planner, grafo persistito, eventi e orchestrazione restano autoritativi; cambia il confine di esecuzione.

Non esiste più una fase runtime di cutover né una selezione `production/test/compatibility/release-ci`. Il precedente meccanismo è stato ritirato perché manteneva due percorsi eseguibili e aumentava configurazione, test combinatori e possibilità di fallback involontario.

## Technical Context

| Concern | Decisione finale |
|---|---|
| Linguaggi | Rust 2021, TypeScript/React, MSRV workspace 1.88 |
| Desktop | Tauri 2; un controller long-lived per workspace |
| Motore | `RunDatabase` / `RunSession` attraverso sidecar Quack |
| Packaging | sidecar ed estensione verificata inclusi offline |
| Storage | profilo risorse versionato in `.duckle/settings.json` |
| Test | unit, integrazione, lifecycle, sicurezza, package e frontend |
| Piattaforme | Windows, macOS e Linux con coppia pin per target |

## Affected Modules and Contracts

| Layer | Paths / tipi | Responsabilità |
|---|---|---|
| Frontend | `SettingsModal.tsx`, `tauri-bridge.ts` | profilo completo atomico e diagnostica |
| Tauri | `apps/desktop/src/` e `build.rs` | staging, controller workspace, IPC sottile |
| Planner | `crates/duckdb-engine/src/plan/` | SQL, Stage, RuntimeSpec e batch; nessuna affinity |
| Runner | `crates/duckle-db-runner/` | pool, lease, sessione, provider, sicurezza e metriche |
| Headless/MCP/Scheduler | `crates/duckle-runner`, `duckle-mcp`, `scheduler` | stesso controller e stesso runtime |
| Persistenza | settings, history, run log e watermark | profilo e risultati compatibili e sanitizzati |

## Design

### Controller e pool

Ogni run emette una sola richiesta di acquisizione. Il controller è l’unico proprietario di provisioning, readiness, lease e rilascio. Il target warm è:

```text
max(base_capacity, ceil(peak_5m × 1.20))
```

La valutazione avviene ogni 5 secondi. Lo scale-in termina soltanto worker ready; worker leased completano normalmente. Il restart riparte dalla capacità base.

### Profilo risorse

`RunnerResourcesProfile` contiene versione, memoria, CPU thread, spill, parallelismo Quack e capacità base. Il salvataggio è atomico. Un worker starting non pubblica una generazione superata; un worker leased drena le query e applica l’ultima generazione. Un errore di applicazione produce `configuration_apply_failed` senza stato misto.

### Esecuzione

`DuckdbEngine` delega le pipeline a `RunDatabase`. `RunSession` possiede lease, cancellazione e profilo. SQL puro e Query Source vengono inviati come batch completi. Preview e partial run usano la stessa sessione. SQL remoto, trasferimento Quack e Parquet seguono la decision table versionata.

Non è consentito avviare direttamente un worker né selezionare una DuckDB CLI. Un bundle assente o non verificato restituisce `runner_unavailable`; un profilo non valido restituisce `invalid_profile`. Non esiste fallback.

### Sicurezza

Il sidecar ascolta soltanto su loopback e riceve una credenziale casuale tramite bootstrap ereditato. Readiness richiede profilo effettivo, identità, protocollo, versione e health. Endpoint, token, PID, path, SQL e capability non attraversano IPC, history o log. Process group/Job Object e sweeper gestiscono cancellazione, parent death e orfani.

### Packaging e comando unico

`cargo tauri build` eseguito da `apps/desktop`:

1. compila `duckle-runner` e `duckle-db-sidecar` in release;
2. verifica `crates/duckle-runner/bin/quack.duckdb_extension` tramite SHA-256;
3. rende sidecar ed estensione una coppia adiacente;
4. compila il frontend;
5. incorpora la coppia nel pacchetto Tauri;
6. genera eseguibile e installer offline.

Non sono richiesti script di staging, variabili di classificazione o manifest di cutover.

## Migration and cleanup

- il Phase 0 spike è stato rimosso dopo il trasferimento delle decisioni utili nell’ADR e nei test;
- SlothDB e xf.dbt restano leggibili ma disabilitati;
- il download/install della DuckDB CLI non fa più parte del normale setup desktop;
- le firme interne con nomi storici possono restare temporaneamente per compatibilità sorgente, ma restituiscono sempre Quack e non rappresentano configurazioni;
- il benchmark CLI/sidecar è opzionale e non abilita il runtime.

## Validation

La validazione richiesta comprende:

- test di pool, autoscaling, profilo, sessione, lifecycle e redazione;
- test di preview, partial, Query Source e concorrenza;
- package smoke offline per i target supportati;
- clippy e test workspace;
- frontend install, lint e build;
- build desktop reale tramite il solo `cargo tauri build`.

`cargo fmt` è esplicitamente escluso su decisione del proprietario.

## Completion condition

La feature è completa quando il normale comando Tauri produce il pacchetto, la pipeline desktop usa la coppia Quack incorporata, non esiste un fallback runtime alla CLI e tutti i controlli non esclusi risultano verdi.
