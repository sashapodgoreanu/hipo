# Domain model

> Stato: solo fatti osservati. Quando non esiste un tipo centrale, la tabella
> lo dichiara esplicitamente invece di introdurlo come astrazione implementata.

## Pipeline e grafo

| Oggetto | Owner autorevole | Controparte frontend | Invarianti/lifecycle |
|---|---|---|---|
| `Pipeline` | `crates/metadata/src/lib.rs` | Repository item `pipeline` in `workspace.ts`; il canvas usa nodi/archi React Flow | `id`, `name`, `version`, `nodes`, `edges`; JSON persistibile, creato e aggiornato nel workspace. |
| `PipelineDoc` | `crates/duckdb-engine/src/plan/mod.rs` | Payload di run compilato dal bridge; nessun alias TS dedicato | Solo `nodes` + `edges`; input tecnico del planner, non piano eseguibile. |
| `PipelineNode` | `duckle_metadata::PipelineNode` | `DuckleNodeData` + tipo React Flow | `id`, `position`, `flow_type`, `NodeData`; l’id del nodo è distinto dall’alias SQL. |
| `NodeData` | `duckle_metadata::NodeData` | `DuckleNodeData` | `label`, `componentId`, `properties`, schema/sample opzionali, `disabled`, `alias`. |
| `PipelineEdge` | `duckle_metadata::PipelineEdge` | Edge React Flow | `source`, `target`, handle e `connectionType`; il planner considera solo i data-edge riconosciuti. |
| `EdgeData` | `duckle_metadata::EdgeData` | Dati edge nel canvas | `connectionType`, `label`, `condition`; non tutti gli edge UI sono dipendenze topologiche Rust. |

Il planner costruisce l’ordine dal grafo. Cicli, alias duplicati, riferimenti
mancanti e fan-in non ammessi producono errori di configurazione. `compile_partial`
mantiene il sottografo upstream di un target e rende il target una foglia.

## Schema e risultati

| Oggetto | Owner / tipo | Note |
|---|---|---|
| `Schema` | Rust `type Schema = Vec<Column>` in `metadata/src/lib.rs` | È un alias Rust, non un record autonomo. |
| `Column` | Rust `Column`; TS `Column` in `frontend/src/pipeline-types.ts` | Nome, tipo, nullable, primary key opzionale e formato opzionale. |
| `DataType` | Rust enum `DataType`; TS union `DataType` | I token JSON sono allineati (`string`, `int64`, `geometry`, ecc.). |
| `Run` | Rust `RunResult`/`RunRecord`; TS `RunResult`/`RunRecord` in `tauri-bridge.ts` | Stato complessivo, durata, nodi, preview, errore e messaggi. |
| `StageResult` | Non rilevato come tipo centrale | Il risultato per nodo è `NodeRunStatus`; il risultato complessivo è `RunResult`. |
| `Preview` | Rust `NodePreview`; TS `NodePreview` | Colonne e righe campionate; limite applicato dall’engine. |

## Componenti e categorie

`Component`, `Source`, `Transform`, `Sink` e `Control` non sono un gerarchia
Rust unica. Sono concetti distribuiti:

- `frontend/src/workflow-ui/palette-data.ts` contiene definizioni palette,
  categorie e component ID;
- `frontend/src/workflow-ui/fields/types.ts` contiene `ComponentManifest` e
  descrizioni di proprietà/porte;
- `component-manifests.ts` contiene manifest, schema sample e autodetect;
- `plan/mod.rs` mappa i component ID in SQL o `RuntimeSpec`;
- `duckdb-engine/src/lib.rs` esegue il runtime dispatch.

Il `NodeKind` base frontend è `source | transform | sink`; la palette usa anche
categorie `control`, `quality` e `custom`. Non esiste un tipo Rust `Component`
che imponga centralmente proprietà, preview, retry o cancellation.

## Connection, Context e Secret

| Oggetto | Tipo concreto | Lifecycle/persistenza |
|---|---|---|
| Connection | TS `ConnectionPayload` in `frontend/src/repo-types.ts` | Repository item `connection`; cifratura/decrittazione in `apps/desktop/src/secrets.rs`. |
| Context | TS `ContextPayload`; payload Rust interno in `context.rs` | Item `context`; risolto durante run/build, incluse variabili namespace e routine. |
| Secret | Nessun tipo condiviso unico; campi `secret`/chiavi sensibili + placeholder `${ENV:...}` | Valori sensibili cifrati in workspace; i placeholder non vengono cifrati e vengono risolti dal contesto/runtime. |

La selezione frontend attuale idrata un payload e copia i valori nelle
properties del nodo. Non è confermata una risoluzione runtime basata su un
Connection ID.

## Piano tecnico e servizi

| Oggetto | Tipo concreto | Relazioni |
|---|---|---|
| `Stage` | Rust `plan::Stage` | Deriva da un nodo; contiene SQL, `StageKind`, runtime opzionale, retry, alias e materializzazione implicita. |
| `StageKind` | Rust enum `View | Sink` | Classifica l’output tecnico del planner. |
| `RuntimeSpec` | Rust enum in `plan/mod.rs` con spec in `plan/specs.rs` | Variante per connector, AI, code, dbt, control e side effect. |
| `CompiledPipeline` | Rust `stages` + `leaves` | Piano ordinato usato dall’engine; non dipende dalla WebView. |
| Engine | `DuckdbEngine` concreto in `duckdb-engine/src/lib.rs` | Shell-out al DuckDB CLI, cancellazione e dispatch; nessun trait `Engine` centrale rilevato. |
| Scheduler | Rust `Schedule`, `ScheduleKind`, `Scheduler` | Cron, interval e file-watch; `schedules.json`; esegue pipeline tramite engine. |
| Runner | Crate `duckle-runner` (`main.rs`, `serve.rs`) | CLI/headless e HTTP web; usa context e engine. |
| MCP | Crate `duckle-mcp` (`main.rs`, `tools.rs`) | JSON-RPC 2.0 su stdio e catalogo di tool; non è un tipo di dominio pipeline. |

## Gap e raccomandazioni

- Gap: contratti dei componenti e DTO IPC sono duplicati/distribuiti.
- Gap: manca una forma unica di `StageResult` o `Engine` astratto.
- Raccomandazione: se in futuro si introducono tali astrazioni, documentarle
  come cambiamenti di contratto, non trattarle come tipi già esistenti.
