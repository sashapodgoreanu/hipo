# Implementation Plan: Data Source condivisi e Query Source con affinità DuckDB

**Branch**: `001-shared-data-source-affinity` | **Date**: 2026-07-15 | **Spec**: [spec.md](./spec.md)

## Summary

Il repository oggi persiste Connection come payload cifrato, compila `PipelineDoc` in `Stage` e usa il DuckDB CLI per-stage (con batching limitato all’intera pipeline). Il piano aggiunge un item workspace `data_source`, il componente `src.query` e un piano di affinità basato sulle componenti connesse Query Source↔Data Source. L’esecuzione proposta introduce un worker interno con una sessione CLI persistente per gruppo, mantenendo il confine RuntimeSpec/materializzazione esistente e senza dipendenze esterne.

## Technical Context

| Concern | Duckle baseline | Feature decision |
|---|---|---|
| Languages | Rust 2021; TypeScript/React | Estensioni additive a DTO, planner, executor e UI |
| Desktop/web | Tauri 2 desktop; runner web bridge | Comandi Tauri e bridge web con DTO/eventi paralleli |
| Engine | DuckDB CLI plus Rust RuntimeSpec dispatch | Worker per gruppo; fallback/errore esplicito se la sessione non è supportabile |
| Storage | Workspace JSON per item; Connection cifrate | `data-sources/<id>.json`; solo `connectionRef`, nessun secret |
| Tests | Cargo; frontend lint/build, nessun framework test rilevato | Unit/integration Rust; test helper frontend solo se introdotto un runner |
| Platforms | Windows, macOS, Linux | Spawn/cancel/cleanup portabili, nessuna nuova sidecar |

## Constitution Check

- [x] Preserva contratti serializzati: nuovi campi opzionali/nuovi item e nessuna riscrittura dei Source legacy.
- [x] Mantiene separati frontend, IPC, planner ed executor; il frontend non replica la semantica SQL/affinità.
- [x] Documenta materializzazione, RuntimeSpec, segreti e impatto di sicurezza nei contratti e nel quickstart.
- [x] Nessuna nuova dipendenza/plugin/estensione obbligatoria; le estensioni sono quelle già supportate dal motore.
- [x] Identifica regressioni per planner, CLI, cleanup, bridge e pipeline esistenti.

## Affected Modules and Contracts

| Layer | Paths / types | Change and boundary |
|---|---|---|
| Frontend | `frontend/src/repo-types.ts`, `workspace.ts`, `App.tsx`, `ProjectTree.tsx`, `pipeline-types.ts`, manifests | `data_source`, editor/azioni, `src.query`, persistenza e rename/delete |
| Tauri/web IPC | `apps/desktop/src/lib.rs`, `frontend/src/tauri-bridge.ts`, `crates/duckle-runner/src/serve.rs` | DTO per resolve, test e preview; eventi di contesto; masking |
| Planner | `crates/duckdb-engine/src/plan/mod.rs`, nuovo `plan/affinity.rs`, builders | validazione SQL read-only, risoluzione riferimenti, gruppi, metadati Stage |
| Executor | `crates/duckdb-engine/src/lib.rs`, nuovo `affinity_session.rs` | worker CLI persistente, scheduling DAG, error propagation e cancellation |
| Metadata | `crates/metadata/src/lib.rs` | proprietà serializzate compatibili del nodo Query Source |
| Persistence/secrets | `frontend/src/workspace.ts`, `apps/desktop/src/secrets.rs` | directory payload e risoluzione Connection senza copiare segreti |
| Tests/docs | `crates/duckdb-engine/tests/`, `src/plan/tests.rs`, `docs/architecture/` | regressioni e runbook operativo |

## Design

### Domain and JSON contracts

`RepoItemType` aggiunge `data_source`; `RepoPayload` aggiunge `DataSourcePayload` con `sqlAlias`, `kind`, `connectionRef`, `readOnly`, catalog/schema e opzioni. L’identità autorevole resta `RepoItem.id`; il payload non contiene credenziali. `PAYLOAD_DIR_BY_TYPE` usa `data-sources/`.

