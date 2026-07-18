# Feature Specification: Quack Sidecar Database Runner

**Feature Branch**: `[003-quack-sidecar-database-runner]`
**Created**: 2026-07-18
**Status**: Draft
**Input**: "Creare la feature 003 Quack sidecar database runner usando il
contesto di documenti, ADR e report. Il sidecar Quack esistente è solo uno
spike: va rinominato e mantenuto temporaneamente per contesto, poi rimosso
quando il sidecar ufficiale è implementato. Rimuovere inoltre affinity, che con
il nuovo runner non serve più. Impostare a 3 i worker sidecar di base e definire
caratteristiche e comportamento del pool. Le modifiche alle opzioni hanno
effetto immediato: le query già attive terminano con il vecchio profilo e le
successive usano quello nuovo; anche la capacità base cambia tramite il normale
autoscaling senza interrompere sidecar in esecuzione."

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
tutto. Settings espone inoltre un unico profilo risorse per workspace: memoria,
thread CPU, quota di spill e massimo di query contemporanee della singola
pipeline run verso il proprio sidecar.

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
Settings estende la sezione esistente con “Runner resources”, senza introdurre
un secondo flusso di setup. La UI distingue runner, protocollo, sidecar per
run, thread CPU e query contemporanee; SlothDB e `xf.dbt` restano chiaramente
disabilitati.

## Clarifications

### Session 2026-07-18

- Q: Come si combinano pool e budget fra workspace? → A: Ogni istanza Duckle
  gestisce un solo workspace e la propria infrastruttura; un secondo workspace
  richiede una seconda istanza Duckle.
- Q: Come ottiene un worker una run senza capacità warm? → A: Ogni run chiede
  un worker al controller del pool; il controller assegna atomicamente un worker
  `ready` oppure avvia e assegna subito un worker on-demand, senza coda o limite
  numerico di run.
- Q: Cosa accade se la domanda richiede molti worker? → A: Non esiste budget o
  limite numerico di worker; ogni run richiede subito il proprio worker quando
  non trova capacità warm.
- Q: Come riparte il picco dopo un riavvio? → A: Il pool riparte dalla capacità
  base 3 e ricostruisce il picco dalle nuove run; il target precedente non è
  persistito.

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

### User Story 4 - Avviare worker pronti e autenticati con capacità elastica (Priority: P1)

**Why this priority**: prewarm e lease del pool devono ridurre la latenza senza
doppie assegnazioni o pubblicazione prematura, mentre una run senza worker
pronto riceve subito un worker on-demand dedicato.

**Independent test**: avviare il supervisor e sottoporre a pressione la capacità
disponibile con run concorrenti, startup lenti e falliti, cancellazioni,
crescita e scale-in, verificando target base, stati, domanda on-demand e
sostituzioni.

1. **Given** un avvio normale con risorse sufficienti e nessuna configurazione
   esplicita, **When** parte il supervisor, **Then** prepara in parallelo il
   target base predefinito di 3 worker e rende acquisibili soltanto quelli che
   completano la readiness autenticata.
2. **Given** capacità pronta, **When** il controller del pool riceve la richiesta
   di una run, **Then** assegna atomicamente un solo worker autenticato con lease
   esclusivo e single-use.
3. **Given** che il picco di domanda concorrente degli ultimi 5 minuti è `P`,
   **When** la policy valuta ogni 5 secondi e le risorse sono disponibili,
   **Then** imposta direttamente il target warm a
   `max(capacità_base, ceil(P * 1.20))`, senza soglie percentuali o step
   incrementali separati.
4. **Given** che non esiste un worker del pool in stato `ready`, **When** il
   controller del pool riceve la richiesta di una run, **Then** decide e avvia
   subito un worker on-demand dedicato, poi lo assegna a quella run. La run
   attende soltanto l'handshake autenticato del worker deciso dal controller.
   Non esistono coda, admission limit o massimo numerico configurabile di run.
   La domanda della run entra immediatamente e al 100% nel picco concorrente
   osservato.
5. **Given** 100 pipeline concorrenti e zero worker `ready`, **When** tutte
   ricevono worker on-demand e la policy effettua la prima valutazione, **Then**
   mantiene indipendenti le 100 run e porta il target warm a 120, cioè il picco
   di 100 con headroom del 20%, non a 53 tramite scatti progressivi.
6. **Given** un worker soltanto in ascolto o con handshake fallito, **When** il
   control plane valuta la readiness, **Then** il worker non diventa
   acquisibile, la capacità degradata è osservabile e il retry usa backoff senza
   creare provision concorrenti duplicati.
7. **Given** una run conclusa, **When** il lease viene rilasciato, **Then** il
   worker del pool è sempre terminato e un eventuale sostituto diventa pronto
   soltanto dopo un nuovo handshake completo.
8. **Given** domanda ridotta dopo un picco, **When** il picco non rientra più
   nella finestra scorrevole di 5 minuti, **Then** lo scale-in riduce il target
   soltanto fino al nuovo valore calcolato, termina soltanto worker `ready` e
   non scende mai sotto la capacità base configurata.

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

### User Story 7 - Configurare risorse e query parallele della singola run (Priority: P1)

