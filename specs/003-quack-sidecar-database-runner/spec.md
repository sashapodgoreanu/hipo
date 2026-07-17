# Feature Specification: Quack Sidecar Database Runner

**Feature Branch**: `[003-quack-sidecar-database-runner]`
**Created**: 2026-07-18
**Status**: Draft
**Input**: "Creare la feature 003 Quack sidecar database runner usando il contesto di documenti, ADR e report. Il sidecar Quack esistente è solo uno spike: va rinominato e mantenuto temporaneamente per contesto, poi rimosso quando il sidecar ufficiale è implementato. Rimuovere inoltre affinity, che con il nuovo runner non serve più."

## Current State and Scope

**Implemented baseline**: Duckle esegue oggi il lavoro DuckDB tramite la CLI,
con un database temporaneo per run, un percorso batch per pipeline SQL idonee e
percorsi per-stage per runtime, controlli e run parziali. Una parte delle
pipeline Query Source usa inoltre `AffinitySession`, un worker CLI persistente
e seriale. Desktop, runner headless, scheduler, MCP, build artifact, strumenti
di inspect/drift/branch e test conoscono ancora il binario CLI o la sua
configurazione. Lo spike isolato in `spikes/quack-sidecar` ha dimostrato su
Windows x64 il confine parent/client Quack e sidecar/server, lo spill, la
concorrenza stateless, il kill e il prewarm; non appartiene al workspace
produttivo, non soddisfa il bootstrap di sicurezza definitivo e non è il runner
ufficiale. SlothDB e `xf.dbt` sono già disabilitati durante questa migrazione,
pur restando leggibili nei documenti esistenti.

**Requested change**: introdurre il runner DuckDB ufficiale come sidecar
isolato e single-use per ogni pipeline run. Il processo principale conserva
planner, orchestrazione, eventi e runtime esterni, mentre il sidecar possiede
un solo database, catalogo, relazioni, memoria e spill per l'intera run. Tutte
le operazioni DuckDB ordinarie attraversano Quack. Dopo la parità completa, il
nuovo runner sostituisce la CLI e il sottosistema affinity viene eliminato del
tutto.

**Out of scope**: condivisione di un worker fra run; servizio DuckDB persistente;
provider Kubernetes o controller distribuito; endpoint Quack pubblico o
Internet; publication plane e Books; esecuzione distribuita; cancellazione di
un singolo statement mantenendo viva la run; rimozione assoluta del fallback
Parquet; riattivazione/migrazione di SlothDB o dbt; implementazione della Query
multi-input della Feature 002; autorizzazione multi-tenant e sandbox completa
del codice utente.

**Behavior to preserve**: compatibilità di pipeline e workspace persistiti,
component ID, node ID, alias SQL, grafo e run parziali; decisioni esistenti
dell'orchestratore su ordine, batching, parallelismo, retry e attese; eventi di
stage, preview, schema, materializzazioni, sink, history, watermark e stato;
risoluzione di Connection/Context e redazione dei secret; comportamento
desktop, headless, scheduled e MCP salvo la sostituzione esplicita del backend.

**UI continuity (when UI changes)**: Engine Setup, selettore engine, stato di
installazione e diagnostiche riusano controlli, terminologia e stati esistenti.
La UI distingue runner, protocollo ed estensioni disponibili senza introdurre
un secondo flusso di setup. SlothDB e `xf.dbt` restano chiaramente disabilitati.

## User Scenarios and Acceptance

### User Story 1 - Eseguire una pipeline nel database isolato della run (Priority: P1)

**Why this priority**: il sidecar ufficiale deve diventare l'unico proprietario
del database della pipeline e sostituire il confine CLI senza cambiare ciò che
l'utente osserva.

**Independent test**: eseguire la stessa suite rappresentativa di pipeline SQL,
runtime, source, transform, sink, preview e run parziale sul backend corrente e
sul runner ufficiale, confrontando risultati, eventi ed effetti esterni.

