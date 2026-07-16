# Execution model

> Il flusso seguente descrive il comportamento osservato, incluse le
> differenze tra run desktop, run parziale e runner headless.

## Flusso principale

```mermaid
flowchart TD
  JSON[Pipeline JSON / React Flow nodes + edges]
  RESOLVE[Context resolution: workspace, contexts, routines, env placeholders]
  VALIDATE[Graph and alias validation]
  ORDER[Data-edge topological ordering]
  LOWER[Node -> Stage]
  MODE{Stage pure SQL?}
  SPEC[RuntimeSpec dispatch]
  BATCH[Single batched DuckDB CLI session]
  PERSTAGE[Per-stage DuckDB CLI processes]
  TEMP[(Per-run temporary .duckdb)]
  MAT[Views / tables / Parquet / external sink materialization]
  PREVIEW[NodePreview and PipelineEvent]
  SINK[Sink side effect]
  HISTORY[RunRecord, logs, watermark/state]
  CLEAN[Cleanup and final RunResult]

  JSON --> RESOLVE --> VALIDATE --> ORDER --> LOWER --> MODE
  MODE -- yes and all stages eligible --> BATCH
  MODE -- no or partial run --> PERSTAGE
  MODE -- runtime/control --> SPEC --> PERSTAGE
  BATCH --> TEMP
  PERSTAGE --> TEMP
  TEMP --> MAT --> PREVIEW
  MAT --> SINK --> HISTORY --> CLEAN
  PREVIEW --> HISTORY
```

## Desktop sequence

```mermaid
sequenceDiagram
  participant UI as React UI
  participant B as tauri-bridge.ts
  participant T as Tauri command
  participant C as context resolver
  participant P as planner
  participant E as DuckdbEngine
  participant D as DuckDB CLI
  participant W as workspace state

  UI->>B: runPipeline(PipelineDoc, options, onEvent)
  B->>T: invoke run_pipeline + Channel
  T->>C: apply environment / received resolved values
  T->>P: execute_pipeline_with_events
  P->>P: graph validation, ordering, Node -> Stage
  P-->>E: CompiledPipeline
  E->>D: batched or per-stage SQL/process
  D-->>E: rows, errors, output files
  E-->>T: PipelineEvent / RunResult
  T->>W: append run history when workspace and pipeline id exist
  T-->>B: serialized result
  B-->>UI: stage status, preview, errors
```

`run_pipeline_partial` performs a backward traversal along data edges and
compiles only the upstream subgraph. Partial runs do not use the batched path;
the target becomes a leaf for preview.

## Planner and stage lifecycle

```mermaid
stateDiagram-v2
  [*] --> Created
  Created --> Validated: compile()
  Validated --> Ordered: graph + alias checks
  Ordered --> Compiled: Stage built
  Compiled --> Waiting: wait/throttle
  Waiting --> Running
  Compiled --> Running: no wait
  Running --> RuntimeDispatch: RuntimeSpec present
  Running --> SQLExecution: runtime absent
  RuntimeDispatch --> SQLExecution: pass-through/control where applicable
  RuntimeDispatch --> Succeeded
  SQLExecution --> Materialized
  Materialized --> Previewed
  Previewed --> Succeeded
  Running --> Retrying: engine error and attempts remain
  Retrying --> Running
  Running --> Cancelled: cancellation flag
  Running --> Failed: config/query/runtime error
  Succeeded --> [*]
  Failed --> [*]
  Cancelled --> [*]
```

`Stage::is_pure_sql()` is true only when `runtime` is absent and the component
does not need a Rust post-write hook such as the current Excel path. The
executor batches only when the full pipeline is eligible: no target partial
run, no retries/waits/memory overrides, and no runtime/sink guard that forces
per-stage execution.

## Materialization

The engine opens a temporary on-disk DuckDB database for a run. Intermediate
relations may be represented as temporary tables, views or temporary Parquet
depending on consumer count, source/attach behavior, rejection/reuse paths,
and whether the run is batched or partial. A source configured for a live view
can be upgraded only in the compatible single-session path; partial runs keep
materialized tables across separate CLI processes.

Sinks produce external effects through `COPY`, connector drivers, HTTP, files,
or other runtime implementations. Existing-target checks, retries, cleanup,
row counts and preview are part of observable behavior.

## Connection, context and secret resolution

```mermaid
flowchart LR
  Repo[workspace repository items]
  Conn[ConnectionPayload]
  Ctx[ContextPayload / routines]
  Env[Process environment / ENV placeholders]
  Enc[Encrypted sensitive fields]
  Resolve[frontend run-resolve.ts + Rust context.rs]
  Props[Node properties / PipelineDoc]
  Mask[secret values retained for redaction]

  Repo --> Conn --> Enc --> Resolve
  Repo --> Ctx --> Resolve
  Env --> Resolve
  Resolve --> Props
  Resolve --> Mask
```

The frontend resolves workspace/context/date-style values before a canvas run;
the desktop command applies environment variables. The headless runner and
scheduler use the Rust context resolver. Secret values are carried to the
engine for connector use and are tracked for masking; the encryption service
stores sensitive connection fields at rest.

## Error, cancellation and history

`EngineError` categorizes configuration, unsupported, query and cancellation
failures. Tauri command errors are generally serialized as `String`. The
current run owns an atomic cancellation flag; `cancel_pipeline` requests
termination of the active DuckDB child process. Stage events are streamed via
`Channel<PipelineEvent>`, while history is appended as `RunRecord` under the
workspace when identifiers are provided.

## Confirmed gaps

- Materialization policy is encoded across planner and executor rather than a
  standalone materialization type.
- The Tauri adapter still contains orchestration in `lib.rs`.
- Frontend/E2E execution coverage is not detected; Rust engine tests dominate.

## Query Source preview and Data Source affinity

La preview di `src.query` costruisce un singolo processo DuckDB con gli
`ATTACH` temporanei dei Data Source risolti dal workspace, esegue un solo
statement read-only e restituisce schema, massimo 1000 righe, durata e
`contextId`. Il limite è 30 secondi; timeout ed errori restituiscono codici
sanitizzati senza dettagli di connessione.

Una pipeline interamente SQL che contiene `src.query` usa il worker CLI
persistente descritto in `docs/architecture/adr-affinity-session.md`: ciascun
alias Data Source viene collegato una sola volta nel worker, ogni Query Source
materializza una `TABLE` nel run database e Join/Sink downstream leggono tale
relazione senza dipendere dal catalogo esterno. Count, schema e preview sono
letti nello stesso processo, perché il worker possiede il lock del run-db.
Pipeline con runtime/control o altri confini non compatibili mantengono il
percorso per-stage esistente finché lo scheduler di compatibilità per gruppi
non viene completato.