`src.query` salva soltanto `dataSourceRefs`, SQL read-only, limite preview e metadati di schema. Il rename confermato aggiorna SQL dipendente; la delete confermata marca i riferimenti non validi. La validazione è case-insensitive sugli alias e rifiuta multi-statement, DDL e DML.

### Planning and execution

Il planner risolve il sottografo selezionato, valida Connection/estensioni e costruisce un grafo bipartito. Ogni componente connessa diventa un `AffinityGroup` run-local con Data Source deduplicati e Query Source ordinate dal DAG.

Il nuovo worker mantiene un processo DuckDB CLI per gruppo, applica estensioni/secret temporanei, esegue `ATTACH` una volta e materializza ogni risultato nel database temporaneo della run. Il planner classifica ogni stage intermedio come `session-preserving`, `session-suspending` o `unsupported`; gli stage non supportati producono un errore di compilazione e non degradano silenziosamente a sessioni differenti. Lo scheduler coordina gli stage compatibili e non introduce parallelismo su uno stesso database finché non è definita una sincronizzazione sicura. Un errore di contesto blocca il gruppo dipendente; un errore Query Source propaga solo ai downstream, lasciando rami indipendenti eseguibili. Cancellation chiude il worker e attiva il cleanup esistente.

### Frontend and IPC

La UI aggiunge palette/editor `src.query`, selezione multipla e dipendenze. Il frontend invia solo riferimenti e metadati non sensibili; la risoluzione autorevole Data Source → Connection → secret avviene nel servizio Rust del desktop o nel boundary autenticato del runner. Il DTO runtime contiene secret solo in memoria e non viene persistito, loggato o restituito al frontend. Si aggiungono comandi `data_source_test` e `query_source_preview` (input: workspace/run DTO, id o node; output: schema/righe/durata/diagnostica sanitizzata) e gli eventi `affinity_context_started`, `data_source_attached`, `query_source_finished`, `affinity_context_finished`. Tauri e web SSE mantengono lo stesso schema serializzabile.

### Secrets, security, and operations

I secret restano cifrati nella Connection; il worker riceve valori solo in memoria/temporary secret files e li rimuove a fine contesto. Errori, history, eventi e preview sono sanitizzati. Read-only è predefinito; filesystem, process spawning, capability e CSP esistenti vanno verificati senza ampliare permessi oltre il necessario.

### Migration and rollout

Nessuna migrazione automatica dei Source. Vecchie pipeline ignorano gli item Data Source e continuano sul percorso attuale. Il formato workspace è versionato/additivo; dati non risolvibili producono diagnostica prima dell’esecuzione. Prima del rilascio va prototipato il framing stdout del CLI interattivo; se non affidabile, la feature resta non abilitata per quel connector invece di degradare silenziosamente la semantica di sessione.

## Test Plan

- Unit: alias/SQL read-only, componenti connesse transitive, deduplicazione attach, stati di errore e sanitizzazione.
- Integration: worker DuckDB, attach una volta, interleaving DAG, partial run, cancellation/cleanup e regressione `two_duckdb_sources_same_database`.
- Frontend: `npm --prefix frontend run lint` e `npm --prefix frontend run build`; nessun framework test esistente da assumere.
- Desktop/IPC: serializzazione command/eventi e parity web bridge; gap E2E documentato (nessuna suite rilevata).
- Commands: `cargo fmt --all --check`; `cargo clippy --workspace --all-targets --exclude duckle-lance`; `cargo test --workspace --exclude duckle-lance`.

## Dependency and ADR Decision

**Nuova dipendenza/plugin/extension/sidecar?** No. Si riusa il DuckDB CLI e le estensioni già presenti.

**ADR needed?** Yes. Va registrata la scelta del worker CLI persistente, il framing/cancellation e la politica di fallback, perché modifica il confine di sessione dell’executor e i contratti IPC.
