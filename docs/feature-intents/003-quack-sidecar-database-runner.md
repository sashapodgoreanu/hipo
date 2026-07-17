# Feature intent architetturale: DuckDB sidecar con protocollo Quack

## Stato

**Proposta completa da validare con ADR, prototipo e benchmark prima della
specifica implementativa.**

**Data**: 2026-07-17  
**Dipendenza funzionale**: sblocca la futura rispecificazione di
`002-universal-query-source-and-multi-input-query`.

Questo documento descrive il cambiamento architetturale desiderato e conserva
le decisioni già condivise. Non è ancora una specifica Spec Kit pronta per
generare piano e task: alcune scelte devono essere decise sulla base di misure
e prove multipiattaforma.

## Sintesi

Duckle deve smettere di usare DuckDB CLI come processo avviato per stage o come
sessione persistente pilotata tramite stdin. Ogni esecuzione di pipeline deve
avere un processo isolato `duckle-db-runner` che incorpora DuckDB tramite la
libreria Rust e possiede un solo database per tutta la durata del run.

Il processo principale di Duckle agisce come **Quack client**. Il sidecar agisce
come **Quack server**. SQL, risultati e scritture viaggiano tramite il protocollo
Quack su localhost; non viene introdotta una REST API dati parallela.

Il database, le relazioni della pipeline, le connessioni, i join, le
materializzazioni, la memoria DuckDB e lo spill appartengono al sidecar. Il
processo principale conserva planner, DAG orchestration, UI/eventi, history e
coordinamento dei runtime non-DuckDB.

La cancellazione termina l'intero processo sidecar. Non viene richiesta una
cancellazione remota della singola query.

## Decisioni già assunte

1. Esiste un sidecar dedicato per ogni run; non viene condiviso fra pipeline.
2. Il main process è Quack client e il sidecar è Quack server.
3. Quack è l'IPC per tutte le operazioni DuckDB ordinarie.
4. Non viene creata una REST/JSON API alternativa per trasportare query o dati.
5. Il sidecar incorpora DuckDB con `duckdb-rs`; non esegue la CLI.
6. Il main incorpora un'istanza DuckDB client necessaria a Quack, ma non deve
   eseguire il piano analitico della pipeline.
7. Le query complete vengono spedite verbatim al server con `quack_query` o
   `remote.query(...)`, così join e materializzazioni restano remoti.
8. Ogni run ha un catalogo condiviso e più connessioni server concorrenti.
9. La cancellazione del run butta giù il sidecar e classifica le connessioni
   interrotte come `cancelled`, non come errore DuckDB.
10. Parquet rimane disponibile come fallback e come termine di confronto; non
    deve essere rimosso prima di conoscere il punto di crossover con Quack.
11. La scelta fra database in-memory compresso, file-backed temporaneo o ibrido
    sarà presa soltanto dopo benchmark rappresentativi.
12. La Feature 002 rimane sospesa finché questa infrastruttura non offre un
    contratto stabile per relazioni, connessioni e runtime esterni.
13. SlothDB viene disabilitato durante questa iniziativa: DuckDB è l'unico
    engine di esecuzione attivo e non viene richiesto al nuovo runner di
    conservare un'astrazione multi-engine.
14. Il nodo `xf.dbt` e il provisioning di dbt Fusion vengono disabilitati:
    dbt non viene migrato a Quack e non costituisce un gate per la rimozione
    della CLI.
15. I sidecar vengono acquisiti tramite lease esclusivi da un pool prewarm:
    una pipeline run usa un solo worker e un worker serve una sola pipeline run.
16. Il worker viene sempre terminato a fine run o cancellazione. Non torna mai
    nel pool; un sostituto entra nella coda `ready` soltanto dopo il nuovo
    handshake DuckDB/Quack.
17. Policy elastica, coda di admission e provisioning sono separati. Il backend
    iniziale avvia processi locali, ma il contratto deve consentire in futuro un
    backend Kubernetes senza cambiare scheduler, `RunSession` o `RunDatabase`.

## Motivazione

L'architettura CLI attuale ha tre limiti strutturali:

- l'avvio ripetuto del processo DuckDB introduce latenza e moltiplica i confini
  di serializzazione;
- sessione, attachment e catalogo non hanno un proprietario unico robusto;
- l'affinity è costretta a classificare componenti specifici anziché governare
  una risorsa condivisa con un contratto generale.

Un database posseduto da un sidecar per l'intera pipeline consente invece:

- cache e catalogo caldi per tutto il run;
- più lettori e writer coordinati sulla stessa istanza;
- Query Source e futuri nodi Query indipendenti dal tipo del downstream;
- eliminazione dello stdin e dei marker file come protocollo SQL;
- isolamento di CPU, memoria, crash e terminazione DuckDB dal processo UI;
- spill controllato in una directory di run;
- una base unica per desktop, runner headless, scheduler e MCP.

## Evidenze dal brownfield scan

Il repository è un Cargo workspace Rust con desktop Tauri/React, runner
headless, scheduler e MCP. L'esecuzione DuckDB è concentrata in
`crates/duckdb-engine`, ma il suo contratto CLI attraversa molti moduli.

Stato verificato al momento di questa proposta:

- `DuckdbEngine` conserva un path al binario e un flag atomico di cancellazione;
- ogni run crea un database temporaneo `.duckdb` su disco;
- il fast path invia l'intera pipeline a una CLI e usa file marker per gli
  eventi di stage;
- il percorso ordinario apre la CLI per stage;
- `AffinitySession` mantiene una CLI persistente, serializza gli statement su
  stdin e usa file JSON/marker per completamento, preview e runtime;
- il percorso affinity accetta soltanto una matrice limitata di stage;
- `ctl.parallelize` esporta una volta l'upstream in Parquet e avvia branch con
  database temporanei indipendenti;
- `code.python` legge tutte le righe in JSON, avvia Python e reimporta un file
  JSON;
- `xf.dbt` apre direttamente il file DuckDB del run tramite `dbt-duckdb`;
- i helper dell'engine contengono 26 invocazioni dirette `run`, 45 letture
  `run_rows` e 37 materializzazioni JSON→DuckDB;
