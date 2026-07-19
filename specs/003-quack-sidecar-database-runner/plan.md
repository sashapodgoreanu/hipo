# Implementation Plan: Quack Sidecar Database Runner

**Branch**: 003-quack-sidecar-database-runner | **Date**: 2026-07-18 | **Spec**: [spec.md](spec.md)

## Summary

Duckle sostituisce il backend DuckDB CLI con un runner Quack ufficiale. Ogni pipeline run entra in WorkerPoolControl: il controller assegna atomicamente un worker warm ready oppure crea e assegna un worker on-demand. Planner, grafo persistito, eventi e orchestrazione restano autoritativi; cambia solo il confine di esecuzione. Il target warm è max(base, ceil(picco_5m × 1,20)), con base 3, valutazione ogni 5 secondi e scale-in dopo 5 minuti.

## Technical Context

| Concern | Duckle baseline | Feature decision |
|---|---|---|
| Languages | Rust 2021; TypeScript/React | Nuovo crate ufficiale duckle-db-runner; alzare MSRV workspace a Rust 1.88. La coppia DuckDB/Quack richiede 1.85.1, ma l'attuale lockfile completo desktop richiede 1.88. |
| Desktop/web | Tauri 2 e runner web | Controller long-lived per istanza/workspace; DTO IPC additivi per profilo e diagnostica. |
| Engine | DuckDB CLI e RuntimeSpec | RunDatabase/RunSession Quack dietro l'esecutore; Stage e RuntimeSpec restano invariati. |
| Storage | Workspace JSON e file run | RunnerResourcesProfile compatibile in settings; picco/target oltre base effimeri. |
| Tests | Cargo, frontend build | Unit controller/profilo, integrazione sidecar/parità, IPC/UI, packaging offline. |
| Platforms | Windows, macOS, Linux | Coppia runner/estensione pin e bundle per target; containment e staging specifici. |

## Constitution Check

- [x] PipelineDoc, nodi, edge, alias e component ID non cambiano; il profilo risorse è una migrazione workspace compatibile.
- [x] Frontend raccoglie dati, Tauri adatta DTO, planner resta puro, controller/esecutore possiedono processi e query.
- [x] Materializzazione, RuntimeSpec, secret, sicurezza e cancellation hanno confini e test espliciti.
- [x] Nuovo sidecar/dipendenza giustificato in research.md, con impatto MSRV, package e piattaforme.
- [x] Coperti motore, planner, desktop, runner, scheduler, MCP, secret, CI e regressioni affinity.
- [x] Nessun nuovo nodo o connettore; main/reject restano invariati e sono coperti dalla parità.
- [x] UI riusa SettingsModal e design system esistenti; aggiunge solo Runner resources e diagnostica non sensibile.

## Affected Modules and Contracts

| Layer | Paths / types | Change and boundary |
|---|---|---|
| Frontend | frontend/src/workflow-ui/SettingsModal.tsx, frontend/src/tauri-bridge.ts, frontend/src/workspace.ts | Profilo completo atomico, DTO/eventi additivi e UI esistente. |
| Tauri | apps/desktop/src/lib.rs, app_settings.rs, engine_manager.rs, build.rs | Controller condiviso, bridge sottile, staging sidecar; capability/CSP non si ampliano. |
| Planner | crates/duckdb-engine/src/plan/mod.rs, graph.rs, builders.rs, specs.rs, affinity.rs | Eliminare affinity; preservare Stage, ordine, batch e RuntimeSpec. |
| Executor | crates/duckdb-engine/src/lib.rs, nuovo crates/duckle-db-runner | Introdurre WorkerPoolControl, LocalProcessProvider, RunSession e RunDatabase. |
| Runner/MCP/Scheduler | crates/duckle-runner, crates/duckle-mcp/src/tools.rs, crates/scheduler/src/lib.rs | Stesso controller per workspace; rimuovere arg/env CLI e run_lock web seriale. |
| Persistence | app_settings.rs, history.rs, run_log.rs, watermark.rs | Profilo versionato; history/events compatibili e sanitizzati. |

## Design

### Domain and JSON contracts