**Why this priority**: l'utente deve poter adattare il sidecar della propria
pipeline a memoria, CPU e disco disponibili senza scambiare query concorrenti
per thread CPU o per numero di pipeline eseguibili.

**Independent test**: salvare nel workspace memoria, thread CPU, quota spill,
massimo query e capacità base; modificare il profilo durante query e run attive
e verificare applicazione al safe point, convergenza del pool e diagnostica del
valore richiesto/effettivo.

1. **Given** Settings aperto, **When** l'utente modifica il profilo risorse,
   **Then** può scegliere memoria e spill automatici, percentuali o quantità
   assolute, thread CPU automatici o interi positivi, e massimo query automatico
   o un intero da 1 a 8; può inoltre configurare la capacità base dei sidecar
   come intero positivo, con valore predefinito 3.
2. **Given** una pipeline run con un sidecar esclusivo, **When** il massimo
   query effettivo è N, **Then** fino a N query compatibili possono essere
   inviate contemporaneamente a quel sidecar; la N+1 attende cancellabilmente.
3. **Given** una o più query attive, **When** l'utente salva una modifica a
   memoria, thread CPU, spill o massimo query, **Then** la modifica diventa
   subito il profilo desiderato; le query attive terminano con il profilo
   precedente, le nuove attendono il drain e partono soltanto dopo
   l'applicazione atomica del nuovo profilo.
4. **Given** una modifica della capacità base, **When** il nuovo valore viene
   salvato, **Then** il normale ciclo di autoscaling converge subito verso la
   nuova base: crea capacità se aumenta, riduce soltanto worker `ready` se
   diminuisce e non interrompe né riconfigura i sidecar `leased`.
5. **Given** sidecar `leased` in eccesso rispetto alla nuova base, **When** le
   rispettive run terminano, **Then** quei sidecar vengono chiusi normalmente e
   non sostituiti finché la capacità non converge alla nuova base.
6. **Given** l'utente avvia molte pipeline, **When** ogni run chiede un worker
   al controller del pool, **Then** il controller assegna un `ready` o crea
   subito un worker on-demand; questi ultimi non entrano nel conteggio warm e
   muoiono alla fine della rispettiva run.
7. **Given** una preferenza di risorse eccede limiti di memoria, CPU, disco o
   futura licenza, **When**
   viene risolta, **Then** il sistema mostra richiesto, effettivo e ragione del
   clamp o del rifiuto senza modificare query già attive.

### Edge Cases

- Una run viene cancellata mentre il proprio worker on-demand completa bootstrap
  o mentre attende un permit interno.
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
- Cento pipeline arrivano in parallelo senza worker `ready`: il controller del
  pool riceve 100 richieste, decide 100 provisioning on-demand e li assegna
  senza coda; tali worker non entrano nel conteggio di capacità warm. Il picco
  concorrente è 100 e fa crescere il target warm a 120, senza budget o limite
  numerico di worker.
- All'avvio uno o più dei 3 worker base falliscono bootstrap o readiness: il
  pool espone la failure, evita doppio provisioning e riprova con backoff.
- Un burst arriva mentre i worker sostitutivi sono ancora `starting`: questi
  contano già nella capacità e non devono essere creati duplicati.
- Lo scale-in osserva un vecchio picco ma non elimina worker `leased`, non
  scende sotto la base configurata e rinnova la propria finestra anche quando
  non riduce.
- Il profilo cambia più volte mentre query del profilo precedente sono attive:
  le nuove query restano in attesa e, dopo il drain, usano atomicamente l'ultima
  versione valida salvata senza attraversare configurazioni intermedie.
- Il massimo query viene ridotto sotto il numero di query attive: nessuna query
  viene cancellata e nessuna nuova query parte prima del drain e
  dell'applicazione del nuovo limite.
- La capacità base viene ridotta mentre tutti i worker sono `leased`: nessuno
  viene terminato; il pool converge tramite release e mancato replenishment.
- La capacità base cambia mentre worker aggiuntivi sono `starting`: il ciclo di
  autoscaling ricalcola il target senza doppio provisioning e riduce soltanto
  worker diventati `ready`.
- Il profilo cambia mentre un worker è `ready` o `starting`: il worker `ready`
  applica subito il nuovo profilo e quello `starting` non può essere pubblicato
  con una versione superata.
- Il sistema operativo rifiuta lo spawn o il bootstrap di uno o più worker:
  falliscono soltanto i rispettivi worker/run con diagnostica sanitizzata; non
  viene introdotto un budget, una coda o un limite numerico di worker.
- Un worker on-demand fallisce bootstrap o la run viene cancellata durante
  bootstrap: fallisce o viene cancellata soltanto quella run, il worker viene
  pulito; la domanda osservata resta valida per la finestra di 5 minuti senza
  trasformare il sidecar in capacità del pool.
- Un picco dura meno di 5 secondi: il contatore di domanda è aggiornato agli
  eventi di avvio/fine run, perciò il campionamento non può perdere il picco.
- L'istanza viene riavviata durante la finestra di picco: scarta il picco e il
  target calcolato non persistiti, riparte dalla capacità base e ricostruisce la
  domanda dalle nuove run senza ripristinare worker warm precedenti.