- desktop, runner, scheduler, MCP, drift, data branch, build artifact e CI
  conoscono il binario DuckDB CLI o `DUCKLE_DUCKDB_BIN`;
- il workspace dichiara già una dipendenza `duckdb`, ma nessun crate la usa;
- il desktop possiede già un modello di packaging per sidecar compressi, usato
  per `duckle-lance`, runner e MCP;
- gli artifact self-contained includono oggi DuckDB CLI ed extension cache.

La migrazione non può quindi limitarsi a sostituire `AffinitySession`: deve
introdurre un confine stabile che copra tutti i consumatori di `DuckdbEngine`.

## Decisione temporanea su SlothDB

SlothDB non partecipa alla nuova infrastruttura Quack e viene disabilitato per
ridurre il numero di confini da migrare. In questa fase:

- DuckDB è l'unico engine selezionabile per nuovi run;
- setup, stato e selezione engine non devono proporre SlothDB come disponibile;
- una pipeline o configurazione esistente che richiede esplicitamente SlothDB
  deve ricevere un errore chiaro `engine_disabled`, senza fallback silenzioso a
  DuckDB;
- crate e codice SlothDB possono rimanere nel repository, ma non fanno parte
  dei gate di parità del runner Quack;
- non vengono aggiunti adapter Quack o compatibility layer per SlothDB;
- la sua eventuale riattivazione richiederà una feature e una decisione
  architetturale separate, dopo la stabilizzazione del nuovo runner DuckDB.

Questa è una disabilitazione funzionale, non ancora una rimozione definitiva
del codice o dei formati persistiti. I workspace esistenti devono rimanere
leggibili anche quando il run viene rifiutato.

## Decisione temporanea su dbt

Il nodo `xf.dbt` non partecipa alla nuova infrastruttura e viene disabilitato
insieme alla sua installazione automatica. In questa fase:

- `xf.dbt` non deve essere selezionabile per nuove pipeline e deve risultare
  nascosto o esplicitamente disabilitato nella palette;
- l'Engine Setup non deve installare dbt Fusion insieme a DuckDB;
- i comandi di installazione e lo stato dbt non devono essere eseguiti durante
  il normale setup o startup;
- una pipeline esistente contenente `xf.dbt` deve rimanere leggibile, ma il run
  deve fallire prima dell'esecuzione con `component_disabled` e una diagnostica
  chiara;
- non deve esistere un fallback silenzioso a dbt Core, dbt-duckdb, SQL generico
  o backend CLI;
- il codice di integrazione e i documenti persistiti possono rimanere nel
  repository per compatibilità e futura valutazione;
- dbt non fa parte dei test di parità, dei benchmark o dei release gate del
  runner Quack;
- l'eventuale riattivazione di dbt richiederà una feature separata, inclusa una
  decisione esplicita sul collegamento a Quack.

La disabilitazione elimina il vincolo attuale per cui `dbt-duckdb` apre
direttamente il file `.duckdb` del run. Non è quindi necessario mantenere un
database file-backed o il backend CLI soltanto per supportare dbt.

## Architettura target

```text
┌───────────────────────────────────────────────────────────────┐
│ Duckle main / duckle-runner / scheduler                       │
│                                                               │
│ planner ─ DAG scheduler ─ admission queue ─ WorkerPoolControl │
│                                      │                        │
│                               exclusive lease                 │
│                                      │                        │
│                                QuackRunClient                 │
└──────────────────────────────────────┬────────────────────────┘
                                       │ WorkerEndpoint
                 ┌─────────────────────┴─────────────────────┐
                 │                                           │
       LocalProcessProvider                         KubernetesProvider
       (prima implementazione)                      (futuro, stesso contratto)
                 │                                           │
                 └─────────────────────┬─────────────────────┘
                                       ▼
                          duckle-db-runner / Quack server
                          DuckDB embedded, un solo run
                          catalogo, memory limit, spill
```

Il control plane non conosce PID, porte locali o Pod come concetti di dominio.
Conosce soltanto `WorkerId`, stato, capacità, `WorkerEndpoint`, readiness e
lease. I dettagli di processo e Kubernetes restano confinati nel provider.

## Responsabilità del processo principale

Il processo principale deve:

- risolvere workspace, pipeline, parametri e dipendenze del DAG;
- accodare la richiesta e acquisire un worker `ready` con lease esclusivo;
- creare un `RunSession` sul `WorkerEndpoint` assegnato;
- mantenere un pool di connessioni Quack client per il run;
- inviare SQL completo al server;
- avviare e coordinare runtime esterni;
- emettere eventi di stage e aggiornare history/UI;
- decidere retry e prosecuzione dei rami indipendenti;
- delegare lifecycle e terminazione al `WorkerProvider` proprietario;
- terminare sidecar e processi associati in caso di fine run o cancellazione;
- eliminare gli artefatti temporanei anche se il sidecar è stato ucciso.

Non deve:

- aprire il database della pipeline localmente;
- eseguire join, sort, aggregate o CTAS della pipeline;
- scaricare nel main una relazione completa salvo richiesta esplicita di un
  runtime che deve elaborarla fuori da DuckDB;
- usare il client Quack come database alternativo della pipeline;
- esporre la connessione DuckDB client grezza al planner o ai componenti.

## Responsabilità del sidecar

`duckle-db-runner` deve:

- aprire una sola istanza DuckDB per il run;
- configurare storage, memoria, thread e spill prima di accettare richieste;
- caricare Quack e le extension DuckDB richieste;
- avviare il Quack server esclusivamente su localhost;
- possedere catalogo, tabelle, view, attachment e transazioni;
- consentire più connessioni client concorrenti;
- eseguire tutto il SQL analitico e le materializzazioni;
- mantenere uno schema di sistema riservato a health, protocol version e
  metriche;
- produrre log sanitizzabili e metriche finali del run;
- terminare se il processo padre non esiste più;
- chiudere normalmente quando il run è concluso, oppure essere terminato
  forzatamente durante la cancellazione.

## Lifecycle del run

### Prewarm e admission