1. **Given** una pipeline valida, **When** l'utente avvia una run, **Then** la
   run acquisisce un worker esclusivo, usa un unico catalogo fino al termine e
   restituisce gli stessi risultati ed eventi attesi.
2. **Given** due run concorrenti, anche della stessa pipeline, **When** vengono
   eseguite, **Then** usano worker, cataloghi, credenziali e directory distinti
   senza perdita o contaminazione di dati.
3. **Given** una preview o una run parziale, **When** viene richiesta, **Then**
   usa lo stesso contratto del runner ufficiale e conserva limiti, schema,
   dipendenze e semantica osservabile correnti.

### User Story 2 - Usare catalogo condiviso e concorrenza senza affinity (Priority: P1)

**Why this priority**: un database posseduto per l'intera run rende superfluo il
worker CLI affinity e consente di governare le risorse in modo generale.

**Independent test**: eseguire pipeline con relazioni condivise, Data Source
server-side, batch multi-statement e 2/4/8 richieste concorrenti, verificando
catalogo unico, correttezza e assenza di ogni percorso affinity.

1. **Given** stage dipendenti o Data Source già inizializzate, **When** richieste
   stateless successive vengono eseguite, **Then** le relazioni regolari e gli
   attachment server-side restano disponibili nel catalogo della run.
2. **Given** richieste compatibili già rese concorrenti dall'orchestratore,
   **When** sono disponibili permit, **Then** vengono eseguite in parallelo
   entro il limite configurato senza riordino introdotto dal backend.
3. **Given** stato temporaneo necessario a più statement, **When** lo stage
   viene compilato, **Then** gli statement restano nella stessa richiesta;
   nessuno stato temporaneo o di sessione è richiesto tra due chiamate.
4. **Given** una pipeline un tempo instradata tramite affinity, **When** viene
   eseguita dopo il cutover, **Then** usa il normale runner per-run e non esiste
   classificazione, gruppo, sessione o fallback affinity.

### User Story 3 - Cancellare o perdere un runner in modo deterministico (Priority: P1)

**Why this priority**: l'isolamento di processo deve rendere cancellazione,
crash e cleanup più prevedibili dell'attuale insieme di processi CLI.

**Independent test**: cancellare e terminare forzatamente run durante scan,
join, spill, trasferimento e runtime esterno, quindi controllare stato, tempi e
assenza di processi/file orfani.

1. **Given** una run attiva o in coda, **When** l'utente la cancella, **Then**
   non partono nuovi stage, il worker e i processi associati terminano, le
   richieste interrotte risultano `cancelled` e il cleanup conclude entro il
   budget dichiarato.
2. **Given** un'uscita inattesa del sidecar senza cancellazione, **When** il
   trasporto si interrompe, **Then** la run risulta `runner_crashed` con causa
   sanitizzata e il worker non viene riutilizzato.
3. **Given** la morte dell'orchestratore, **When** il sistema riparte, **Then**
   il contenimento del processo e lo sweeper impediscono worker orfani e
   rimuovono soltanto artefatti temporanei scaduti.

### User Story 4 - Avviare worker pronti e autenticati sotto capacità limitata (Priority: P1)

**Why this priority**: prewarm e coda devono ridurre la latenza senza consentire
doppie assegnazioni, crescita illimitata o pubblicazione prematura.

**Independent test**: saturare capacità base e massima con run concorrenti,
startup lenti e falliti, cancellazioni in coda e scale-in, verificando stati,
fairness, budget e sostituzioni.

1. **Given** capacità pronta, **When** arriva una run, **Then** riceve
   atomicamente un solo worker autenticato con lease esclusivo e single-use.
2. **Given** capacità massima esaurita, **When** arrivano altre run, **Then**
   attendono in una coda FIFO limitata e cancellabile oppure ricevono timeout,
   senza creare worker oltre i limiti.
