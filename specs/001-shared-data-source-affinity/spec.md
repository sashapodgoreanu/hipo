# Feature Specification: Data Source condivisi e Query Source con affinità DuckDB

**Feature Branch**: `[001-shared-data-source-affinity]`
**Created**: 2026-07-15
**Status**: Draft
**Input**: Data Source condivisi e Query Source con affinità di esecuzione DuckDB

## Current State and Scope

**Implemented baseline**: Duckle dispone di Connection come payload del workspace,
Source configurati nei nodi, planner Rust basato su `PipelineDoc`, executor DuckDB
CLI e materializzazione degli intermedi nel database temporaneo di una run. Non
esiste ancora un tipo persistibile Data Source né un componente `src.query`.

**Requested change**: introdurre Data Source riutilizzabili a livello workspace e
Query Source SQL che li referenzino tramite identificatori stabili, garantendo che
Query Source che condividono almeno un Data Source usino lo stesso contesto e la
stessa sessione DuckDB durante una singola run.

**Out of scope**: migrazione automatica dei Source esistenti, scrittura remota
tramite Query Source, transazioni distribuite, caching tra run, sessioni condivise
tra run differenti e supporto iniziale a connector non rappresentabili come
cataloghi collegabili a DuckDB.

**Behavior to preserve**: tutte le pipeline esistenti e gli attuali Source devono
continuare a funzionare senza configurare Data Source.

## Clarifications

### Session 2026-07-15

- Q: Come devono essere persistiti i Data Source? → A: Estendere `RepoItem` con
  il tipo `data_source` e un payload dedicato.
- Q: Come deve comportarsi il rename di un alias usato? → A: Dopo conferma
  esplicita, aggiornare automaticamente il testo SQL delle Query Source
  dipendenti.
- Q: Come deve comportarsi l’eliminazione di un Data Source usato? → A: Consentire
  l’eliminazione solo dopo conferma esplicita, mostrando le dipendenze e marcando
  le Query Source come non valide.
- Q: Cosa deve accadere se una Query Source fallisce? → A: Fallire la Query
  Source e i downstream dipendenti, consentendo ai rami indipendenti dello stesso
  contesto di proseguire.
- Q: Quale SQL può eseguire una Query Source? → A: Solo SQL di lettura (`SELECT`,
  `WITH` e funzioni/tabella DuckDB), senza DDL, DML o statement multipli.

La persistenza dei Data Source riutilizza il modello workspace esistente,
separandoli logicamente da Connection e pipeline senza introdurre un registry
parallelo.

## User Scenarios and Acceptance

### User Story 1 - Gestire Data Source condivisi (Priority: P1)

**Why this priority**: una risorsa catalogo configurata una volta deve poter essere
riutilizzata senza duplicare credenziali o parametri tecnici.

**Independent test**: da una Connection esistente, l’utente crea, modifica,
duplica, verifica e visualizza un Data Source nel workspace.

1. **Given** una Connection valida, **When** l’utente crea un Data Source
   compatibile, **Then** viene salvato separatamente dalla Connection e dalla
   pipeline.
2. **Given** un alias SQL già usato, **When** l’utente prova a salvarne uno
   equivalente senza distinzione maiuscole/minuscole, **Then** il salvataggio è
   rifiutato con un errore comprensibile.
3. **Given** un Data Source usato da Query Source, **When** viene eliminato,
   **Then** richiede conferma esplicita, mostra le dipendenze e marca le Query
   Source dipendenti come non valide dopo l’eliminazione.

### User Story 2 - Creare una Query Source (Priority: P1)

**Why this priority**: l’utente deve poter scrivere SQL DuckDB usando alias
catalogo stabili, senza copiare credenziali nei nodi.

**Independent test**: l’utente inserisce una Query Source nel canvas, seleziona
uno o più Data Source, scrive SQL, esegue preview e collega il risultato a un
Transform o Sink.

1. **Given** Data Source disponibili, **When** l’utente seleziona Data Source e
   scrive SQL, **Then** l’editor mostra gli alias e salva solo i riferimenti.
2. **Given** una query valida, **When** l’utente richiede la preview, **Then**
   mostra schema, righe, durata e contesto senza credenziali.
3. **Given** un riferimento inesistente o SQL non valido, **When** la Query
   Source viene validata o eseguita, **Then** l’errore identifica la causa senza
   contenere segreti.

### User Story 3 - Eseguire Query Source con affinità (Priority: P1)

**Why this priority**: il catalogo collegato deve restare disponibile per tutta
la sessione; il solo riuso del comando `ATTACH` non è sufficiente.

**Independent test**: una pipeline con più Query Source viene eseguita e il
report indica Data Source e Query Source di ciascun contesto.

1. Due Query Source che referenziano `SALES` usano lo stesso contesto e la stessa
   sessione DuckDB; `SALES` viene collegato una sola volta.
2. Query A (`SALES + CUSTOMERS`) e Query B (`CUSTOMERS + ANALYTICS`) condividono
   un unico contesto per affinità transitiva.
3. Query Source indipendenti senza Data Source condivisi possono usare contesti
   distinti.