Il supervisor mantiene un target elastico di worker già inizializzati. Un
worker attraversa gli stati `starting`, `ready`, `leased`, `terminating` e
`terminated`; soltanto `ready` è acquisibile.

1. Il pool avvia in parallelo la capacità base.
2. Il provider considera completato il provisioning soltanto dopo readiness
   infrastrutturale e handshake applicativo Quack.
3. Una richiesta di run entra in una coda FIFO cancellabile e con timeout.
4. L'acquisizione cambia atomicamente `ready -> leased` e lega worker ID, run ID
   e lease ID. Anche due esecuzioni della stessa pipeline ricevono worker
   diversi.
5. Alla conclusione o cancellazione, il worker viene terminato e non riusato.
6. La policy ricalcola il target; se manca capacità, prepara un nuovo worker e
   lo pubblica soltanto quando è nuovamente `ready`.

La capacità conta `starting + ready + leased`, non soltanto i worker disponibili.
Questo evita creazioni duplicate mentre un avvio lento è già in corso.

### Policy elastica bounded

L'idea prende spunto da una policy esistente basata su capacità minima, soglia
di utilizzo, crescita a step e riduzione da picco osservato, ma il contratto di
Duckle non ne replica l'implementazione.

- `base_capacity`: capacità minima prewarm;
- `max_capacity`: limite rigido di worker, ulteriormente limitato dal budget
  globale RAM/CPU/disco;
- `grow_threshold`: soglia iniziale proposta 70%, da validare con benchmark;
- `growth_step`: inizialmente `max(1, ceil(base_capacity * 0.25))`;
- `scale_in_window`: finestra mobile o tumbling che viene sempre rinnovata;
- `scale_in_headroom`: target derivato dal picco recente più margine;
- `queue_limit`, `acquire_timeout` e politica di fairness espliciti.

La saturazione non crea worker on-demand oltre `max_capacity`: applica
backpressure e mantiene la richiesta in coda. Lo scale-in termina solo worker
`ready`; per quelli `leased` registra capacità da non rimpiazzare quando il run
finisce. Una finestra conclusa senza riduzione deve comunque ripartire con un
nuovo picco, evitando che un picco storico blocchi per sempre lo scale-in.

Il budget globale riserva risorse anche per i worker `starting`. Per la prima
versione tutti i worker possono usare un unico profilo; in seguito il contratto
può introdurre profili di capacità senza cambiare la semantica del lease.

### Contratto del provider

Il pool usa un'interfaccia asincrona concettualmente equivalente a:

```rust
trait WorkerProvider {
    async fn provision(&self, spec: WorkerSpec) -> Result<ProvisionedWorker>;
    async fn readiness(&self, worker: &WorkerRef) -> Result<WorkerEndpoint>;
    async fn terminate(&self, worker: WorkerRef, reason: TerminationReason)
        -> Result<()>;
    async fn inspect(&self, worker: &WorkerRef) -> Result<WorkerObservation>;
}
```

`provision` non rende il worker acquisibile. Il passaggio a `ready` appartiene
al control plane dopo l'handshake e deve essere idempotente. `terminate` deve
essere idempotente perché completion, cancellazione, crash e shutdown del
supervisor possono concorrere.

Il provider locale traduce il contratto in processo, Windows Job Object/process
group, porta localhost e directory di run. Il futuro provider Kubernetes lo
traduce preferibilmente in un Kubernetes Job single-worker, relativo Pod,
identity/label, endpoint di rete, readiness probe, Secret/bootstrap ed
`emptyDir` per spill.

### Compatibilità futura con Kubernetes

Il modello non deve basarsi su un `Deployment` generico né sull'HPA per
assegnare pipeline. I worker sono risorse stateful, single-use e con lease
esclusivo; il target elastico è quindi governato dal control plane Duckle.

Una prima implementazione Kubernetes dovrebbe creare un Job effimero per ogni
worker, con un solo Pod, `restartPolicy: Never` e retry controllato. Terminare
un worker significa eliminare il Job proprietario, non soltanto il Pod, per
evitare che il Job controller lo ricrei. Quando serve alta disponibilità del
supervisor, l'evoluzione naturale è un controller con CRD
`DuckleWorkerPool`/`DuckleWorker` oppure uno store con CAS. La decisione
definitiva è rinviata, ma devono restare invarianti:

- `WorkerId` e `LeaseId` sono identità logiche, non nomi Job/Pod;
- l'assegnazione `ready -> leased` è atomica anche con più repliche del
  supervisor;
- il Job e il suo Pod sono cancellati a fine run e non vengono rimessi `ready`;
- la cancellazione della pipeline elimina la risorsa proprietaria e classifica
  la chiusura del trasporto come `cancelled`;
- un sostituto viene contato come capacità già durante `Pending/starting`, ma è
  assegnabile soltanto dopo readiness probe e handshake Quack;
- CPU, memoria ed ephemeral storage hanno request/limit; il `memory_limit` di
  DuckDB resta inferiore al limite del container per lasciare headroom;
- lo spill usa `emptyDir` disk-backed con `sizeLimit`, non RAM (`medium: Memory`),
  salvo benchmark esplicitamente favorevole;
- endpoint discovery, TLS/service identity e NetworkPolicy sostituiscono il
  vincolo localhost senza cambiare il protocollo dati Quack;
- scheduler e runtime vedono sempre `WorkerEndpoint`, indipendentemente da Pod
  IP, Service o tunnel usato dal provider.

Il provider locale resta il primo target e usa lo stesso state machine. In
questo modo Kubernetes aggiunge un control-plane adapter e deployment policy,
non una seconda implementazione dell'esecuzione pipeline.

### Bootstrap

1. Il provider locale crea una directory univoca per il worker; il provider
   Kubernetes crea storage effimero equivalente.
2. Genera token Quack casuale, run ID e configurazione.
3. Scrive un bootstrap file con permessi limitati all'utente corrente.
4. Il provider avvia `duckle-db-runner` passando configurazione e identità con
   un meccanismo protetto; stdin non viene usato come protocollo.
5. Il sidecar legge e cancella il bootstrap file, apre DuckDB e avvia Quack su
   una porta localhost libera.