3. **Given** un worker soltanto in ascolto o con handshake fallito, **When** il
   control plane valuta la readiness, **Then** il worker non diventa
   acquisibile e viene terminato o sostituito secondo policy.
4. **Given** una run conclusa, **When** il lease viene rilasciato, **Then** il
   worker è sempre terminato e un eventuale sostituto diventa pronto soltanto
   dopo un nuovo handshake completo.

### User Story 5 - Proteggere capability e secret del database (Priority: P1)

**Why this priority**: la credenziale Quack concede pieno accesso SQL al worker
e non può attraversare superfici persistenti o codice non trusted.

**Independent test**: usare secret sintetici riconoscibili e fault injection in
bootstrap, query, log, errori, history, profiler, export e cancellazione,
verificando che nessun valore sia divulgato.

1. **Given** un nuovo worker locale, **When** viene avviato, **Then** identità e
   credenziale uniche viaggiano soltanto su canali bootstrap ereditati e non
   tramite argomenti, environment, stdin generico o file.
2. **Given** una porta conosciuta ma token assente, errato o appartenuto a un
   worker precedente, **When** un client tenta l'accesso, **Then**
   l'autenticazione è rifiutata senza divulgare credenziali.
3. **Given** codice utente o runtime esterno, **When** consuma o produce una
   relazione, **Then** usa un trasferimento controllato e non riceve endpoint o
   capability primaria del worker.

### User Story 6 - Distribuire e migrare il runner ufficiale (Priority: P1)

**Why this priority**: il cutover è completo soltanto se desktop, artifact e
strumenti funzionano offline e non dipendono più dalla CLI o dallo spike.

**Independent test**: installare e usare build pulite sui target supportati,
senza rete e senza DuckDB CLI nel sistema, quindi eseguire desktop, headless,
scheduler, MCP, build artifact e strumenti dati.

1. **Given** una build supportata, **When** viene installata o avviata offline,
   **Then** runner ed estensioni compatibili sono disponibili come coppia
   verificata e una pipeline non richiede download della CLI.
2. **Given** una versione client/server o estensione incompatibile, **When** il
   worker completa il bootstrap, **Then** il mismatch blocca l'assegnazione
   prima del primo stage con diagnostica chiara.
3. **Given** che tutti i gate di parità sono superati, **When** avviene il
   cutover, **Then** codice, setup, packaging, variabili e test specifici della
   CLI e tutto il sottosistema affinity sono rimossi.
4. **Given** lo spike Phase 0 rinominato e conservato durante la migrazione,
   **When** il runner ufficiale soddisfa i criteri di accettazione, **Then** lo
   spike viene rimosso e i documenti storici indicano chiaramente che non è un
   componente distribuibile né un fallback.

### Edge Cases

- Una run viene cancellata mentre attende un worker o un permit interno.
- Il sidecar effettua il bind ma non completa autenticazione, versione o health.
- Il bootstrap è troncato, malformato, contiene trailing bytes o una versione
  sconosciuta.
- Il worker termina durante spill, trasferimento, DDL, append o cleanup.
- Il disco di spill raggiunge il limite o lo spazio libero minimo.
- Un batch multi-statement fallisce dopo aver completato statement precedenti.
- Due mutation concorrenti causano un conflitto deterministico.
- Un server setup viene richiesto simultaneamente da più stage.
- Una query produce zero righe, tipi nested, decimal, timestamp o valori null.
- Un runtime richiede una relazione grande con uno o più consumer e il
  trasferimento diretto non è la scelta più conveniente.
- Un output persistente e un artefatto temporaneo convivono nella directory di
  run; il cleanup non deve cancellare l'output richiesto dall'utente.
- Workspace esistenti selezionano SlothDB o contengono `xf.dbt`: restano
  leggibili e falliscono con diagnostica esplicita, senza fallback.