PipelineDoc non cambia. RunnerResourcesProfile, persistito in .duckle/settings.json, contiene versione, memoria, CPU thread, spill, quack_parallelism e base_capacity. L'assenza dei nuovi campi equivale ad automatico e base 3; memory_limit_mb legacy resta memoria assoluta. Il profilo non contiene endpoint, token, PID o path.

Ogni run emette una sola AcquireRequest verso WorkerPoolControl. Il controller è l'unico owner di provisioning/lease: run e orchestratore non possono scegliere direttamente worker warm o on-demand. Stati e relazioni sono definiti in [data-model.md](data-model.md).

### Planning and execution

Il planner continua a produrre SQL, Stage, RuntimeSpec, alias e decisioni batch/per-stage. Rimuovere plan/affinity.rs, campi affinity in Stage e execute_affinity_worker. Query Source usa catalogo e setup server-side del sidecar per-run.

DuckdbEngine delega il per-run a RunDatabase: WorkerPoolControl acquisisce il worker, RunSession possiede lease/cancel/profile e il client invia SQL completo o batch multi-statement Quack. TEMP/SET restano nella richiesta già batched dall'orchestratore; materializzazione, preview, runtime e trasferimenti ottengono un adapter al posto del solo db_path. SQL remoto, trasferimento Quack e Parquet usano una decision table versionata, misurata per volume, consumer, retry e capacità runtime; ogni ramo è verificato contro benchmark riproducibili.

Tick 5 s: target max(base, ceil(picco_5m × 1,20)). Il picco conta tutte le run, warm e on-demand. I worker starting evitano duplicati; scale-in termina soltanto ready; leased terminano normalmente e non sono rimpiazzati se in eccesso. Restart dalla sola base.

### Frontend and IPC

SettingsModal invia settings_set_runner_resources con l'intero profilo, evitando setter sequenziali e stati misti. settings_get_runner_resources restituisce richiesto, effettivo e diagnostica non sensibile. Ready applica la nuova generazione, starting non pubblica una generazione vecchia, leased drena query attive e poi applica atomicamente l'ultima versione; failure produce configuration_apply_failed per le nuove query. Contratti: [runner-resources-ipc.md](contracts/runner-resources-ipc.md) e [runner-pool-contract.md](contracts/runner-pool-contract.md).

### Secrets, security, and operations

LocalProcessProvider avvia il sidecar solo su loopback con endpoint opaco, pipe anonime ereditate/handle allowlist e credenziale casuale per worker. Readiness richiede l'applicazione integrale del profilo effettivo, poi handshake identità, protocollo, versione e health. I worker espongono metriche sanitizzate di memoria e spill correnti/di picco; spill bounded e disk-full sono gate di integrazione. Token non entrano in argv, environment, file ready, IPC, history o log. Process group/Job Object e sweeper per run ID gestiscono cancel, parent death e orfani. Non aumentare capability Tauri/CSP.

Pinare coppia DuckDB/Quack e distribuirla offline con checksum/version verification; Quack sperimentale resta dietro wrapper interno. I log Quack grezzi non sono esposti quando contengono SQL o endpoint: pubblicare solo eventi Duckle redatti.

### Migration and rollout

Prima del codice, allineare ADR e feature intent alla spec: controller obbligatorio, nessuna queue/budget worker, on-demand deciso dal controller, picco 5 minuti e 20% headroom. Rinominare lo spike in spikes/quack-sidecar-phase0-spike e mantenerlo solo come PoC.

Introdurre un adapter di compatibilità e un gate di selezione non predefinito: il runner ufficiale può essere esercitato da test e compatibilità, ma nessun entry point produttivo lo usa finché bootstrap sicuro, package offline, versione, containment, redazione, benchmark e parità non sono approvati. Prima del gate, ogni finding rilevante della checklist di qualità deve essere risolto oppure accettato esplicitamente con motivazione. Inventariare e migrare anche inspect, drift, branch/diff e CI. Il percorso affinity-free può essere sviluppato e provato dietro compatibilità, ma affinity resta disponibile fino al cutover. Dopo i gate, un solo cutover rimuove CLI, DUCKLE_DUCKDB_BIN, argomenti --duckdb, affinity, download/setup CLI e spike. Desktop, headless, scheduler, MCP e artifact usano la stessa coppia runner/estensione offline. SlothDB e xf.dbt restano leggibili ma disabilitati.