6. Il provider locale osserva `ready.json`; Kubernetes usa una probe locale al
   container. In entrambi i casi PID/Pod e token non fanno parte del contratto
   pubblico del pool.
7. Il main crea il client Quack, autentica e verifica `ducklesys.health()`.
8. Solo dopo il health check vengono avviati gli stage.

Il bootstrap file è un meccanismo di avvio, non una seconda API dati.

### Esecuzione

Il main mantiene `RunSession`:

```text
RunSession
  ├─ run_id
  ├─ run_directory
  ├─ worker_lease
  ├─ worker_endpoint
  ├─ cancellation_state
  ├─ QuackRunClient pool
  └─ runtime_process_scope
```

Ogni operazione DuckDB passa attraverso un'interfaccia tipizzata, per esempio:

```rust
trait RunDatabase {
    async fn execute_remote(&self, request: SqlRequest) -> Result<SqlOutcome>;
    async fn query_remote(&self, request: QueryRequest) -> Result<QueryResult>;
    async fn describe_relation(&self, relation: RelationRef) -> Result<Schema>;
    async fn preview(&self, relation: RelationRef, limit: usize) -> Result<Preview>;
    async fn import(&self, input: RelationInput, target: RelationRef) -> Result<ImportResult>;
    async fn export(&self, source: RelationRef, format: TransferFormat) -> Result<Artifact>;
    async fn metrics(&self) -> Result<RunDatabaseMetrics>;
}
```

L'implementazione primaria è `QuackRunDatabase`; durante la migrazione può
esistere un backend legacy CLI dietro la stessa interfaccia.

### Conclusione normale

1. Il main attende tutti gli stage.
2. Legge metriche finali e stato del catalogo.
3. Rilascia il lease con esito e metriche finali.
4. Il provider termina sempre quel worker entro un timeout breve.
5. Se la chiusura cooperativa non riesce, applica la terminazione forzata.
6. Pulisce storage, spill e snapshot non persistenti.
7. Il pool prepara un sostituto se il target elastico lo richiede.

### Cancellazione

1. Il main marca il run `cancelling`.
2. Impedisce l'avvio di nuovi stage.
3. Chiude o abbandona le richieste Quack client attive.
4. Chiede al provider di terminare il worker associato al lease.
5. Attende l'uscita; dopo il timeout applica una terminazione forzata.
6. Converte gli errori di socket conseguenti in `cancelled`.
7. Pulisce le risorse del worker e registra `cancelled` in history.

Non viene implementata la cancellazione del singolo statement DuckDB. La
cancellazione di uno stage attivo cancella l'intero run.

## Contratto Quack

Il main crea una o più connessioni DuckDB locali dedicate esclusivamente al
client Quack. Ogni connessione usa `ATTACH` verso il sidecar per ottenere una
sessione sticky:

```sql
CREATE SECRET run_quack_secret (
    TYPE quack,
    TOKEN ?,
    SCOPE 'quack:127.0.0.1:<port>'
);

ATTACH 'quack:127.0.0.1:<port>' AS run_remote (TYPE quack);
SET httpfs_connection_caching = true;
```

Gli stage vengono inviati come query verbatim:

```sql
FROM run_remote.query($sql$
    CREATE OR REPLACE TABLE run_data.stage_10 AS
    SELECT ...
$sql$);
```

Questo vincolo è fondamentale. Eseguire nel client una query che combina
tabelle remote potrebbe spostare operatori nel processo principale. Il wrapper
`QuackRunClient` deve quindi rendere pubblico `execute_remote`, non la
connessione locale grezza.

Le query ordinarie devono restituire solamente metadati piccoli. Dataset grandi
attraversano Quack solo quando un runtime esterno li richiede esplicitamente.

## Protocollo applicativo SQL

Il sidecar espone uno schema riservato `ducklesys`, versionato e non utilizzabile
come namespace dei nodi utente.

Contratto minimo desiderato:

```sql
FROM ducklesys.health();
FROM ducklesys.runtime_info();
FROM ducklesys.memory_status();
FROM ducklesys.spill_status();
FROM ducklesys.protocol_info();
```

Il protocollo deve riportare almeno:

- protocol version Duckle;
- versione DuckDB e versione Quack;
- run ID e sidecar PID;
- modalità storage;
- memory limit, uso corrente e peak osservato;
- directory, uso corrente e peak dello spill;
- numero di connessioni e query attive;
- uptime e stato `ready/running/finishing`.

Shutdown e altre funzioni di controllo possono essere implementati tramite
funzioni registrate o una tabella di controllo osservata dal sidecar, ma devono
viaggiare attraverso Quack. La terminazione forzata resta sempre disponibile
attraverso il process handle.

## Catalogo e nomi delle relazioni

Ogni sidecar serve un solo run, ma il catalogo deve comunque separare:

- `ducklesys`: oggetti interni e metriche;
- `run_data`: output materializzati degli stage;
- cataloghi live dichiarati dalle Data Source;
- oggetti temporanei legati a una connessione.

Gli output attraversabili devono avere nomi deterministici derivati dal node ID
e dal nome dell'output, non dal component ID o dalla label visuale. Alias SQL,
node ID e component ID restano concetti distinti.

Le tabelle condivise fra connessioni devono vivere in `run_data`. Gli oggetti
`TEMP` sono ammessi solamente all'interno di un lease pinned alla stessa
connessione e non possono costituire il contratto fra due stage generici.

## Pool di connessioni e parallelismo

Il sidecar contiene una sola istanza DuckDB, ma ogni lavoro concorrente usa una
connessione server distinta. Il main usa una connessione Quack client distinta
per ogni richiesta concorrente o una connessione acquisita da un pool.

Lo scheduler deve ragionare per modalità di accesso dichiarata:

| Modalità | Esempio | Scheduling iniziale |
|---|---|---|
| `SharedRead` | scan, preview, describe | parallelo |
| `Append` | append alla stessa tabella | parallelo con verifica conflitti |
| `Mutation` | insert/update/delete | parallelo solo se compatibile, altrimenti retry |
| `SchemaChange` | create/replace/drop/attach | lease esclusivo sulla risorsa interessata |
| `ConnectionPinned` | uso di stato TEMP/sessione | stessa connessione |
| `ExternalTransfer` | Python/Rust | lease fino a import/export completato |
| `UnknownSql` | SQL non classificabile | conservativo: esclusivo |

La prima milestone può preservare l'esecuzione sequenziale per ottenere parità,
ma la feature architetturale deve includere il parallelismo sicuro come
risultato finale. DuckDB supporta più connessioni nello stesso processo e
append concorrenti; conflitti su aggiornamenti incompatibili devono essere
ritentati o riportati in modo deterministico.

## Nuovo significato di affinity

Affinity non deve più significare “usa la stessa CLI e accetta solo alcuni
component ID”. Deve descrivere requisiti di risorsa e sessione:

- quali Data Source live servono allo stage;
- quali inizializzazioni devono esistere su una connessione;
- se lo stage può usare una relazione materializzata condivisa;
- se richiede una connessione pinned;
- quali lease sono condivisi o esclusivi.

Ogni nuova connessione Quack deve inizializzare le risorse richieste in modo
idempotente. Il prototipo deve verificare quali attachment DuckDB sono globali
all'istanza e quali rimangono connection-local; il planner non deve assumere
una semantica non provata.

La compatibilità downstream non dipende da whitelist o blacklist di
`component_id`.

## Runtime esterni e trasferimento dati

I runtime non-DuckDB possono continuare a vivere nel main o in processi figli,
ma non devono aprire direttamente il database del sidecar.

Sono previsti tre percorsi:

### SQL pushdown

Quando il runtime può esprimere il lavoro in SQL, il main invia la query completa
al sidecar. È il percorso preferito perché non trasferisce la relazione.

### Quack streaming

Python, Rust o altri client compatibili possono collegarsi direttamente al
sidecar con un token limitato al run. Possono leggere input e scrivere output
senza coinvolgere il main nel payload.

### Snapshot Parquet

Per grandi fan-out, tool incompatibili con Quack, retry indipendenti o carichi
in cui lo streaming ripetuto risulta più costoso, il sidecar esporta una volta
un Parquet nella run directory e i consumer lo leggono.

La scelta deve essere effettuata da una policy di trasferimento, non codificata
nel tipo di componente:

```text
RelationTransport
  ├─ RemoteSql
  ├─ QuackStream
  └─ ParquetSnapshot
```

Il benchmark deve individuare il punto di crossover per volume, numero di
consumer e tipo di trasformazione.

## Consumer residui del file DuckDB

Con dbt disabilitato, la CLI può essere rimossa quando branch/diff, drift,
inspect e build artifact hanno un percorso equivalente testato. Nessun database
file-backed o backend legacy deve essere conservato esclusivamente per dbt.

## Memoria, storage e spill

Il requisito non è “usare sempre `:memory:`”, ma mantenere il working set caldo,
evitare OOM e ridurre I/O non necessario.

Il sidecar deve configurare esplicitamente:

```sql
SET memory_limit = '<budget>';
SET temp_directory = '<run-dir>/spill';
SET max_temp_directory_size = '<budget-disco>';
SET threads = <budget-cpu>;
SET preserve_insertion_order = false; -- quando semanticamente sicuro
```

Modalità da confrontare:

1. `:memory:` standard;
2. database in-memory con storage compresso;
3. database file-backed temporaneo mantenuto aperto e caldo;
4. modalità ibrida con catalogo file-backed e intermedi spillabili.

Un database file-backed non è automaticamente più lento: DuckDB può usare
compressione e buffer cache, mentre le tabelle in-memory non compresse possono
consumare più RAM ed essere meno veloci in alcuni workload analitici.

Il memory limit deve essere calcolato dal budget del run o del sistema, non
lasciato implicitamente all'80% della RAM. Come valore iniziale prudente si può
valutare il 50–60% della memoria disponibile, lasciando spazio a main process,
client Quack e runtime esterni.

Il main deve monitorare separatamente:

- RSS del main;
- RSS del sidecar;
- RSS dei runtime esterni;
- byte letti/scritti nella run directory;
- peak dello spill;
- spazio disco libero prima e durante il run.

## Cancellazione, crash e cleanup

Il main possiede sempre la responsabilità finale del cleanup perché un sidecar
terminato non può garantirlo.

Su Windows il run deve usare un Job Object con semantica equivalente a
`KILL_ON_JOB_CLOSE`. Su Linux/macOS deve usare un process group. Sidecar e
runtime figli associati al run non devono rimanere orfani.

Comportamenti richiesti:

- cancellazione intenzionale → stato `cancelled`;
- uscita inattesa del sidecar → stato `runner_crashed` con exit code e causa
  sanitizzata disponibile;
- perdita della connessione durante cancellazione → non trasformata in errore;
- perdita della connessione senza cancellazione → errore del run;
- directory residue dopo crash dell'intera applicazione → rimosse da uno
  sweeper al successivo avvio, usando run ID e TTL;
- output esplicitamente persistenti → mai rimossi dallo sweeper;
- spill, bootstrap, token e snapshot interni → sempre considerati temporanei.

## Sicurezza

Quack espone l'intera superficie SQL visibile al server. La feature deve quindi:

- bindare esclusivamente `127.0.0.1`/localhost;
- generare un token casuale distinto per run;
- non stampare token in command line, log, errori o `ready.json`;
- conservare il bootstrap file con permessi utente e cancellarlo appena letto;
- non esporre il server su `0.0.0.0`;
- mantenere TLS disabilitato per localhost e non supportare connessioni remote in
  questa feature;
- disabilitare o sanitizzare Quack/DuckDB query logging quando lo statement può
  contenere secret;
- evitare che connection string e token vengano inclusi nei SQL restituiti a
  UI, history, preview o MCP;
- trattare codice Python/Rust utente come trusted all'interno del singolo run,
  senza concedergli accesso ad altri run;
- impedire alias e identificatori SQL non quotati o non validati.