- Antivirus, firewall o porta locale impediscono lo startup: il worker non
  viene pubblicato e la causa è sanitizzata.

## Domain and Contract Impact

| Area | Affected? | Current owner / file | Required compatibility behavior |
|---|---:|---|---|
| Pipeline / PipelineDoc | No | `crates/metadata`, `crates/duckdb-engine/src/plan/` | Nessuna riscrittura o nuova dipendenza dal backend nei documenti persistiti. |
| Node / Edge / handles / alias | No | `metadata`, `frontend/src/pipeline-types.ts` | Node ID, component ID, alias, porte e ordinamento del grafo restano invariati. |
| Component ID / properties / ports | Yes | palette, manifests, planner | Nessun nuovo componente; stati disabilitati di SlothDB/`xf.dbt` restano espliciti e compatibili. |
| Schema / preview / lineage | Yes | metadata, engine | Stessi schema, limiti, lineage e risultati osservabili dopo la migrazione. |
| Connection / context / secrets | Yes | workspace, `secrets.rs`, `context.rs` | Precedenza e placeholder esistenti preservati; capability e secret mai persistiti o divulgati. |
| Stage / RuntimeSpec / materialization | Yes | `plan/mod.rs`, engine | Ordine, batch, retry, eventi, output e side effect preservati; cambia soltanto il backend di esecuzione. |
| Tauri IPC / web bridge | Yes | desktop `lib.rs`, `tauri-bridge.ts` | DTO/eventi compatibili; setup, stato, cancellazione ed errori riflettono il runner ufficiale. |
| Workspace persistence / migration | Yes | `workspace.ts`, engine history/state | Formati leggibili senza migrazione; history conserva semantica e aggiunge solo diagnostica compatibile. |
| Frontend UI / design system | Yes | existing `frontend/src/...` components and styles | Riutilizzare visual language e interazioni esistenti; nessuna regressione non intenzionale. |

## Functional Requirements

- **FR-001**: Ogni pipeline run DEVE possedere esattamente un worker DuckDB
  dedicato, esclusivo e non condiviso con altre run.
- **FR-002**: Il worker DEVE possedere per tutta la run un solo database, il
  catalogo, le relazioni, gli attachment, il budget di memoria e lo spill.
- **FR-003**: Il processo principale DEVE conservare planner, DAG orchestration,
  eventi, history, retry e coordinamento dei runtime esterni, ma NON DEVE
  eseguire join, aggregate, sort o materializzazioni della pipeline né
  conservare dataset completi salvo trasferimenti richiesti esplicitamente.
- **FR-004**: Quack DEVE essere l'unico protocollo dati ordinario fra main e
  sidecar; NON DEVE essere introdotta un'API REST/JSON parallela per query o
  risultati.
- **FR-005**: Il backend DEVE eseguire esattamente la richiesta singola o il
  batch già deciso dall'orchestratore, senza anticipare, fondere, dividere,
  riordinare o parallelizzare autonomamente gli stage.
- **FR-006**: Tutti i chiamanti attivi del motore DEVONO usare un unico
  contratto di database della run per execute, query, schema, count, preview,
  import, export, metriche e cleanup.
- **FR-007**: L'esecuzione ordinaria DEVE essere stateless fra richieste: gli
  output condivisi vivono come relazioni regolari nel catalogo della run;
  `TEMP` e impostazioni necessarie a più statement restano nello stesso batch.
- **FR-008**: Il percorso ordinario NON DEVE creare o dipendere da attachment
  Quack client sticky; le connessioni client grezze e il catalogo client non
  DEVONO essere esposti a planner, componenti o runtime.
- **FR-009**: Il planner/orchestratore DEVE restare autorità per setup e
  dipendenze delle Data Source; il setup server-side DEVE essere applicato una
  volta per identità/alias della risorsa e completare prima degli stage
  dipendenti.