- Il target warm calcolato non è ancora raggiunto: il controller continua a
  decidere worker on-demand per le richieste che non trovano un `ready`; non si
  crea una coda né un limite numerico di run.

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
- **FR-010**: Ogni pipeline run DEVE avere un solo sidecar e limitarne le
  richieste Quack concorrenti tramite un gate FIFO e cancellabile. Il massimo
  query della run è configurabile come automatico o `1..=8`, con default e
  massimo iniziale pari a 8; NON È il numero di sidecar nel pool né il numero
  di thread CPU. L'attesa NON DEVE creare connessioni o sidecar aggiuntivi.
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
  del runtime, non dal solo component ID. La policy DEVE essere una decision
  table versionata, sostenuta da benchmark riproducibili sullo stesso hardware,
  e ogni ramo DEVE avere test di selezione e di correttezza prima del cutover.
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
  picco separatamente dal main e dai runtime esterni. Readiness DEVE fallire se
  il profilo effettivo non è stato applicato integralmente; metriche
  sanitizzate di memoria e spill correnti e di picco DEVONO essere disponibili
  per ogni worker.
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
- **FR-030**: Ogni istanza Duckle DEVE gestire un solo workspace e un solo
  `WorkerPoolControl`, che è l'unico controller elastico, pool, lease, profilo,
  picco e target di quell'istanza. L'apertura di un altro
  workspace DEVE richiedere un'altra istanza Duckle con infrastruttura separata;
  worker, lease, profilo e capacità non attraversano istanze. Il target base DEVE
  avere valore predefinito 3 ed essere configurabile come intero positivo; i
  worker warm sono preparati in parallelo senza budget o limite numerico di
  worker. `QuackPermitGate` resta un gate per-run e NON DEVE essere implementato
  o gestito come un secondo pool di connessioni. La policy del primo rilascio ha
  quattro parametri espliciti e osservabili: capacità base 3 configurabile,
  headroom fissa 20%, periodo di valutazione 5 secondi e finestra
  scorrevole/stabilizzazione scale-in 5 minuti; headroom, periodo e finestra NON
  sono ulteriori impostazioni utente.
- **FR-055**: Ogni pipeline run DEVE presentare una sola richiesta di
  acquisizione a `WorkerPoolControl`; soltanto il controller DEVE decidere e
  compiere l'assegnazione atomica di un worker `ready` oppure il provisioning e
  l'assegnazione di un worker on-demand. Né la run né l'orchestratore possono
  aggirare il controller, scegliere direttamente il tipo di worker o avviare un
  worker fuori da questa decisione.