La risoluzione dei secret richiede una decisione specifica: inviare un ATTACH
con credenziali tramite il client lascia la stringa anche nel contesto DuckDB
locale. È preferibile consegnare al sidecar un bootstrap manifest protetto con
le risorse risolte, così il main invia successivamente solo resource ID e SQL
privo di credenziali. Questo punto è un gate di sicurezza, non un dettaglio.

## Observability

Il main continua a emettere gli eventi di pipeline perché conosce l'inizio e il
termine delle proprie richieste. Quack non deve diventare un secondo sistema di
eventi applicativo.

Le metriche server devono essere correlabili con:

- run ID;
- stage ID e attempt;
- Quack connection ID;
- client query ID;
- durata client e durata server;
- righe prodotte o trasferite;
- byte e batch Quack;
- memoria, spill e CPU sidecar;
- modalità di trasporto usata.

Il SQL sensibile deve essere sostituito da hash/fingerprint o testo redatto.
Quack offre log strutturati con connection/query ID, utili per correlare client
e server, ma la persistenza deve rispettare le regole sui secret.

## Packaging e distribuzione

Il nuovo sidecar è un componente obbligatorio, non opzionale.

### Desktop

- compilare `duckle-db-runner` prima del desktop;
- comprimerlo e incorporarlo usando il pattern già adottato per gli altri
  sidecar;
- estrarlo in una cache versionata e verificata;
- non scaricare più DuckDB CLI al termine della migrazione;
- aggiornare la schermata Engine Setup per distinguere disponibilità del
  sidecar, Quack e extension richieste.

### Runner headless e artifact

- `duckle-runner` diventa orchestratore e Quack client;
- l'artifact self-extracting include `duckle-db-runner` per l'OS target;
- il bundle include le extension Quack e connector necessarie oppure documenta
  chiaramente il requisito di rete;
- build cross-target deve produrre una coppia client/server con la stessa
  versione DuckDB/Quack.

### Costo binario

Il client Quack ufficiale è anch'esso DuckDB. Main e sidecar possono quindi
contenere due copie della libreria DuckDB, aumentando build time, dimensione
degli artifact, memoria di startup e superficie di aggiornamento. Questa
duplicazione è conseguenza della scelta Quack client/server e deve essere
misurata esplicitamente.

Non si deve implementare un client Quack proprietario basato sul wire format
interno: il protocollo è ancora beta e usa serializzazione DuckDB non pensata
come API indipendente stabile.

## Extension e compatibilità di versione

Client e server devono essere vincolati alla stessa versione DuckDB e Quack. La
feature deve verificare:

- disponibilità di Quack per Windows x64/arm64, Linux x64/arm64 e macOS
  universal/arch supportate;
- caricamento con `duckdb-rs` bundled;
- installazione offline o packaging dell'extension;
- compatibilità con extension core e community già usate da Duckle;
- comportamento dell'opzione unsigned e relativa superficie di sicurezza;
- aggiornamento atomico della coppia client/server;
- messaggio diagnostico chiaro in caso di mismatch.

Quack è attualmente beta e le funzioni o il protocollo possono cambiare. Tutte
le chiamate devono essere isolate in `QuackRunClient`, con protocol version
Duckle indipendente, per limitare il costo di un upgrade.

## Migrazione proposta

La rimozione della CLI deve essere incrementale.

### Fase 0 — ADR e spike

- approvare confini e ownership;
- disabilitare `xf.dbt`, dbt Fusion setup e ogni fallback automatico;
- costruire sidecar minimo con DuckDB embedded e Quack server;
- costruire client Rust Quack nel main;
- provare prewarm, lease esclusivo, coda bounded, kill-and-replace e crescita
  elastica su processi locali;
- provare startup, query verbatim, scrittura, letture parallele, append,
  attachment, spill e kill;
- misurare binari, memoria e latenza.

### Fase 1 — Astrazione `RunDatabase`

- introdurre `RunSession`, `WorkerPoolControl`, `WorkerProvider`, provider locale
  e `RunDatabase`;
- separare policy elastica pura, coda/admission e provisioning;
- mantenere il backend CLI come compatibility backend;
- migrare health, inspect, query, count, schema e preview;
- rendere il client DuckDB locale privato al modulo Quack.

### Fase 2 — Parità SQL sequenziale

- eseguire pipeline pure SQL nel sidecar;
- mantenere inizialmente scheduling sequenziale;
- migrare materializzazione, errori, history, partial run e preview;
- sostituire batch CLI e marker file;
- coprire Query Source senza whitelist affinity.

### Fase 3 — Runtime e trasporti

- sostituire `run_rows` e materializzazioni JSON con il nuovo contratto;
- implementare Quack streaming e Parquet fallback;
- migrare Python, JavaScript, WASM, connector Rust e sink;
- migrare `ctl.parallelize`, child pipeline e foreach;
- migrare gli strumenti attivi che richiedono un file DuckDB.

### Fase 4 — Concorrenza

- introdurre resource modes e connection pool;
- eseguire letture e append compatibili in parallelo;
- implementare retry deterministico dei conflitti;
- verificare fairness e limiti di connessioni/thread.

### Fase 5 — Packaging e rimozione CLI

- aggiornare desktop, runner artifact, scheduler, MCP, drift, branch e CI;
- distribuire sidecar ed extension per tutti i target;
- rimuovere download, setup e variabili della CLI;
- eliminare `AffinitySession` e il backend legacy soltanto dopo parità completa.

## Benchmark obbligatorio

### Varianti

| ID | Storage | IPC/trasporto |
|---|---|---|
| A | CLI file-backed attuale | stdin/file marker/Parquet |
| B | sidecar file-backed temporaneo | Quack |
| C | sidecar in-memory compresso | Quack |
| D | sidecar ibrido | Quack |
| E | sidecar | snapshot Parquet condiviso |

### Workload