- **FR-010**: Ogni worker DEVE limitare le richieste Quack concorrenti tramite
  un gate FIFO e cancellabile, configurabile da 1 a 8 e con default/massimo
  iniziale pari a 8; l'attesa NON DEVE creare connessioni o worker aggiuntivi.
- **FR-011**: Le richieste compatibili già dichiarate concorrenti
  dall'orchestratore DEVONO poter procedere in parallelo; conflitti di mutation
  DEVONO produrre retry limitati o errori deterministici secondo la policy
  dell'orchestratore.
- **FR-012**: Il sottosistema affinity DEVE essere eliminato dopo il cutover,
  inclusi sessione, gruppi, classificazioni per component ID, marker, routing,
  fallback, test e documentazione operativa; nessun dato persistito DEVE
  richiederne una migrazione.
- **FR-013**: La rimozione di affinity NON DEVE rimuovere la normale descrizione
  di dipendenze, modalità di accesso, setup o limiti di concorrenza usata dal
  planner e dall'orchestratore.
- **FR-014**: Il runner ufficiale DEVE preservare risultati, row count, schema,
  preview, materializzazione, sink, retry, wait, eventi, history, watermark,
  run parziali e propagazione degli errori delle pipeline supportate.
- **FR-015**: Runtime, connector e strumenti che oggi leggono il database o
  usano helper CLI DEVONO essere migrati a SQL remoto, trasferimento Quack
  controllato o snapshot Parquet prima della rimozione della CLI.
- **FR-016**: La scelta fra SQL remoto, trasferimento Quack e snapshot Parquet
  DEVE dipendere da una policy misurata per volume, consumer, retry e capacità
  del runtime, non dal solo component ID.
- **FR-017**: La cancellazione di uno stage attivo DEVE cancellare l'intera run,
  impedire nuovi stage, terminare worker e processi associati e classificare le
  interruzioni conseguenti come `cancelled`.
- **FR-018**: Un'uscita inattesa del sidecar senza cancellazione DEVE produrre
  `runner_crashed`, con exit code e causa sanitizzata quando disponibili.
- **FR-019**: Completion, cancellazione, crash e shutdown concorrenti DEVONO
  usare terminazione e cleanup idempotenti; spill, bootstrap e snapshot interni
  DEVONO essere rimossi entro 10 secondi, senza eliminare output persistenti.
- **FR-020**: Il worker locale e i runtime associati DEVONO essere contenuti in
  un process scope che impedisca orfani alla morte del parent; residui di crash
  DEVONO essere gestiti da uno sweeper con run ID e TTL.
- **FR-021**: Il worker DEVE applicare budget espliciti di memoria, CPU, spill e
  spazio temporaneo prima della readiness e DEVE riportarne uso corrente e
  picco separatamente dal main e dai runtime esterni.
- **FR-022**: Un workload oltre il budget di memoria ma entro quello disco DEVE
  completare tramite spill bounded; disco esaurito o spazio insufficiente
  DEVONO produrre un errore specifico e sanitizzato.
- **FR-023**: Ogni worker DEVE avere identità e capability casuale uniche,
  limitate alla sua vita e non riutilizzate da sostituti.
- **FR-024**: Nel provider locale la capability e la configurazione sensibile
  DEVONO attraversare soltanto un canale bootstrap anonimo ereditato tramite
  allowlist; command line, environment, filesystem, readiness metadata e stdin
  generico sono vietati.
- **FR-025**: Il sidecar locale DEVE effettuare direttamente il bind su loopback
  e porta assegnata dal sistema; un listener raggiungibile NON È readiness.
- **FR-026**: Un worker DEVE diventare `ready` soltanto dopo verifica autenticata
  di identità, nonce/challenge, protocollo, versioni e health stateless.
- **FR-027**: Capability, Connection secret e SQL sensibile NON DEVONO comparire
  in log, errori, stdout/stderr, history, eventi UI, profiler, file temporanei o
  SQL esportato; la telemetria usa redazione o fingerprint.