4. Source driver-based, REST, Kafka o control flow in altri rami non spezzano il
   contesto delle Query Source che condividono Data Source.

### User Story 4 - Errore, cancellazione e sicurezza (Priority: P1)

1. Un errore di estensione o `ATTACH` impedisce l’esecuzione delle Query Source
   dipendenti e identifica il Data Source coinvolto.
2. Password e stringhe di connessione sono rimosse dagli errori mostrati e
   salvati.
3. La cancellazione chiude sessioni, attachment, secret temporanei, database e
   file intermedi secondo la politica di cleanup della run.

## Domain and Contract Impact

| Area | Affected? | Current owner / file | Required compatibility behavior |
|---|---:|---|---|
| Pipeline / PipelineDoc | Yes | `crates/metadata/src/lib.rs`, `crates/duckdb-engine/src/plan/mod.rs` | Aggiungere proprietà senza invalidare pipeline esistenti. |
| Node / component ID | Yes | `frontend/src/pipeline-types.ts`, palette/manifests | Nuovo ID `src.query`; ID esistenti invariati. |
| Data Source | New | `RepoItemType`/`RepoPayload` in `frontend/src/repo-types.ts` da estendere | Nuovo item `data_source` con payload dedicato, id immutabile, alias univoco e rename propagato alle Query Source. |
| Connection | Yes | `repo-types.ts`, `workspace.ts`, `apps/desktop/src/secrets.rs` | Credenziali solo nella Connection cifrata. |
| Context / Secrets | Yes | `frontend/src/run-resolve.ts`, `crates/duckdb-engine/src/context.rs` | Precedenza e masking esistenti preservati. |
| Planner / affinity | New | `crates/duckdb-engine/src/plan/` | Gruppi sulle componenti connesse del sottografo eseguito. |
| Stage / RuntimeSpec | Yes | `plan/mod.rs`, `plan/specs.rs`, `duckdb-engine/src/lib.rs` | Query Source materializza una relazione e mantiene la sessione necessaria. |
| Tauri IPC / web bridge | Yes | `apps/desktop/src/lib.rs`, `frontend/src/tauri-bridge.ts` | DTO, preview, eventi e diagnostica serializzabili e mascherati. |
| Workspace persistence | Yes | `frontend/src/workspace.ts`, history/state | Nessuna migrazione automatica dei Source esistenti. |

## Functional Requirements

- **FR-001**: Il workspace supporta una risorsa `data_source` distinta da
  Connection e pipeline, persistita tramite il modello `RepoItem` esistente.
- **FR-002**: Un Data Source contiene id immutabile, nome, alias SQL, tipo,
  Connection reference, read-only predefinito, catalogo/schema opzionali e opzioni
  specifiche.
- **FR-003**: L’alias SQL è univoco senza distinzione maiuscole/minuscole, valido
  per DuckDB e stabile nel tempo.
- **FR-004**: La modifica di un alias usato richiede conferma esplicita, mostra le
  Query Source dipendenti e aggiorna automaticamente il loro testo SQL.
- **FR-004a**: L’eliminazione di un Data Source usato da Query Source dipendenti
  richiede conferma esplicita, mostra le dipendenze e marca le Query Source come
  non valide dopo l’eliminazione.
- **FR-005**: Il sistema verifica la compatibilità tra Data Source e Connection.
- **FR-006**: Le credenziali non sono copiate in Data Source o Query Source.
- **FR-007**: La palette offre `src.query` con selezione multipla, editor SQL,
  alias, preview, schema, durata, cancellazione ed errori.
- **FR-008**: Query Source salva `dataSourceRefs`, SQL, limite preview, schema
  rilevato e impostazioni supportate.
- **FR-009**: Riferimenti mancanti, alias duplicati, Connection mancanti/non
  compatibili, tipo non supportato, estensione/ATTACH falliti e SQL non valido
  sono rifiutati prima dell’esecuzione dipendente.
- **FR-010**: Prima del run si analizzano solo sottografo, Data Source,
  Connection, estensioni e gruppi realmente necessari.
- **FR-011**: Query Source che condividono almeno un Data Source usano lo stesso
  contesto e la stessa sessione DuckDB durante la singola run.
- **FR-012**: L’affinità è transitiva sul grafo bipartito Query Source ↔ Data Source.
- **FR-013**: Ogni Data Source viene collegato al massimo una volta nel contesto.
- **FR-014**: Ogni Query Source materializza il risultato nel database temporaneo
  e lo rende disponibile ai nodi downstream.
- **FR-015**: Il runtime rispetta DAG e dipendenze senza richiedere Query Source
  consecutive nel canvas.
- **FR-016**: Retry, wait, control flow e stage esterni non spostano Query Source
  affini in sessioni differenti.
- **FR-016a**: Se un retry, wait, control flow o RuntimeSpec attraversa un
  affinity group, il planner deve dichiarare se la sessione viene
  sospesa/ripresa oppure se il gruppo è non supportato; non è ammesso degradare
  silenziosamente a sessioni differenti.