- startup e shutdown per pipeline;
- 20, 50 e 100 stage SQL piccoli;
- scan da 1M, 10M e 100M righe;
- join, group by, window, sort e pivot;
- join/sort che forzano spill con budget da 512 MB e 1 GB;
- 2/4/8 lettori paralleli;
- 2/4/8 append sulla stessa tabella e su tabelle differenti;
- update concorrenti con e senza conflitto;
- fan-out verso 2/4/8 consumer;
- Quack streaming contro singolo Parquet riusato;
- Python/Rust che consumano interamente o parzialmente una relazione;
- Query Source con attachment live;
- partial run, preview e schema inspection;
- cancellazione durante scan, join, spill e trasferimento;
- crash sidecar e crash del parent;
- pipeline concorrenti, ognuna con il proprio sidecar;
- burst sopra capacità base e massima, attesa FIFO e timeout;
- worker avviati ma non ancora ready, failure di bootstrap e replenishment;
- kill-and-replace senza riuso di PID/worker ID;
- scale-in dopo un picco e rinnovo della finestra senza picchi storici sticky;
- simulazione del provider remoto con latenze e failure simili a Kubernetes.

### Misure

- wall time totale;
- latenza p50/p95 per stage;
- righe e byte al secondo;
- CPU main, sidecar e runtime;
- peak RSS separato per processo;
- byte letti/scritti su disco;
- peak e durata dello spill;
- volume trasferito su loopback;
- tempo di startup, cancel e cleanup;
- dimensione binari/artifact e build time;
- errori, conflitti e retry;
- correttezza di tipi, null, decimal, timestamp, nested e zero righe.

## Gate proposti

La feature può sostituire la CLI soltanto se:

1. tutti i test di comportamento esistenti applicabili passano sul backend
   Quack;
2. il main process non conserva dataset della pipeline e la sua RSS non cresce
   proporzionalmente al database, salvo preview/runtime espliciti;
3. un workload oltre il memory budget completa tramite spill senza OOM;
4. cancellazione termina il sidecar rapidamente e il cleanup conclude entro
   10 secondi negli OS supportati;
5. nessun token o secret sintetico compare in log, errori, history, query
   esportate o bootstrap residui;
6. la variante scelta non introduce regressioni bulk non approvate rispetto
   alla CLI e migliora in modo misurabile le pipeline con molti stage piccoli;
7. il punto di crossover Quack/Parquet è documentato e usato dalla policy;
8. tutti i consumer attivi del path `.duckdb` hanno una strategia funzionante;
9. client/server version mismatch fallisce prima di iniziare gli stage;
10. packaging desktop e runner headless funziona offline sui target supportati.

Le soglie percentuali definitive devono essere fissate dopo il benchmark
baseline, non inventate prima di misurare l'hardware reale.

## Rischi principali e mitigazioni

| Rischio | Impatto | Mitigazione proposta |
|---|---|---|
| Quack è beta e cambia protocollo/API | Alto | pin versione, wrapper unico, protocol version Duckle, feature flag e backend legacy durante migrazione |
| DuckDB completo anche nel main client | Alto su size/RSS | client senza dati, thread/memoria ridotti, misure separate, sidecar compresso; accettare la duplicazione come costo della scelta Quack |
| Query eseguita accidentalmente nel client | Critico | API che espone solo query verbatim remote, connessione grezza privata, test CPU/RSS e plan placement |
| Mismatch client/server/extension | Alto | coppia build atomica, handshake versione prima del run |
| Quack non disponibile offline | Alto | bundle dell'extension o build verificata; nessun affidamento silenzioso su autoinstall |
| Attachment connection-local | Alto | test esplicito, initializer idempotente per connessione, lease pinned quando necessario |
| Pipeline esistenti contengono `xf.dbt` | Medio | documenti leggibili, validazione `component_disabled`, nessun fallback silenzioso |
| Secret presenti nel SQL client | Critico | bootstrap protetto delle risorse e attach server-side, logging disabilitato/redatto |
| Default authorization Quack permissiva | Medio/alto | localhost, token per-run, processo effimero, auth callback se runtime non trusted |
| Write conflict tra connessioni | Medio | access modes, retry limitato e diagnostica deterministica |
| Sidecar orfano o runtime figlio orfano | Alto | Job Object/process group, parent PID monitor, startup sweeper |
| Kill lascia spill o snapshot | Medio | ownership cleanup nel main, directory per-run, TTL sweeper |
| Disco esaurito dallo spill | Alto | max temp size, controllo spazio libero, evento e errore specifico |
| `:memory:` usa troppa RAM o è più lento | Alto | confronto con file-backed compresso; nessuna scelta aprioristica |
| Quack streaming ripete trasferimenti fan-out | Medio | Parquet snapshot riusabile e policy basata su benchmark |
| Regressioni su connector/runtime numerosi | Alto | compatibility backend, migrazione per famiglia, suite di parità |
| Aumento build time e artifact size | Medio/alto | profilo sidecar dedicato, compressione, cache CI, misure come release gate |
| Antivirus/firewall interferisce con sidecar/localhost | Medio | binary firmato, path stabile, localhost only, errori di bootstrap chiari |
| Porta occupata o race di startup | Medio | bind port 0 nel sidecar, `ready.json` atomico, retry bounded |
| Crash C++/extension | Medio | isolamento sidecar; main resta vivo e registra `runner_crashed` |
| Due pipeline concorrenti competono per RAM/disco | Alto | `max_capacity`, budget globale riservato anche a `starting`, profilo per-worker e admission queue |
| Crescita elastica senza limite causa OOM o thrashing | Critico | nessun bypass on-demand, hard cap RAM/CPU/disco, hysteresis, cooldown e backpressure |
| Picco storico impedisce lo scale-in | Medio | finestre tumbling/mobili rinnovate anche quando non avviene una riduzione |
| Due scheduler assegnano lo stesso worker | Critico | transizione atomica ready→leased; single writer locale, CAS/Lease/CRD in deployment distribuito |
| Worker dichiarato pronto troppo presto | Alto | readiness infrastrutturale più handshake Quack applicativo prima della pubblicazione |
| Provider locale entra nel dominio | Alto | handle opaco e `WorkerProvider`; nessun PID, porta o path nell'API di scheduler |
| Kubernetes ripristina un Pod cancellato in modo inatteso | Alto | terminare il Job proprietario, control plane proprietario del desired target, retry/finalizer chiari e test delete-to-cancel |
| HPA e pool policy si contendono le repliche | Alto | un solo proprietario del target; HPA non governa direttamente i worker leased |