- **FR-028**: Codice utente, browser, plugin e runtime esterni NON DEVONO
  ricevere endpoint o capability `execution_trusted_full_sql_v1` del worker.
- **FR-029**: Il control plane DEVE rappresentare identità, endpoint verificato,
  credenziale opaca e sicurezza del trasporto come un'unica risorsa e NON DEVE
  esporre PID, porta, path, Pod o dettagli provider allo scheduler.
- **FR-030**: Il pool globale DEVE separare policy elastica, admission/lease e
  provisioning; contare worker `starting`, `ready` e `leased`; rispettare
  capacità massima e budget globali; non offrire bypass on-demand illimitati.
- **FR-031**: La coda di admission DEVE essere FIFO, limitata, cancellabile e
  soggetta a timeout; l'assegnazione `ready -> leased` DEVE essere atomica.
- **FR-032**: Un worker acquisito DEVE essere single-use e sempre terminato a
  fine run; il sostituto DEVE avere nuova identità/capability e completare tutta
  la readiness prima della pubblicazione.
- **FR-033**: Lo scale-in DEVE terminare soltanto worker pronti e inattivi e
  usare finestre rinnovabili, così un picco storico non impedisce la riduzione
  futura.
- **FR-034**: Eventi e metriche DEVONO correlare run, stage, attempt, richiesta,
  durata, righe/byte, trasporto, memoria, spill e CPU senza duplicare il sistema
  di eventi applicativo nel sidecar.
- **FR-035**: Client, server ed estensione Quack DEVONO essere distribuiti e
  aggiornati come coppia compatibile; un mismatch DEVE fallire prima di
  eseguire qualsiasi stage.
- **FR-036**: Desktop, runner headless, artifact self-contained, scheduler, MCP,
  preview, inspect, drift, branch/diff e CI DEVONO usare il runner ufficiale e
  funzionare senza una DuckDB CLI installata.
- **FR-037**: Il package ufficiale DEVE includere runner ed estensioni richieste
  per startup offline sui target Windows, Linux e macOS supportati, con
  integrità e versione verificabili.
- **FR-038**: La CLI PUÒ restare soltanto come backend di compatibilità durante
  la migrazione; dopo i gate di parità DEVONO essere rimossi download, setup,
  binary staging, variabili dedicate, invocazioni, codice, test e documentazione
  che la presentano come requisito.
- **FR-039**: Prima dell'integrazione produttiva, `spikes/quack-sidecar` DEVE
  essere rinominato `spikes/quack-sidecar-phase0-spike`, mantenuto isolato dal
  workspace e marcato inequivocabilmente come PoC non distribuibile e non
  utilizzabile come fallback.
- **FR-040**: Nello stesso cutover che rende ufficiale il nuovo runner e rimuove
  la CLI, `spikes/quack-sidecar-phase0-spike` DEVE essere eliminato insieme a
  manifest, lockfile, sorgenti, comandi e riferimenti operativi; ADR e report
  storici DEVONO indicare che lo spike è stato rimosso dopo la validazione.
- **FR-041**: SlothDB e `xf.dbt` DEVONO restare disabilitati, senza fallback
  silenzioso; workspace e pipeline esistenti restano leggibili e ricevono
  rispettivamente `engine_disabled` o `component_disabled` in esecuzione.
- **FR-042**: Nessun percorso produttivo PUÒ essere instradato al nuovo runner
  prima che bootstrap sicuro, packaging offline, compatibilità di versione,
  containment, redazione e suite di parità siano verificati.

## Execution and Security Impact

- **Graph/planner**: il grafo persistito e il lowering restano autoritativi. Le
  modalità di accesso e le barriere di setup governano concorrenza e risorse;
  affinity e whitelist per component ID vengono rimosse.