- **FR-017**: Un’esecuzione parziale considera solo il sottografo selezionato.
- **FR-018**: La diagnostica espone contesto, Data Source, Query Source,
  estensioni, attachment, durate, stato ed errori sanitizzati.
- **FR-019**: Cancellazione e cleanup non lasciano processi o file temporanei della
  run.
- **FR-020**: Il primo rilascio supporta DuckDB, SQLite, PostgreSQL, MySQL/MariaDB
  e DuckLake quando compatibili con le estensioni disponibili; gli altri Source
  restano invariati.
- **FR-021**: Un errore di Query Source marca la Query Source e i downstream
  dipendenti come falliti, ma consente ai rami indipendenti dello stesso contesto
  di proseguire secondo il DAG.
- **FR-022**: Query Source accetta solo SQL di lettura (`SELECT`, `WITH` e
  funzioni/tabella DuckDB); DDL, DML e statement multipli vengono rifiutati.

## Execution and Security Impact

- **Graph/planner**: raggruppare le componenti connesse del sottografo; non
  trattare automaticamente ogni trigger UI come data-edge.
- **Execution**: l’engine attuale usa spesso processi per-stage; la feature
  richiede una sessione persistente per gruppo. Il piano dovrà scegliere worker
  persistente, sessione multi-statement o soluzione equivalente.
- **Error policy**: un errore di inizializzazione del contesto impedisce le Query
  Source che dipendono da quel contesto; un errore di query si propaga al suo
  sottografo downstream senza bloccare rami indipendenti.
- **Connections/secrets**: Data Source contiene solo il riferimento; statement
  `ATTACH`/`CREATE SECRET`, log, history ed errori devono essere mascherati.
- **Query SQL**: la Query Source è read-only a livello di linguaggio; non può
  modificare Data Source remoti né introdurre side effect DDL/DML.
- **IPC**: servono DTO per Data Source, preview, contesto e diagnostica, oltre a
  eventi di inizializzazione, esecuzione e cancellazione.
- **Security**: read-only predefinito, validazione Connection, estensioni,
  filesystem e process spawning devono essere riesaminati.
- **Multiplatform**: mantenere compatibilità Windows, macOS e Linux.

## Compatibility and Migration

**Serialized format changed?** Sì: nuovi repository item Data Source e proprietà
per `src.query`; il formato dei Source esistenti non viene riscritto.

**Migration / fallback**: nessuna migrazione automatica dei Source legacy.
Pipeline esistenti continuano a usare i Source attuali; un rename confermato di
un alias aggiorna invece il SQL delle Query Source dipendenti. Riferimenti Data
Source non risolvibili producono errore prima del run.

**Existing component/pipeline behavior**: il modello è additivo e non converte
automaticamente i Source esistenti.

## Acceptance Criteria

- [ ] Data Source possono essere creati, modificati, duplicati e verificati; la
  rimozione con dipendenze richiede conferma e rende visibili le Query Source non
  valide.
- [ ] Alias `sales` e `SALES` non coesistono nello stesso workspace.
- [ ] Una Query Source usa uno o più Data Source e produce una relazione
  downstream.
- [ ] Query Source che condividono un Data Source risultano nella stessa
  sessione, anche quando sono separati da stage intermedi applicabili.
- [ ] L’affinità transitiva produce un solo gruppo per riferimenti condivisi.
- [ ] Errori di Connection, estensione, `ATTACH` e SQL non espongono credenziali.
- [ ] Un errore di Query Source non blocca rami indipendenti, ma marca falliti la
  Query Source e i relativi downstream.
- [ ] DDL, DML e statement multipli vengono rifiutati da una Query Source.
- [ ] Cancellazione e cleanup non lasciano processi o file della run.
- [ ] Pipeline e Source esistenti continuano a funzionare.

## Success Criteria

- Per ogni gruppo di Query Source condivise, una sola sessione è usata e ogni
  Data Source viene collegato una sola volta.
- Il 100% dei riferimenti mancanti, alias duplicati e Connection incompatibili
  viene rifiutato prima dell’esecuzione dipendente.
- Il 100% degli errori di test contenenti credenziali note è privo del valore
  sensibile in UI e history.
- Query Source valide rendono il risultato disponibile ai nodi downstream nella
  stessa run anche con stage non SQL in rami differenti.
- Le pipeline esistenti non richiedono modifiche per continuare a funzionare.

## Assumptions, Gaps, and Decisions

- **Confirmed fact**: Connection sono repository item separati e i campi sensibili
  sono gestiti da `apps/desktop/src/secrets.rs`.
- **Confirmed fact**: planner ed engine usano `Stage`/`RuntimeSpec`, DuckDB CLI,
  database temporaneo e percorsi batch/per-stage.
- **Gap**: non esistono Data Source, Query Source, gruppi di affinità o sessione
  DuckDB persistente condivisa tra più stage.
- **Gap**: non esiste un tipo unico per Component, Engine o StageResult.
- **Decisione necessaria nel plan**: modalità concreta della sessione condivisa e
  coordinamento con stage intermedi esterni.
