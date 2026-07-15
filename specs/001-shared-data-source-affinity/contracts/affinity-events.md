# Affinity and IPC contracts (proposti)

Comandi:

- `data_source_test`: input workspace/run DTO e Data Source id; output compatibilità, alias, estensioni e diagnostica sanitizzata.
- `query_source_preview`: input pipeline/node e risoluzioni effimere; output schema, righe limitate, durata e affinity context id.

Eventi serializzabili, replicati dal bridge web/SSE:

`affinity_context_started {contextId, querySourceIds, dataSourceIds}`, `data_source_attached {contextId, dataSourceId, alias, durationMs}`, `query_source_finished {contextId, nodeId, status, materializedRelation}`, `affinity_context_finished {contextId, status, durationMs}`.

Gli eventi riportano solo identificatori, alias e messaggi sanitizzati. Ogni evento include `schemaVersion`, `runId`, `contextId`, `sequence`, `timestamp` e `status`. Gli eventi dello stesso run/context sono ordinati per `sequence`; i consumer tollerano duplicati e ignorano versioni future non riconosciute.

Gli errori usano l’envelope `{ code, message, retryable, nodeId?, dataSourceId?, sanitized: true }`. La perdita di un evento non modifica lo stato autorevole della run: il client può ricostruire lo stato dal risultato finale o da una richiesta diagnostica.

Il contratto deve restare compatibile con gli eventi attuali di `run_pipeline` e `run_pipeline_partial`; l’implementazione deve definire framing stdout, cancellation e cleanup del worker CLI.