- **Execution**: un solo database per run; richieste stateless; relazioni
  condivise regolari; batch preservati; concorrenza bounded; cancellazione
  process-level; Parquet resta fallback misurato.
- **Connections/secrets**: precedenza e masking esistenti restano invariati.
  Capability del worker e secret Data Source sono distinti, opachi e mai
  persistiti o inseriti nel testo delle query osservabile.
- **IPC**: process spawning, bootstrap/control channel, eventi, stato worker e
  cancellazione richiedono contratti serializzabili soltanto per dati non
  sensibili; handle e capability non attraversano l'IPC frontend.
- **Security**: loopback-only, full SQL consentito soltanto al supervisor
  trusted, handshake prima della readiness, process containment, handle
  allowlist, nessun accesso diretto da runtime o publication plane.
- **Multiplatform**: comportamento equivalente su Windows, macOS e Linux;
  differenze di Job Object/process group, handle/file descriptor e packaging
  richiedono test specifici di piattaforma.

## Compatibility and Migration

**Serialized format changed?** No per pipeline, nodi, edge e workspace. Eventuali
nuove impostazioni operative del worker devono avere default compatibili e non
entrare nel contratto del grafo.

**Migration / fallback**: migrazione incrementale dietro un contratto comune,
con CLI temporaneamente disponibile fino alla parità. Il cutover è atomico dal
punto di vista del prodotto: runner ufficiale e package offline diventano il
solo percorso DuckDB, poi CLI, affinity e spike Phase 0 vengono rimossi. Parquet
rimane un fallback di trasporto, non un backend di esecuzione alternativo.

**Existing component/pipeline behavior**: nessun documento viene riscritto per
il cambio backend. Tutti i componenti attivi devono superare test di parità;
SlothDB e `xf.dbt` conservano leggibilità e diagnostiche esplicite, senza
riattivazione implicita.

## Acceptance Criteria

- [ ] Tutti gli entry point DuckDB attivi eseguono tramite il runner ufficiale
  senza richiedere la CLI.
- [ ] Due run concorrenti non condividono worker, catalogo, credenziale o
  directory e non mostrano contaminazione.
- [ ] Richieste compatibili 2/4/8-way dimostrano concorrenza reale e risultati
  corretti; una nona richiesta attende e può essere cancellata.
- [ ] Pipeline prima instradate tramite affinity conservano risultati/eventi sul
  percorso normale e non rimane codice o routing affinity.
- [ ] Cancellazione, crash e parent death non lasciano processi o file temporanei
  oltre il budget di cleanup.
- [ ] Workload che supera la memoria disponibile completa tramite spill bounded
  senza OOM quando il budget disco è sufficiente.
- [ ] Token, secret e SQL sensibile sintetici sono assenti da ogni output
  persistente o osservabile coperto dalla suite.
- [ ] Worker senza handshake completo, con token errato o versione incompatibile
  non vengono mai assegnati.
- [ ] Admission e scale-in rispettano FIFO, capacità, budget, single-use e
  sostituzione con nuove identità.
- [ ] Desktop e artifact headless funzionano offline sui target supportati con
  coppia runner/extension verificata.
- [ ] Lo spike è prima rinominato e preservato come evidenza Phase 0, quindi
  eliminato quando il runner ufficiale supera tutti i gate.
- [ ] Pipeline e workspace esistenti restano leggibili senza migrazione.
- [ ] UI ed errori mantengono linguaggio e semantica esistenti, senza secret.
- [ ] Test di regressione, integrazione, sicurezza, packaging e build sono
  identificati per ogni piattaforma interessata.

## Success Criteria

- **SC-001**: Il 100% dei test comportamentali applicabili del backend corrente
  passa sul runner ufficiale con risultati, eventi e side effect equivalenti.
- **SC-002**: Dopo il cutover, una scansione verificabile del prodotto e dei
  package trova zero invocazioni produttive della DuckDB CLI, zero dipendenze da
  variabili CLI e zero riferimenti eseguibili ad affinity o allo spike Phase 0.