### Operational contract and evidence gate

Il selector riceve un entry-point class (`production`, `test`, `compatibility`
o `release-ci`) e la decisione di gate firmata dal release approver. Prima del
cutover soltanto test e compatibility possono scegliere Quack; production e
release-ci restano sul backend CLI/Affinity. Dopo il cutover non esiste fallback
silenzioso alla CLI: profilo non risolvibile e bundle non verificato restituiscono
rispettivamente `invalid_profile` e `runner_unavailable` sanitizzati.

Il manifest di cutover è l'unica evidenza approvabile: identifica technical
owner/release approver, risultati SC applicabili, finding risolti o accettati,
versioni/checksum/licenze DuckDB-Quack, benchmark congelato e deroghe motivate.
Il worker espone metriche sanitizzate per run/history (memoria/spill in byte,
CPU ms, righe/byte/durata/trasporto) a inizio/fine richiesta e ogni 5 secondi;
non conserva un archivio runner separato. `shutdown/cancel` prevale su crash,
release, apply profile e scale; il controller serializza tali transizioni per
worker e non pubblica un worker starting con profilo superato.

## Constitution Check — Post-Design

- [x] Il design non modifica il grafo persistito e rende compatibile la sola nuova impostazione workspace.
- [x] RunDatabase isola il backend senza trasferire decisioni planner a frontend, provider o sidecar.
- [x] Secret e dettagli di processo non attraversano IPC, history o scheduler; spawn e cleanup sono espliciti.
- [x] Package, MSRV, pin Quack, test multipiattaforma e rimozione CLI sono release gate espliciti.
- [x] I test separano comportamento engine/planner, IPC/UI, packaging e redaction.

## Test Plan

- Unit: transizioni worker, lease atomica, 100 richieste senza ready, tick 5 s, finestra 5 min, formula, restart, scale-in solo ready, precedenze shutdown/apply/scale, profilo drain-safe e decision table di trasferimento.
- Integration: bootstrap/token/version mismatch, applicazione del profilo prima della readiness, metriche memory/spill current+peak, spill bounded/disk-full, SQL/setup server-side, 2/4/8 query, preview, partial run, runtime/materializzazione, cancel/crash/orphan cleanup entro 10 s, isolamento run e assenza di fallback CLI post-cutover.
- Entry point: desktop manual/partial, runner CLI/web, scheduler, MCP, inspect, drift e branch/diff passano tutti dal controller; sostituire run_lock web e verificare il profilo effettivo identico per ogni entry point.
- Frontend/IPC: persistence temp-workspace, save atomico, coalescing/apply failure e controllo visuale Settings.
- Security/package: secret canary assente da argv/env/file/log/event/history; smoke offline Windows/macOS/Linux; capability invariata.
- Benchmark/cutover: manifest congelato prima della misura con hardware/build/dataset/seed/warm-up/ripetizioni/soglia per workload, baseline CLI sullo stesso hardware per SQL remoto/Quack/Parquet e consumer 1/2/4/8; scansione zero riferimenti produttivi a CLI, affinity e spike.

Comandi: cargo fmt --all --check; cargo clippy --workspace --all-targets --exclude duckle-lance; cargo test --workspace --exclude duckle-lance; npm --prefix frontend ci; npm --prefix frontend run lint; npm --prefix frontend run build. Per gate feature completo: clippy all-features e cargo test --workspace quando disponibili.

## Dependency and ADR Decision

**New dependency/plugin/extension/sidecar?** Sì: duckle-db-runner, DuckDB embedded e Quack. Risolve isolamento per-run, cleanup e catalogo condiviso. Alternative scartate: CLI affinity, DuckDB nel main, REST/JSON e Parquet-only. Costi: MSRV 1.88 (per lockfile workspace), binario più grande, pin/package offline e test tre OS.

**ADR needed?** Sì: aggiornare [ADR Quack](../../docs/architecture/adr-quack-sidecar-runner.md) e intent 003 prima dell'implementazione; l'ADR affinity è superseduto al cutover.