## Ambito incluso

- nuovo binario sidecar DuckDB embedded;
- Quack client nel main e Quack server nel sidecar;
- lifecycle per-run, health, version handshake e cleanup;
- pool prewarm elastico bounded, coda di admission e lease esclusivo single-use;
- astrazione `WorkerProvider` e prima implementazione per processi locali;
- database unico per pipeline;
- memory budget e spill;
- migrazione dell'interfaccia `DuckdbEngine` verso `RunDatabase`;
- pipeline desktop, headless, scheduled e invocate da MCP;
- inspect, preview, partial run, history e cancellazione;
- trasporto Quack più fallback Parquet;
- parallelismo sicuro multi-connessione;
- disabilitazione di `xf.dbt` e rimozione del relativo provisioning automatico;
- packaging multipiattaforma e aggiornamento CI;
- deprecazione e rimozione finale della CLI.

## Fuori ambito

- server Quack condiviso da più run;
- esposizione pubblica o Internet di Quack; anche il futuro traffico in-cluster
  richiederà una decisione di sicurezza separata;
- esecuzione distribuita su più macchine;
- implementazione del provider Kubernetes, del relativo controller/CRD e del
  deployment multi-replica; la compatibilità del contratto è invece inclusa;
- database persistente gestito come servizio Duckle;
- transazioni distribuite tra sistemi esterni;
- cancellazione di un singolo statement mantenendo viva la pipeline;
- rimozione assoluta di Parquet;
- migrazione o riattivazione di dbt;
- implementazione del nodo Query multi-input della Feature 002;
- autorizzazione multi-tenant o sandbox completa del codice utente.

## Impatto sui moduli

| Area | Impatto previsto |
|---|---|
| `crates/duckdb-engine` | separare planner/orchestrator dal backend CLI; introdurre `RunDatabase`; migrare helper e runtime |
| nuovo `crates/duckle-db-runner` | DuckDB embedded, configurazione, Quack server, system schema e lifecycle |
| workspace Cargo | versione DuckDB/Quack pin, feature bundled, profili build |
| `apps/desktop` | supervisor, process handle, staging sidecar, Engine Setup, cancel |
| `crates/duckle-runner` | Quack client, bundle sidecar, self-extract, serve mode, branch/drift |
| `crates/scheduler` | sidecar per run e budget concorrenti |
| `crates/duckle-mcp` | usare il nuovo factory/session contract |
| planner/affinity | resource requirements e connection lease al posto della matrice component ID |
| connector/runtime | import/export/stream al posto di db path e helper CLI |
| `crates/slothdb-engine` | engine disabilitato; nessuna migrazione Quack in questa feature |
| `xf.dbt` / Engine Setup | nodo e installazione dbt Fusion disabilitati; compatibilità documentale preservata |
| CI/release | build sidecar, package Quack, rimuovere gradualmente install CLI |
| documentazione | nuovo ADR e supersessione dell'ADR affinity CLI |

## Impatto sulla Feature 002

La nuova infrastruttura non implementa automaticamente Query Source universale
o Query multi-input, ma ne stabilisce il fondamento.

Dopo questa feature la 002 potrà assumere:

- un catalogo del run condiviso e stabile;
- output dei nodi disponibili come relazioni nominate;
- query multi-input eseguite interamente nel sidecar;
- join fra Query Source, Source DuckDB e risultati intermedi;
- connessioni concorrenti con resource lease;
- affinity basata su risorse, non component ID;
- runtime esterni collegabili tramite Quack o snapshot;
- cancellazione dell'intero run tramite terminazione sidecar.

La 002 dovrà essere riscritta soltanto dopo la Fase 2 per il contratto base e
dopo gli spike di Fase 3/4 per parallelismo e runtime esterni. Implementarla
sull'attuale `AffinitySession` produrrebbe lavoro destinato a essere rimosso.

## Deliverable prima della specifica implementativa

1. Brownfield impact report aggiornato con tutti i chiamanti CLI.
2. ADR “DuckDB per-run sidecar con Quack”.
3. Prototipo client/server multipiattaforma.
4. Report benchmark Quack vs Parquet e memory vs file-backed.
5. Strategia per i consumer attivi del db path.
6. Threat model per token, secret bootstrap e codice utente.
7. Decisione su packaging extension e version pin.
8. Piano di migrazione per famiglie di RuntimeSpec.
9. Elenco test di parità e release gate.

Solo dopo questi deliverable la proposta deve essere trasformata in una nuova
feature Spec Kit con requisiti e task eseguibili.

## Riferimenti tecnici

- [DuckDB Quack overview](https://duckdb.org/docs/current/quack/overview)
- [DuckDB Quack reference](https://duckdb.org/docs/current/quack/reference)
- [DuckDB Quack security](https://duckdb.org/docs/current/quack/security)
- [DuckDB Quack extension status](https://duckdb.org/docs/current/core_extensions/quack)
- [DuckDB concurrency](https://duckdb.org/docs/current/connect/concurrency)
- [DuckDB Rust client](https://duckdb.org/docs/current/clients/rust)
- [DuckDB workload tuning](https://duckdb.org/docs/current/guides/performance/how_to_tune_workloads)
- [DuckDB out-of-memory guidance](https://duckdb.org/docs/current/guides/performance/oom)
- [Kubernetes Jobs](https://kubernetes.io/docs/concepts/workloads/controllers/job/)
- [Kubernetes probes](https://kubernetes.io/docs/concepts/workloads/pods/probes/)
- [Kubernetes Leases](https://kubernetes.io/docs/concepts/architecture/leases/)
- [Kubernetes ephemeral volumes](https://kubernetes.io/docs/concepts/storage/ephemeral-volumes/)
- [Kubernetes resource management](https://kubernetes.io/docs/concepts/configuration/manage-resources-containers/)
- [Intento funzionale Feature 002](002-universal-query-source-and-multi-input-query.md)
- [ADR affinity CLI corrente](../architecture/adr-affinity-session.md)