- **FR-031**: Ogni worker DEVE attraversare gli stati `starting`, `ready`,
  `leased`, `terminating` e `terminated`; soltanto `ready` è acquisibile. La
  capacità del pool DEVE contare soltanto worker del pool `starting + ready +
  leased`. Un worker diventa `ready` soltanto come bundle completo con sidecar,
  master client, secret scoped e health stateless autenticata; NON DEVE
  contenere clone o attachment Quack precreati. L'assegnazione `ready ->
  leased` DEVE essere atomica.
- **FR-032**: Un worker acquisito DEVE essere single-use e sempre terminato a
  fine run. La policy DEVE ricalcolare il target dopo release, cancellazione o
  crash e, se serve capacità, preparare un sostituto con nuova
  identità/capability che completa tutta la readiness prima della pubblicazione.
- **FR-033**: Ogni 5 secondi il controllo elastico DEVE calcolare il massimo
  numero di pipeline run concorrenti dell'unico workspace dell'istanza che hanno
  richiesto un sidecar nei 5 minuti precedenti, indipendentemente dal fatto che
  siano state servite da worker del pool o da worker on-demand. Il target warm
  richiesto dell'istanza DEVE essere
  `max(base_capacity, ceil(picco_5_minuti * 1.20))`; è un valore assoluto,
  non uno step, e sostituisce soglia del 70% e crescita del 20% della base.
  Se è superiore al target corrente, il normale scale-out DEVE avviare subito la
  differenza, contando i worker `starting` per evitare provisioning duplicato.
  Se è inferiore, lo scale-in può terminare soltanto worker `ready`, non può
  scendere sotto la base e non può interrompere o riconfigurare worker `leased`;
  questi non vengono rimpiazzati al rilascio se eccedono il nuovo target. Non
  DEVE esistere un massimo configurabile, una admission queue, backpressure o
  budget di worker che limiti il numero di pipeline o worker on-demand richiesti.
  Startup failure DEVONO essere osservabili e ritentati con backoff senza
  provisioning duplicato. Il picco e il target oltre la capacità base sono stato
  effimero dell'istanza: dopo un riavvio il controller DEVE ripartire dalla sola
  capacità base e ricostruire il picco dalle nuove run.
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
  containment, redazione, benchmark e suite di parità siano verificati. Fino a
  quel gate il runner ufficiale può essere usato soltanto dalla suite di test e
  dal percorso di compatibilità esplicitamente selezionato; gli entry point
  produttivi continuano a usare il backend di compatibilità. Tutti i finding
  rilevanti della checklist di qualità dei requisiti DEVONO risultare risolti
  oppure accettati esplicitamente con motivazione prima del gate. Il cutover
  abilita il runner ufficiale e rimuove affinity nello stesso rilascio.
- **FR-043**: Settings DEVE conservare per workspace un profilo non sensibile
  con memoria, thread CPU, quota di spill, massimo query per pipeline run e
  capacità base dei sidecar. La capacità base supporta un intero positivo e ha
  default 3.
  Memoria e spill supportano automatico, percentuale o quantità assoluta;
  thread supporta automatico o intero positivo; massimo query supporta
  automatico o `1..=8`.
- **FR-044**: Il resolver DEVE produrre profilo richiesto ed effettivo prima
  della readiness e dopo ogni modifica, considerando host, pool e futura
  licenza e mostrando ogni clamp/rifiuto con una ragione non sensibile. Le
  percentuali vengono convertite in valori assoluti e ogni profilo valido
  riceve una versione applicabile atomicamente.
- **FR-045**: Il profilo DEVE essere applicato ugualmente da desktop, headless,
  scheduler e MCP; non entra in PipelineDoc e non è autorità del frontend.
  Una modifica salvata DEVE diventare immediatamente il nuovo stato desiderato
  per il workspace senza richiedere riavvio o una nuova pipeline run.
- **FR-046**: La directory di spill resta controllata dal runner. Settings non
  espone un editor generico delle impostazioni DuckDB; thread CPU, query
  concorrenti, capacità base dei sidecar e numero di run devono avere etichette
  e telemetria distinte.
- **FR-047**: Una modifica a memoria, thread CPU, spill o massimo query DEVE
  essere applicata subito ai worker `ready`; un worker `starting` NON DEVE
  diventare `ready` con una versione superata. Per ogni sidecar `leased` la
  modifica DEVE usare una barriera di riconfigurazione cancellabile: le query
  già attive terminano integralmente con il profilo precedente, nessuna nuova
  query parte durante il drain e tutte le query successive usano atomicamente
  l'ultima versione valida del profilo. Una query NON DEVE osservare una
  configurazione parziale o cambiare profilo durante l'esecuzione.
- **FR-048**: Se più modifiche arrivano durante il drain, il sistema DEVE
  coalescerle sull'ultima versione valida. Se l'applicazione fallisce, il
  sidecar DEVE conservare il precedente profilo effettivo, mantenere pendente la
  riconfigurazione e rifiutare nuove query con `configuration_apply_failed`
  finché l'applicazione riesce o una versione successiva valida la sostituisce;
  NON DEVE riprendere con uno stato parzialmente applicato.
- **FR-049**: Una modifica della capacità base DEVE aggiornare immediatamente il
  target dell'autoscaler e NON DEVE avviare un percorso di resize separato. Un
  aumento usa il normale scale-out; una riduzione usa il normale scale-in,
  termina soltanto worker `ready` e marca i `leased` in eccesso come capacità da
  non rimpiazzare al rilascio. Worker `leased` e relative query NON DEVONO essere
  interrotti, riavviati o riconfigurati per raggiungere la nuova base.
- **FR-050**: Il controllo elastico DEVE emettere eventi e log strutturati,
  sanitizzati e correlabili per ogni valutazione e azione: capacità precedente,
  desiderata ed effettiva; conteggio `starting`/`ready`/`leased`; motivo della
  decisione; identificativo dell'istanza/workspace, capacità base, picco
  concorrente a 5 minuti, headroom, target calcolato e capacità pubblicata;
  richiesta/esito di provision,
  lease, scale-out, scale-in,
  release, replacement, backoff e failure. Gli eventi DEVONO correlare run,
  worker e lease quando presenti, senza includere endpoint, capability, secret o
  SQL sensibile.
- **FR-051**: Quando `WorkerPoolControl` riceve una richiesta e non ha un worker
  del pool in stato `ready`, DEVE decidere, avviare e assegnare immediatamente
  un worker on-demand esclusivo a quella run, senza metterla in coda né applicare
  un limite numerico di run. La run può iniziare gli stage soltanto dopo
  l'handshake autenticato del worker assegnato dal controller.
- **FR-052**: Un worker on-demand NON DEVE essere capacità warm del pool,
  entrare nello stato `ready`, essere riutilizzato, sostituito o conteggiato come
  worker del pool `starting`/`ready`/`leased`. DEVE terminare a completion,
  cancellazione o crash della propria run. La domanda della run che esso serve
  DEVE invece partecipare al 100% al picco concorrente di **FR-033** e **FR-054**,
  esattamente come una run servita da un worker del pool.
- **FR-053**: Telemetria e log DEVONO distinguere esplicitamente worker warm del
  pool e worker on-demand. Ogni provision, readiness, assegnazione, completion,
  cancellazione, cleanup o failure on-demand DEVE essere correlata alla sola run
  e dichiarare che è esclusa dai conteggi di capacità warm; deve riportare il
  contributo della relativa run al picco concorrente, il target derivato e la
  decisione, senza esporre endpoint, capability, secret o SQL sensibile.
- **FR-054**: Il controllo elastico DEVE trattare ogni decisione del controller
  che avvia un worker on-demand per assenza di worker `ready` come una unità di domanda
  concorrente dal momento della richiesta fino alla conclusione, cancellazione o
  failure della run. Ogni run DEVE contribuire una volta sola al picco,
  indipendentemente da worker warm del pool o on-demand, e il massimo DEVE essere registrato
  a eventi così da non perdere burst più brevi del periodo di valutazione. I
  worker on-demand non aumentano la capacità warm misurata del pool: fanno aumentare la
  domanda da cui è calcolato il target warm.

## Execution and Security Impact

- **Graph/planner**: il grafo persistito e il lowering restano autoritativi. Le
  modalità di accesso e le barriere di setup governano concorrenza e risorse;
  affinity e whitelist per component ID vengono rimosse.
- **Execution**: un solo database/sidecar per run; ogni run passa prima dal
  controller del pool, che assegna un worker `ready` o decide un worker
  on-demand; richieste stateless;
  relazioni condivise regolari; batch preservati; fino a N query compatibili
  verso lo stesso sidecar, con N configurato per run; cancellazione process-
  level; Parquet resta fallback misurato. Le modifiche al profilo usano un drain
  delle query attive prima dell'applicazione atomica alle successive; la
  capacità base converge soltanto tramite autoscaling. Se il pool non offre un
  worker `ready`, la run riceve un worker on-demand fuori conteggio warm che
  termina con la run; il numero di pipeline run non è limitato da questa
  configurazione.
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

**Serialized format changed?** No per pipeline, nodi ed edge. Sì, in modo
compatibile, per impostazioni operative del workspace: il precedente
`memory_limit_mb` viene interpretato come valore assoluto di memoria e assenza
dei nuovi campi risorsa equivale ad automatico, mentre l'assenza della capacità
base equivale a 3. Il profilo non entra nel contratto del grafo.

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
  corretti nello stesso sidecar della stessa pipeline run; una nona richiesta
  attende e può essere cancellata.
- [ ] Settings configura memoria, thread CPU, quota spill e massimo query per
  run e capacità base; la UI distingue queste grandezze dal numero di run.
- [ ] Il profilo richiesto, quello effettivo e il motivo di ogni clamp sono
  coerenti in desktop, headless, scheduler e MCP, senza secret.
- [ ] Una modifica del profilo diventa subito desiderata, non interrompe query
  attive e viene applicata atomicamente a tutte le query successive dopo il
  drain.
- [ ] Una modifica della capacità base converge tramite il normale autoscaling;
  nessun worker `leased` viene terminato o riconfigurato.
- [ ] Ogni pipeline run passa dal controller del pool; senza worker `ready`, il
  controller decide subito provisioning e assegnazione di un worker on-demand
  per run, senza coda o limite numerico.
- [ ] I worker on-demand sono esclusi dalla capacità warm e dal lifecycle del
  pool e terminano senza rientrarvi alla fine della propria run; le rispettive
  richieste alimentano la crescita di domanda prevista.
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
- [ ] Scale-in, capacità, single-use e sostituzione rispettano le invarianti
  dichiarate, senza admission queue o budget di worker.
- [ ] Il pool pubblica 3 worker base autenticati; ogni failure di bootstrap è
  sanitizzata, osservabile e ritentata senza introdurre un limite di worker.
- [ ] Ogni 5 secondi crescita e scale-in usano il picco di domanda concorrente
  della finestra scorrevole di 5 minuti e il 20% di headroom; domanda on-demand
  e pool contribuiscono entrambe al picco, i worker `starting` evitano provision
  duplicati e il floor resta pari alla base configurata.
- [ ] Ogni valutazione o azione di autoscaling espone motivo, capacità,
  conteggi, incremento calcolato, risorse e risultato senza divulgare segreti.
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
- **SC-006**: Nei test 2/4/8-way di una singola pipeline run, ogni richiesta
  concorrente usa una sessione server distinta, conserva la correttezza dei
  risultati e non supera 8 richieste attive verso il sidecar della run.
- **SC-007**: Nei test di molte pipeline non si osservano doppie lease né
  contaminazioni; ogni run ottiene un solo sidecar, dal pool se pronto oppure
  tramite worker on-demand se il pool non ha un worker `ready`.
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
- **SC-013**: In 100 avvii indipendenti del supervisor del pool, ciascuno con
  risorse sufficienti e timeout di readiness dichiarato, il pool raggiunge
  esattamente 3 worker base `ready` quando il campo è assente o vale 3; zero
  worker viene assegnato prima dell'handshake e zero worker `ready` contiene
  clone o attachment precreati.
- **SC-014**: Nei test di burst e scale-in, ogni valutazione ogni 5 secondi usa
  il massimo di domanda concorrente negli ultimi 5 minuti e imposta il target a
  `max(base, ceil(picco * 1.20))`; i worker `starting` impediscono provisioning
  duplicato e il target non scende sotto la base configurata.
- **SC-015**: Nel 100% dei test di modifica durante 1/2/4/8 query attive, le
  query già partite completano con il profilo precedente, zero query osserva un
  profilo misto e la prima query successiva al drain usa l'ultima versione
  valida salvata; worker `ready` e `starting` non accettano lease con una
  versione superata.
- **SC-016**: Nei test di modifica della capacità base verso valori maggiori e
  minori, il pool converge tramite scale-out/scale-in, termina zero worker
  `leased` e non sostituisce i worker in eccesso quando le rispettive run
  terminano.
- **SC-017**: Il 100% delle decisioni e azioni esercitate nei test di
  autoscaling produce un evento correlabile con capacità, conteggi, motivo,
  incremento o riduzione, risorse considerate ed esito; zero eventi contengono
  endpoint, capability, secret o SQL sensibile.
- **SC-018**: In un test con 100 pipeline lanciate in parallelo e zero worker
  `ready`, il controller riceve 100 richieste, decide subito 100 provisioning e
  assegnazioni on-demand, ciascuna correlata a una sola run; zero worker
  on-demand entra nei conteggi warm del pool e il 100% termina al completamento,
  cancellazione o crash della run. La valutazione registra un picco concorrente
  di 100 e porta il target warm da 3 a 120 tramite il normale provisioning del
  pool.
- **SC-019**: Con risorse sufficienti e nuove ondate di 100 pipeline a distanza
  di un minuto, dopo la prima ondata il pool raggiunge 120 worker `ready` prima
  della successiva. La seconda ondata di 100 ottiene worker del pool per tutte
  le run, conserva almeno 20 worker warm e non crea worker on-demand; anche questo
  picco continua a mantenere il target a 120.
- **SC-020**: Dopo un picco di 100 e nessuna ulteriore domanda per 5 minuti, la
  prima valutazione successiva riduce il target da 120 alla base configurata o
  al nuovo picco della finestra; termina zero worker `leased`, termina soltanto
  `ready` in eccesso e registra picco, headroom, target e motivo dello scale-in.
- **SC-021**: Dopo un riavvio avvenuto con target warm 120, l'istanza pubblica
  soltanto la capacità base 3 e non ripristina il vecchio picco; nuove pipeline
  ricevono immediatamente worker on-demand se necessario e ricostruiscono il
  picco per le successive valutazioni.

## Operational Definitions and Cutover Governance

Questa sezione rende misurabili i termini operativi usati da requisiti e criteri
di successo; prevale su descrizioni meno specifiche nei documenti di feature.

### Entry point e percorso di compatibilità

| Termine | Definizione vincolante |
|---|---|
| Entry point produttivo | Desktop (run, partial, preview), runner headless CLI/web, scheduler, MCP, inspect, drift, branch/diff e artifact distribuito avviati per un utente o un job reale. |
| CI | Unit/integration CI può selezionare il runner ufficiale per le prove; build/release CI non può pubblicare o abilitare il runner ufficiale finché il gate non è approvato. |
| Percorso di compatibilità | Il backend CLI/Affinity esistente, selezionato esplicitamente per entry point, che conserva il comportamento brownfield fino al cutover. Non è un fallback silenzioso dopo il cutover. |
| Percorso ufficiale pre-gate | Il runner Quack selezionabile solo da test e compatibilità esplicita; non riceve traffico produttivo. |
| Rifiuto del gate | Il release resta sul percorso di compatibilità, il cutover è bloccato e viene pubblicata una diagnostica sanitizzata con gli ID delle evidenze mancanti o fallite. Una nuova valutazione usa un nuovo manifest di evidenza e non altera run attive. |

Il **technical owner** prepara l'evidenza; il **release approver** accetta il
manifest e le eventuali deroghe motivate. I ruoli devono essere nominati nel
manifest di cutover prima della raccolta dati. Un gate è approvabile soltanto
quando tutti gli item SC-001--SC-011 applicabili hanno esito `pass`, ogni
finding rilevante di `runner-quality.md` è `resolved` o `accepted` con
motivazione, e non esistono deroghe di sicurezza, redazione, containment o
compatibilità offline. Le deroghe prestazionali devono indicare workload,
impatto, scadenza e approvatore; non consentono di aggirare gli altri gate.

### Profilo, failure e telemetria

Il resolver applica nell'ordine: valore richiesto valido, limite hard dell'host,
capacità fisica del workspace/pool e quindi limite di licenza quando presente.
Il primo vincolo più restrittivo produce il valore effettivo e una ragione
sanitizzata (`host_limit`, `workspace_capacity`, `license_limit` oppure
`invalid_profile`). Percentuali sono risolte contro la memoria o lo spazio
temporaneo effettivamente disponibile al momento della risoluzione.

Un worker non può completare readiness finché memoria, CPU thread, quota spill e
spazio temporaneo effettivi non sono applicati integralmente. Se la risoluzione
o l'applicazione fallisce, il worker non viene pubblicato; per un worker leased
il precedente profilo effettivo rimane attivo, il profilo desiderato resta
pendente e le nuove query ricevono `configuration_apply_failed` finché una
versione valida si applica. La cancellazione o lo shutdown prevalgono su una
riconfigurazione pendente e avviano cleanup senza tentare un apply successivo.

| Metrica sanitizzata per worker/run | Unità e frequenza | Destinazione/retention |
|---|---|---|
| memoria corrente e picco | byte, a inizio/fine richiesta e campione ogni 5 s | evento run/history con la retention già configurata per la run; nessun archivio runner separato |
| spill corrente e picco | byte, a inizio/fine richiesta e campione ogni 5 s | evento run/history con la retention già configurata per la run |
| CPU worker | millisecondi CPU cumulativi, a fine richiesta e campione ogni 5 s | evento run/history con la retention già configurata per la run |
| righe, byte trasferiti, durata e trasporto | contatori/durata per stage e attempt | evento stage/run e diagnostica UI non sensibile |

Sono ammessi soltanto ID opachi run/stage/attempt/worker/lease, conteggi,
durate, byte, stato, reason code e valori delle metriche sopra. Endpoint, porta,
PID, path, capability, token, secret, SQL e testo grezzo di errori Quack sono
redatti o sostituiti da fingerprint/diagnostica Duckle sanitizzata.

“Immediato” significa: un save rende atomico il nuovo profilo **desiderato**
quando la persistenza risponde con una nuova versione; un acquire senza ready
registra atomicamente domanda e decisione on-demand prima del provisioning. In
entrambi i casi l'unica attesa consentita è rispettivamente il drain della query
attiva o l'handshake autenticato del worker deciso; nessuna coda di admission o
resize separato viene introdotto.

### Decision table e benchmark

La decision table versionata per SQL remoto, trasferimento Quack e snapshot
Parquet riceve: volume stimato, numero di consumer, necessità di retry,
capacità del runtime, mutabilità, disponibilità del sidecar e costo di
materializzazione. Produce: meccanismo scelto, ragione, limiti di memoria/spill
e strategia di retry/cleanup. Eccezioni obbligatorie sono sidecar indisponibile,
runtime incapace di ricevere il formato scelto, errore di trasferimento,
consumer multipli e snapshot non riutilizzabile; ogni eccezione ha diagnostica
sanitizzata e delega retry/failure alla policy dell'orchestratore.

Il manifest benchmark, congelato **prima** di qualunque misura di cutover,
identifica: commit/build, versione DuckDB/Quack, OS, modello e core CPU, RAM,
disco e spazio libero, configurazione energetica, dataset/seed, generatori,
workload 1M/10M/100M, warm-up, numero di ripetizioni, raccolta di mediana e
percentili, e consumer 1/2/4/8. Ogni workload dichiara nel manifest una soglia
approvata prima dell'esecuzione; una soglia non può essere cambiata dopo aver
visto il risultato. Differenze di hardware richiedono un manifest separato e
non sono confrontabili con lo stesso gate. Il failure di una soglia mantiene il
percorso di compatibilità, registra la causa e richiede nuova baseline o
deroga prestazionale approvata prima di una nuova valutazione.

### Stati concorrenti, compatibilità e dipendenze

Il controller serializza acquire, release, scale e profilo per worker ID. Le
precedenze sono: `shutdown/cancel` > `crash/failure` > `release` >
`profile_apply` > `scale`. Un on-demand cancellato durante bootstrap viene
terminato e pulito; la domanda della run resta nel picco fino al suo evento
terminale. Un worker starting con profilo superato non viene pubblicato; se
non è più necessario per il target viene terminato senza lease. Con riduzione
della base, worker `ready` in eccesso terminano, worker `leased` terminano alla
fine della run senza replacement e worker `starting` vengono mantenuti soltanto
se necessari al target ricalcolato.

Workspace con campi di profilo assenti usano automatico/base 3; il legacy
`memory_limit_mb` è memoria assoluta. Un campo legacy sconosciuto o un profilo
non valido non modifica l'ultimo profilo effettivo e restituisce
`invalid_profile` con reason code non sensibile. Un entry point che non risolve
un profilo valido restituisce tale errore senza avviare un worker; un entry
point che non trova un bundle ufficiale verificato restituisce
`runner_unavailable`. Dopo il cutover nessuno dei due casi ripiega
silenziosamente sulla CLI.

La coppia DuckDB/Quack è posseduta dal maintainer del release package, viene
pinata con versione, checksum, licenza e provenienza nel manifest del bundle,
e segue il ciclo di aggiornamento del release. Windows, macOS e Linux sono
supportati soltanto dopo smoke offline per target; incompatibilità client,
server o estensione blocca readiness con `runner_version_mismatch` e richiede
un bundle compatibile, non download runtime.

L'istanza rifiuta l'apertura concorrente di un secondo workspace con
`workspace_already_open`; dopo close completo può aprire un nuovo workspace con
un nuovo controller, profilo e picco. La presente spec è l'autorità durante la
migrazione: ADR e feature intent devono essere riallineati prima del codice e
non possono mantenere hard maximum, queue o backpressure concorrenti.

## Assumptions, Gaps, and Decisions

### Elastic Policy Summary

| Voce | Regola vincolante |
|---|---|
| Isolamento | Un'istanza Duckle possiede un workspace e un controller; un altro workspace richiede una seconda istanza con infrastruttura separata. |
| Capacità base | Intero positivo dell'istanza/workspace, default 3; è il floor del target. |
| Domanda osservata | Numero massimo delle pipeline run concorrenti dell'istanza/workspace che richiedono un sidecar; include al 100% run servite dal pool e run servite on-demand, una volta sola per run. |
| Capacità del pool | Conta soltanto worker warm del pool dell'istanza `starting`/`ready`/`leased`; un worker on-demand non è capacità warm, non è riusabile e termina con la run. |
| Periodo di valutazione | Ogni 5 secondi; i picchi sono aggiornati anche agli eventi, così burst più brevi non sono persi. |
| Finestra del picco e scale-in | Finestra scorrevole di 5 minuti; il picco resta efficace fino alla sua scadenza, poi può avvenire scale-in. |
| Riavvio | Il picco e il target oltre la base non persistono: al riavvio si riparte dalla base e si osservano le nuove run. |
| Headroom | 20% fisso sul picco osservato. |
| Formula del target | `max(capacità_base, ceil(picco_5_minuti * 1.20))`. |
| Scale-out | Se il target calcolato è maggiore, avvia subito la differenza con il normale provisioning; `starting` evita duplicati. |
| Scale-in | Se il target calcolato è minore dopo la finestra di 5 minuti, termina solo `ready`; non interrompe, riavvia o rimpiazza `leased` in eccesso. |
| Limite worker | Non esiste budget, clamp o limite numerico di worker/pipeline; un errore di spawn/bootstrap è una failure osservabile del singolo worker/run, non una decisione di capacità. |

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
- **Decision**: un'istanza Duckle apre un solo workspace e possiede un solo
  controller elastico, `WorkerPoolControl`, con target base
  configurabile/default 3. Per aprire un altro workspace serve un'altra
  istanza Duckle, che possiede tutta la propria infrastruttura; non esiste
  condivisione di worker, profilo risorse, lease o picco tra istanze. I worker
  sono warm, esclusivi e single-use; il gate Quack interno al worker limita
  query concorrenti ma non contiene connessioni persistenti e non partecipa alla
  crescita del pool.
- **Decision**: la policy non usa soglia 70% né step di crescita della base.
  Ogni 5 secondi osserva il picco di pipeline run concorrenti nella finestra
  scorrevole degli ultimi 5 minuti e imposta il target a
  `max(base_capacity, ceil(picco * 1.20))`. Il 20% è headroom sul picco, non uno
  step. I worker warm del pool `starting`, `ready` e `leased` sono capacità;
  ogni run on-demand contribuisce al 100% alla domanda ma non diventa capacità
  warm del pool.
- **Decision**: `quack_parallelism` iniziale usa il range verificato `1..=8`,
  default/massimo 8, per la singola pipeline run e il suo unico sidecar;
  aumentarlo richiede nuove misure e aggiornamento della decisione
  architetturale. Non è un limite al numero di worker/sidecar né al numero di
  pipeline avviabili.
- **Decision**: memoria, thread CPU, quota spill e `quack_parallelism` sono un
  unico profilo per-workspace della Feature 003. Una modifica diventa subito il
  nuovo profilo desiderato; le query attive mantengono il profilo precedente e
  le successive usano quello nuovo dopo una barriera di drain. Il valore può
  essere ristretto da host/pool/licenza.
- **Decision**: la capacità base appartiene allo stesso profilo workspace, ha
  default 3 e si applica aggiornando il target del normale autoscaler. I worker
  `leased` non vengono toccati; una riduzione converge terminando worker `ready`
  e omettendo il replenishment dei worker in eccesso al termine della run.
- **Decision**: ogni run chiede il worker a `WorkerPoolControl`; il controller
  decide atomicamente se assegnare un worker `ready` o avviare e assegnare un
  worker on-demand, esclusivo e single-use. Non esistono admission queue,
  attesa o massimo numerico configurabile di run; i worker on-demand sono fuori
  dalla capacità warm e muoiono con la run, senza diventare `ready`. La domanda
  della run, non il worker on-demand come capacità, entra però al 100% nel picco:
  un picco di 100 fa preparare 120 worker warm per il giro successivo. Il picco
  rimane efficace 5 minuti e poi abilita il normale scale-in.
- **Decision**: SlothDB e dbt restano disabilitati e fuori dai gate di parità;
  la loro riattivazione richiede feature separate.
- **Gap**: le soglie prestazionali definitive non sono ancora fissate; devono
  essere approvate dopo baseline comparabili, mentre sicurezza, cleanup,
  correttezza, capacità e assenza di CLI/affinity hanno gate già misurabili.
- **Gap**: backoff di startup e la verifica dei tempi di readiness necessari a
  rendere disponibili i 120 worker prima dell'ondata successiva devono essere
  fissati nel piano e validati dai benchmark; headroom 20%, valutazione 5
  secondi, finestra di scale-in 5 minuti e assenza di un budget worker sono già
  vincolanti.
- **Gap documentale**: ADR e feature intent descrivono ancora hard maximum,
  admission queue e backpressure per la saturazione. Questa specifica richiede
  invece che il controller del pool decida worker on-demand immediati quando non
  ha un worker `ready`, fuori dalla capacità warm. ADR e intent devono essere
  riallineati prima dell'implementazione.
- **Dependency**: la Feature 002 può essere rispecificata soltanto dopo la
  stabilizzazione del contratto del runner e non viene implementata qui.