- **SC-003**: Il 100% dei tentativi con token assente, errato o revocato e con
  identità/versione inattesa viene rifiutato prima dell'assegnazione del worker.
- **SC-004**: In tutti i test con secret sintetici noti, zero valori sensibili
  compaiono in argv, environment post-bootstrap, file, log, errori, history,
  eventi, profiler o SQL esportato.
- **SC-005**: Il 100% delle cancellazioni provate durante coda, query, spill e
  trasferimento conclude lo stato della run e il cleanup interno entro 10
  secondi, senza worker o runtime orfani.
- **SC-006**: Nei test 2/4/8-way ogni richiesta concorrente usa una sessione
  server distinta, conserva la correttezza dei risultati e non supera 8
  richieste attive per worker.
- **SC-007**: Nei test di saturazione non si osservano doppie lease né capacità
  oltre il massimo; tutte le richieste accettate rispettano l'ordine FIFO salvo
  cancellazioni e timeout documentati.
- **SC-008**: Un workload di spill oltre il memory budget e dentro il disk
  budget completa senza OOM; uso e picco risultano osservabili e bounded.
- **SC-009**: Per query remote che restituiscono soltanto piccoli metadati, la
  memoria del main non cresce proporzionalmente alla dimensione del database
  misurata nei workload 1M/10M/100M.
- **SC-010**: Ogni workload del benchmark obbligatorio dispone di baseline sullo
  stesso hardware, soglia approvata e risultato entro soglia; il crossover
  Quack/Parquet è documentato per 1/2/4/8 consumer prima del cutover.
- **SC-011**: Una build pulita per ogni target supportato avvia ed esegue una
  pipeline rappresentativa senza rete e senza una CLI DuckDB installata.
- **SC-012**: Il 100% degli output persistiti esistenti usati nella suite di
  compatibilità resta leggibile senza conversione, inclusi documenti con
  SlothDB o `xf.dbt` che ricevono l'errore disabilitato previsto.

## Assumptions, Gaps, and Decisions

- **Confirmed fact**: il backend produttivo usa ancora DuckDB CLI e
  `AffinitySession`; l'evidenza è nel motore, nel runner e nella documentazione
  brownfield.
- **Confirmed fact**: lo spike Phase 0 ha prodotto un conditional GO per
  l'astrazione, non l'approvazione al cutover produttivo.
- **Confirmed fact**: lo spike ha verificato concorrenza stateless 2/4/8,
  catalogo server-side persistente, spill, kill e prewarm su Windows x64; restano
  aperti packaging offline, multipiattaforma, benchmark completo e parità.
- **Decision**: affinity non viene reinterpretata nel nuovo modello; viene
  rimossa completamente perché il database per-run e le richieste stateless ne
  eliminano la necessità.
- **Decision**: il PoC viene rinominato `spikes/quack-sidecar-phase0-spike`, resta
  temporaneamente come evidenza riproducibile e viene eliminato al cutover del
  runner ufficiale.
- **Decision**: la prima implementazione del provider è locale; il contratto
  resta provider-neutral, ma Kubernetes non è deliverable di questa feature.
- **Decision**: `quack_parallelism` iniziale usa il range verificato `1..=8`,
  default/massimo 8; aumentarlo richiede nuove misure e aggiornamento della
  decisione architetturale.
- **Decision**: SlothDB e dbt restano disabilitati e fuori dai gate di parità;
  la loro riattivazione richiede feature separate.
- **Gap**: le soglie prestazionali definitive non sono ancora fissate; devono
  essere approvate dopo baseline comparabili, mentre sicurezza, cleanup,
  correttezza, capacità e assenza di CLI/affinity hanno gate già misurabili.
- **Dependency**: la Feature 002 può essere rispecificata soltanto dopo la
  stabilizzazione del contratto del runner e non viene implementata qui.
