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
7. Le query complete vengono spedite verbatim al server esclusivamente con
   `quack_query(uri, sql)`, così join e materializzazioni restano remoti.
   `remote.query(...)` e `quack_query_by_name(...)` richiedono il client
   `ATTACH TYPE quack` sticky e non fanno parte del contratto ordinario.
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
15. I sidecar vengono acquisiti tramite lease esclusivi da `WorkerPoolControl`:
    una pipeline run usa un solo worker e un worker serve una sola pipeline run.
16. Il worker viene sempre terminato a fine run o cancellazione. Non torna mai
    in `WorkerPoolControl`; un sostituto entra nella coda `ready` soltanto dopo il nuovo
    handshake DuckDB/Quack autenticato e dopo la preparazione della capacità
    base di worker. Non esiste un pool di connessioni.
17. Policy elastica, coda di admission e provisioning sono separati. Il backend
    iniziale avvia processi locali, ma il contratto deve consentire in futuro un
    backend Kubernetes senza cambiare scheduler, `RunSession` o `RunDatabase`.
18. Esiste un solo pool elastico: `WorkerPoolControl`, quello globale dei worker
    warm. Ogni worker possiede un `QuackPermitGate` di concorrenza configurabile;
    non esiste un secondo pool di connessioni.
19. Il percorso caldo di uno stage non esegue mai `ATTACH`: acquisisce un
    permit, esegue il poco costoso `try_clone()`, invia `quack_query`, quindi
    rilascia clone e permit. Se tutti i permit sono occupati, la richiesta
    attende.
20. `quack_parallelism` è configurabile in `WorkerSpec` per worker e ha default
    e limite massimo di Fase 1 `8` (range valido `1..=8`). `QuackPermitGate`
    è FIFO e cancellabile: se saturo, attende senza creare clone; la
    cancellazione rimuove l'attesa. Il worker non precrea clone né alias
    `ATTACH`. Il valore non modifica `SET threads`, che governa il parallelismo
    interno di ogni singola query.

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
- applicare il semaforo `quack_parallelism` e creare clone on demand;
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

L'unico pool elastico è `WorkerPoolControl`, il pool dei worker. `ready` non significa che il
listener HTTP è soltanto raggiungibile: il worker pubblicabile contiene
sidecar, database client/master mantenuta viva, secret scoped e una health query
stateless autenticata riuscita. `quack_parallelism = 8` (default) è un limite
runtime a semaforo; readiness non precrea le 8 clone e non crea alias `ATTACH`.

1. `WorkerPoolControl` avvia in parallelo la capacità base.
2. La readiness infrastrutturale del sidecar non rende il worker acquisibile.
3. Il control plane crea e conserva la master, configura il secret scoped e
   completa una health query stateless autenticata.
4. Solo il bundle completo passa a `ready`; una richiesta di run entra in una
   coda FIFO cancellabile e con timeout.
5. L'acquisizione cambia atomicamente `ready -> leased` e lega worker ID, run ID
   e lease ID. Anche due esecuzioni della stessa pipeline ricevono worker
   diversi.
6. Alla conclusione o cancellazione, il worker viene terminato e non riusato.
7. La policy ricalcola il target; se manca capacità, prepara un nuovo worker e
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

`WorkerPoolControl` usa un'interfaccia asincrona concettualmente equivalente a:

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

1. Il provider crea identità e storage univoci per il worker.
2. Genera una credenziale casuale per-worker e la conserva in un handle opaco.
3. Il provider locale crea bootstrap e control pipe anonime; il child eredita
   soltanto gli handle dichiarati esplicitamente.
4. Il token e la configurazione attraversano la bootstrap pipe, mai command
   line, environment, stdin generico o filesystem.
5. Il sidecar apre DuckDB, effettua direttamente il bind su
   `127.0.0.1:0` e avvia Quack.
6. Endpoint, identità e versioni ritornano sulla control pipe senza token; il
   target produttivo non usa `ready.json`.
7. Il main crea e conserva il database client e la master tramite il
   `CredentialHandle`; verifica identità, protocol version e query minima
   autenticata.
8. Solo dopo il gate autenticato il worker cambia `starting -> ready`; le clone
   vengono create on demand sotto il semaforo `quack_parallelism`.

Il canale di bootstrap/control è un meccanismo di lifecycle provider-specific,
non una seconda API dati. Il contratto normativo è descritto nell'ADR
[identità, bootstrap e sicurezza dei worker](../architecture/adr-worker-identity-bootstrap-security.md).

### Esecuzione

Il main mantiene `RunSession`:

```text
RunSession
  ├─ run_id
  ├─ run_directory
  ├─ worker_lease
  ├─ worker_endpoint
  ├─ cancellation_state
  ├─ QuackRunClient + QuackPermitGate (non è un pool)
  └─ runtime_process_scope
```

Ogni operazione DuckDB passa attraverso un'interfaccia tipizzata, per esempio:

```rust
trait RunDatabase {
    async fn execute_sql(&self, request: SqlRequest) -> Result<SqlOutcome>;
    async fn query_sql(&self, request: QueryRequest) -> Result<QueryResult>;
    async fn describe_relation(&self, relation: RelationRef) -> Result<Schema>;
    async fn preview(&self, relation: RelationRef, limit: usize) -> Result<Preview>;
    async fn import(&self, input: RelationInput, target: RelationRef) -> Result<ImportResult>;
    async fn export(&self, source: RelationRef, format: TransferFormat) -> Result<Artifact>;
    async fn metrics(&self) -> Result<RunDatabaseMetrics>;
}
```

L'implementazione primaria è `QuackRunDatabase`; durante la migrazione può
esistere un backend legacy CLI dietro la stessa interfaccia.

`RunDatabase` sostituisce esclusivamente il meccanismo di esecuzione a basso
livello. Planner e orchestratore restano proprietari dell'ordine, del batching,
del parallelismo, degli eventi per-stage e dei retry. Se l'orchestratore invia
una singola query, il backend esegue quella query; se invia un batch SQL già
formato, il backend esegue quel batch sullo stesso lease. Il backend Quack non
deve ricevere anticipatamente l'intera pipeline, fondere richieste, dividerle o
riordinarle. Gli eventuali marker CLI non rappresentabili come SQL vengono
sostituiti nell'adapter mantenendo invariati gli eventi osservabili per-stage.
Lo spike conferma inoltre che Quack accetta un batch remoto multi-statement:
due materializzazioni dipendenti e il `SELECT` finale sono stati eseguiti in
un'unica invocazione, con risultato finale corretto. Non dimostra però
atomicità transazionale, rollback dopo errore intermedio, compatibilità dei
marker, preview o attribuzione dell'errore allo stage; tutti restano test
obbligatori di Fase 2.

### Conclusione normale

1. Il main attende tutti gli stage.
2. Legge metriche finali e stato del catalogo.
3. Rilascia il lease con esito e metriche finali.
4. Il provider termina sempre quel worker entro un timeout breve.
5. Se la chiusura cooperativa non riesce, applica la terminazione forzata.
6. Pulisce storage, spill e snapshot non persistenti.
7. `WorkerPoolControl` prepara un sostituto se il target elastico lo richiede.

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

### Due risorse chiamate `ATTACH`

La parola `ATTACH` non identifica una sola operazione e non deve comparire come
un unico tipo nel nuovo runner:

| Tipo | Esempio | Proprietario | Decisione |
|---|---|---|---|
| trasporto Quack client | `ATTACH 'quack:…' AS remote (TYPE quack)` | DuckDB client nel main | scartato dal percorso ordinario: si usa `quack_query` stateless |
| Data Source server | `ATTACH '<DSN>' AS sales (TYPE postgres)` o `ATTACH 'ducklake:…' AS lake` | DuckDB nel sidecar | deciso da planner/orchestratore e inviato al sidecar prima del nodo dipendente |

Il primo crea una sessione Quack sticky nel processo main e non deve essere
confuso con il secondo, che collega una risorsa dati reale al catalogo del
sidecar. Alias, cache, idempotenza, segreti e lifecycle sono distinti.

Il planner/orchestratore continua a risolvere le Data Source e a produrre gli
attach/prelude già presenti nel piano Query Source. Il nuovo `RunDatabase` non
deduce attach dal testo SQL e non li esegue localmente: riceve un
`ServerSetupAction` già risolto, lo invia verbatim con `quack_query` e lo
serializza/deduplica per `(worker_run, resource_id, alias)` prima dello stage
che lo richiede. Questa è una barriera di setup, non una nuova decisione di
scheduling: l'orchestratore conserva ordine, batch, eventi e parallelismo.

S3 non è normalmente un `ATTACH` dati: richiede `CREATE SECRET (TYPE s3)` e
configurazione `httpfs` nel sidecar, poi funzioni come
`read_parquet('s3://…')`. DuckLake, Postgres, MySQL e cataloghi/file DuckDB o
SQLite usano invece il loro `ATTACH` server-side quando previsto dall'extension.

Durante il prewarm il main apre un solo database DuckDB client per il worker e
mantiene viva una connessione master fino alla terminazione del worker. Crea un
secret temporaneo scoped all'endpoint e verifica una health query autenticata.
Non crea `ATTACH` né clone persistenti durante il prewarm:

```sql
CREATE TEMPORARY SECRET run_quack_secret (
    TYPE quack,
    TOKEN ?,
    SCOPE 'quack:127.0.0.1:<port>'
);

SET httpfs_connection_caching = true;
```

Il secret viene creato in-memory con parameter binding. Per ogni richiesta il
wrapper acquisisce un permit dal semaforo `quack_parallelism`, crea una
connessione con `Connection::try_clone()`, invia lo SQL verbatim con
`quack_query`, quindi distrugge la clone e rilascia il permit:

```sql
FROM quack_query('quack:127.0.0.1:<port>', $sql$
    CREATE OR REPLACE TABLE run_data.stage_10 AS
    SELECT ...
$sql$);
```

Il token non compare nella chiamata: `quack_query` lo risolve dal secret scoped
parametrizzato. Con il default `quack_parallelism = 8` possono esistere al
massimo 8 clone/query contemporanee; la nona richiesta attende un permit.

L'API pubblica di `QuackRunDatabase` espone questo solo primitivo remoto:
`execute_stateless(sql)` / `query_stateless(sql)`. Non espone
`remote.query(...)`, `quack_query_by_name(...)` né il catalogo DuckDB client;
questo rende impossibile reintrodurre per errore un transport `ATTACH` sticky o
un piano federato locale.

Lo spike ha verificato la persistenza necessaria: esegue un `ATTACH` DuckDB
server-side tramite una richiesta `quack_query` stateless, poi legge dalla
tabella attachata con una seconda richiesta stateless e ottiene il valore atteso
`42`. L'attach dati resta quindi nel catalogo del sidecar per il run anche se la
connessione Quack che lo ha inviato non è sticky. La gestione di conflitti e
deduplica resta nella barriera `ServerSetupAction` dell'orchestratore.

Questo vincolo è fondamentale. Eseguire nel client una query che combina
tabelle remote potrebbe spostare operatori nel processo principale. Il wrapper
`QuackRunClient` deve quindi rendere pubblico `execute_stateless`, non la
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

Le tabelle condivise fra richieste devono vivere in `run_data`. Gli oggetti
`TEMP` e le impostazioni `SET` stateless vivono soltanto dentro la singola
richiesta; quando più statement devono condividerli, l'orchestratore li invia
nello stesso batch. Non possono costituire il contratto fra due chiamate stage.

## `QuackPermitGate` e parallelismo

Il sidecar contiene una sola istanza DuckDB, ma ogni lavoro concorrente usa una
richiesta server distinta. Il main acquisisce un permit, crea una clone locale,
esegue con `quack_query` la singola richiesta o il batch deciso
dall'orchestratore, quindi distrugge la clone e rilascia il permit.

Lo spike del 2026-07-17 ha caratterizzato la semantica che il progetto non deve
più assumere implicitamente:

| Configurazione client | 2 × 250 ms | 4 × 250 ms | 8 × 250 ms | ID a 8 |
|---|---:|---:|---:|---:|
| clone, stesso alias `ATTACH` | 517–527 ms | 1.023–1.046 ms | 2.069–2.070 ms | 1 |
| clone, alias `ATTACH` distinti | 262–267 ms | 266–268 ms | 270–271 ms | 8 |
| clone, `quack_query` stateless | 265–266 ms | 266–269 ms | 272–277 ms | 8 |
| database client indipendenti | 262–275 ms | 264–275 ms | 282–286 ms | 8 |

Pertanto `try_clone()` con un solo `ATTACH` eredita correttamente catalogo e
stato sticky, ma non costituisce un meccanismo concorrente: tutte le query dirette a
quell'alias condividono la stessa connessione server e vengono serializzate.
La soluzione scelta mantiene un solo database/master client e usa
`quack_query` stateless. Database client separati e alias preattachati
funzionano, ma non sono necessari. Esiste un solo pool elastico,
`WorkerPoolControl`; dentro il worker esiste soltanto `QuackPermitGate`.

La capacità `N` è `WorkerSpec.quack_parallelism`, configurabile nel range
`1..=8`, con default e massimo di Fase 1 `8`; limita quante
richieste già rese concorrenti dall'orchestratore possono occupare il sidecar,
senza introdurre nuove decisioni di scheduling. `SET threads = M` governa
invece il parallelismo interno di DuckDB per una query e non sostituisce le `N`
sessioni Quack necessarie per eseguire più stage contemporaneamente.

Il worker non precrea connessioni. Con il default sono disponibili 8 permit;
una nona richiesta attende in FIFO e può essere cancellata. Ogni clone nasce
dopo l'acquisizione del permit e viene distrutta al termine; non viene eseguito
alcun `ATTACH`. Non esistono membri del gate da sostituire né crescita eager:
un errore di clone/query rilascia sempre il permit e torna tipizzato
all'orchestratore, che decide l'eventuale retry. Un crash sidecar rende il
worker non sano; la sua sostituzione appartiene a `WorkerPoolControl`.

### Stato `TEMP` e modello threading

Il contratto stateless sceglie esplicitamente che `TEMP` e `SET` non possano
attraversare due unità di esecuzione. Quando servono a più statement,
l'orchestratore mantiene quegli statement nello stesso batch remoto
`RequestLocal`; tra stage il solo contratto è una relazione regolare in
`run_data`. Non esistono nel primo rilascio lease connection-pinned, affinity
key o riaquisizione di una stessa connessione.

`duckdb-rs::Connection` è `Send`, ma non `Sync`. Il runner deve quindi:

1. mantenere la master in possesso esclusivo di un solo factory/control thread
   bloccante, senza `Arc<Connection>`;
2. al permit acquisito, chiedere a quel thread un `try_clone()` serializzato;
3. trasferire la clone a un solo query worker bloccante, proprietario esclusivo
   fino alla fine di `quack_query`;
4. droppare la clone prima di rilasciare il permit.

L'API async comunica con questi worker tramite messaggi; non può eseguire le
query bloccanti direttamente sul runtime async né condividere una `Connection`.
La cancellazione termina il sidecar e abbandona il lavoro client pendente senza
attendere una chiusura cooperativa su una connessione condivisa.

Il costo di bootstrap è stato misurato separatamente su tre run profilati
Windows x64. Con sidecar solo infrastrutturalmente pronto, il primo `ATTACH`
della master richiede 1,11–1,21 s e l'intero bootstrap client negli ultimi tre
run 1,24–1,27 s. Partendo dalla master già autenticata, 60 sequenze esatte
`try_clone -> ATTACH sulla clone -> prima query` hanno prodotto:

| Operazione | Intervallo osservato | Mediana per run |
|---|---:|---:|
| `try_clone()` | 18–96 µs | 22–28 µs |
| `ATTACH` sulla clone | 6,37–12,10 ms | 8,28–10,08 ms |
| prima query | 2,10–4,59 ms | 2,73–2,99 ms |
| totale fino alla prima query riuscita | 8,99–15,39 ms | 10,75–13,61 ms |

Ogni run ha osservato 20 `quack_connection_id` distinti per 20 nuovi slot e la
clone ha riusato correttamente il secret temporaneo parametrizzato della
master. Un precedente campione non profilato del bootstrap completo ha
raggiunto 3,39 s, quindi questi valori descrivono lo spike locale e non sono
ancora uno SLO.

Il costo di `try_clone()` è trascurabile rispetto alla query e non giustifica
prewarm. Gli `ATTACH` misurati appartengono all'alternativa sticky scartata: il
percorso selezionato crea la clone on demand e usa direttamente `quack_query`.
Non viene creato un secondo pool di connessioni.

Lo scheduler deve ragionare per modalità di accesso dichiarata:

| Modalità | Esempio | Scheduling iniziale |
|---|---|---|
| `SharedRead` | scan, preview, describe | parallelo |
| `Append` | append alla stessa tabella | parallelo con verifica conflitti |
| `Mutation` | insert/update/delete | parallelo solo se compatibile, altrimenti retry |
| `SchemaChange` | create/replace/drop/attach | lease esclusivo sulla risorsa interessata |
| `RequestLocal` | TEMP/SET necessari a più statement | stesso batch remoto |
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
- se richiede più statement nella stessa richiesta stateless;
- quali lease sono condivisi o esclusivi.

Ogni richiesta stateless deve inizializzare in modo idempotente le risorse
locali che le servono. Lo stato `TEMP` o `SET` necessario a più statement deve
restare nello stesso batch remoto; il contratto generico fra richieste usa
relazioni regolari in `run_data`.

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

Il connector Duckle può trasferire una relazione tramite Quack senza
materializzarla nel main. Python, Rust o altro codice utente non ricevono il
`CredentialHandle` primario del worker. Una connessione diretta da un runtime è
ammessa soltanto dopo aver introdotto una capability delegata, distinta,
revocabile e limitata; con l'autenticazione Quack default questo percorso resta
disabilitato. In alternativa il runtime usa un broker controllato o snapshot.

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

Il contratto normativo completo è l'ADR
[identità, bootstrap e sicurezza dei worker](../architecture/adr-worker-identity-bootstrap-security.md).
Quack espone l'intera superficie SQL visibile al server. La feature deve quindi:

- bindare esclusivamente `127.0.0.1`/localhost;
- generare un token CSPRNG distinto per worker single-use;
- consegnarlo soltanto attraverso handle anonimi ereditati esplicitamente;
- non serializzarlo in command line, environment, filesystem, log, errori,
  history, eventi UI, SQL esportato o readiness metadata;
- rappresentare endpoint, identità, credenziale e sicurezza del trasporto con
  un unico `VerifiedWorker` provider-neutral;
- pubblicare il worker soltanto dopo handshake Quack autenticato;
- non esporre il server su `0.0.0.0`;
- mantenere TLS disabilitato per localhost e non supportare connessioni remote in
  questa feature;
- disabilitare o sanitizzare Quack/DuckDB query logging quando lo statement può
  contenere secret;
- evitare che connection string e token vengano inclusi nei SQL restituiti a
  UI, history, preview o MCP;
- non consegnare il raw `CredentialHandle` a Python/Rust o altro codice utente;
  l'accesso avviene tramite API controllate, stream o snapshot;
- impedire alias e identificatori SQL non quotati o non validati.

Il profilo corrente è `execution_trusted_full_sql_v1`: autenticazione con
capability per-worker e autorizzazione full SQL impostata esplicitamente. Il
full SQL è necessario al supervisor per eseguire DDL, DML, ATTACH e query della
pipeline, ma è sicuro soltanto perché endpoint e credenziale restano confinati
al processo trusted. Non è un profilo read-only e non può essere riutilizzato
per browser, plugin o codice utente.

Il client deve usare `CREATE TEMPORARY SECRET` scoped all'endpoint e
`quack_query(uri, sql)` senza `TOKEN` inline. La connessione raw resta privata a
`QuackRunClient`; l'API pubblica invia soltanto SQL completo stateless. Questo
impedisce che planner o componenti costruiscano accidentalmente query federate
eseguite nel DuckDB client.

La risoluzione dei secret delle Data Source richiede una decisione specifica:
inviare un ATTACH con credenziali tramite il client lascia la stringa anche nel
contesto DuckDB locale. È preferibile consegnare al sidecar le risorse risolte
nel payload della bootstrap pipe o tramite un successivo canale di controllo
autenticato, così il main invia solo resource ID e SQL privo di credenziali.
Questi secret sono distinti dalla capability Quack del worker e seguono lo
stesso divieto di persistenza e logging. Questo punto è un gate di sicurezza,
non un dettaglio.

### Vincolo futuro: pubblicazione dei Books

La futura funzionalità Book è soltanto un vincolo architetturale e resta fuori
dall'ambito di questa feature. Un Book pubblicato potrà essere raggiunto, per
esempio, sotto `localhost/<book-name>/...` e potrà in futuro usare
DuckDB-Wasm/Quack per interrogare risultati prodotti dalle pipeline.

Questa possibilità non autorizza il riuso del worker di esecuzione:

- un Book non riceve mai endpoint o capability `execution_trusted_full_sql_v1`;
- publication plane e execution plane hanno processi, database, lifecycle,
  budget e profili di sicurezza distinti;
- vengono pubblicate soltanto relazioni approvate o snapshot dedicati, non il
  catalogo vivo della pipeline;
- un accesso browser richiede reverse proxy, TLS fuori dal puro localhost,
  policy CORS/same-origin e credenziali limitate e revocabili;
- l'autorizzazione Quack predefinita permissiva non è accettabile;
- una regex sul prefisso SQL non costituisce enforcement read-only: servirà un
  gateway di query approvate o una authorization function che analizzi gli
  statement, più sandbox di filesystem, extension e rete;
- il token esposto al browser, se previsto, deve essere una capability breve e
  limitata al Book, mai il token del database runner.

Routing dei Book, manifest UI, eventi, pulsanti che avviano pipeline,
navigazione e publication lifecycle saranno oggetto di una feature separata.

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
- caratterizzare clone, alias `ATTACH` e `quack_query` stateless con parallelismo
  2, 4 e 8;
- provare prewarm, lease esclusivo, coda bounded, kill-and-replace e crescita
  elastica su processi locali;
- provare startup, query verbatim, scrittura, letture parallele, append,
  attachment, spill e kill;
- misurare binari, memoria e latenza.

### Fase 1 — Astrazione `RunDatabase`

- introdurre `RunSession`, `WorkerPoolControl`, `WorkerProvider`, provider locale
  e `RunDatabase`;
- preservare senza modifiche le decisioni correnti dell'orchestratore su query
  singole, batch, parallelismo ed eventi per-stage;
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

- introdurre resource modes e semaforo `quack_parallelism`;
- creare una clone on demand per permit e inviare con `quack_query` stateless;
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
| Clone multipli sullo stesso alias Quack serializzano | Alto | percorso ordinario stateless con `quack_query`, zero alias client, semaforo `quack_parallelism`; test su `quack_connection_id` |
| Pipeline esistenti contengono `xf.dbt` | Medio | documenti leggibili, validazione `component_disabled`, nessun fallback silenzioso |
| Secret presenti nel SQL client | Critico | `CredentialHandle` opaco, secret scoped in-memory, verifica parameter binding, logging/profiling disabilitato e test di redazione |
| Default authorization Quack permissiva | Medio/alto | localhost, token CSPRNG per-worker single-use, nessuna esposizione a user code, auth callback ulteriore se richiesto dal threat model |
| Profilo execution riusato per Books/browser | Critico | profili e processi separati; mai esporre il worker vivo o la sua capability; publication feature con threat model dedicato |
| Write conflict tra connessioni | Medio | access modes, retry limitato e diagnostica deterministica |
| Sidecar orfano o runtime figlio orfano | Alto | Job Object/process group, parent PID monitor, startup sweeper |
| Kill lascia spill o snapshot | Medio | ownership cleanup nel main, directory per-run, TTL sweeper |
| Disco esaurito dallo spill | Alto | max temp size, controllo spazio libero, evento e errore specifico |
| `:memory:` usa troppa RAM o è più lento | Alto | confronto con file-backed compresso; nessuna scelta aprioristica |
| Quack streaming ripete trasferimenti fan-out | Medio | Parquet snapshot riusabile e policy basata su benchmark |
| Regressioni su connector/runtime numerosi | Alto | compatibility backend, migrazione per famiglia, suite di parità |
| Aumento build time e artifact size | Medio/alto | profilo sidecar dedicato, compressione, cache CI, misure come release gate |
| Antivirus/firewall interferisce con sidecar/localhost | Medio | binary firmato, path stabile, localhost only, errori di bootstrap chiari |
| Porta occupata o race di startup | Medio | bind diretto `127.0.0.1:0` nel sidecar e endpoint restituito sulla control pipe |
| Token osservabile nel bootstrap | Critico | pipe anonima ereditata tramite handle allowlist; niente argv, environment, file, stdin generico o readiness metadata |
| Processo sibling legge memoria/handle | Alto | DACL processo, divieto handle generici, Job Object e threat model esplicito; admin/debugger privilegiato resta rischio residuo |
| Crash C++/extension | Medio | isolamento sidecar; main resta vivo e registra `runner_crashed` |
| Due pipeline concorrenti competono per RAM/disco | Alto | `max_capacity`, budget globale riservato anche a `starting`, profilo per-worker e admission queue |
| Crescita elastica senza limite causa OOM o thrashing | Critico | nessun bypass on-demand, hard cap RAM/CPU/disco, hysteresis, cooldown e backpressure |
| Picco storico impedisce lo scale-in | Medio | finestre tumbling/mobili rinnovate anche quando non avviene una riduzione |
| Due scheduler assegnano lo stesso worker | Critico | transizione atomica ready→leased; single writer locale, CAS/Lease/CRD in deployment distribuito |
| Worker dichiarato pronto troppo presto | Alto | readiness infrastrutturale più handshake Quack applicativo prima della pubblicazione |
| Provider locale entra nel dominio | Alto | handle opaco e `WorkerProvider`; nessun PID, porta o path nell'API di scheduler |
| Kubernetes ripristina un Pod cancellato in modo inatteso | Alto | terminare il Job proprietario, control plane proprietario del desired target, retry/finalizer chiari e test delete-to-cancel |
| HPA e `WorkerPoolControl` si contendono le repliche | Alto | un solo proprietario del target; HPA non governa direttamente i worker leased |

## Ambito incluso

- nuovo binario sidecar DuckDB embedded;
- Quack client nel main e Quack server nel sidecar;
- lifecycle per-run, health, version handshake e cleanup;
- `WorkerPoolControl` prewarm elastico bounded, coda di admission e lease esclusivo single-use;
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
- implementazione, routing e pubblicazione dei Books;
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

## Disciplina di documentazione

Ogni prova che conferma, smentisce o restringe un'ipotesi di questa feature deve
aggiornare nello stesso change set:

1. il contratto normativo in questo feature intent;
2. la decisione e le conseguenze nell'ADR pertinente;
3. comando, ambiente, metodo, risultati e limiti nel report dello spike;
4. il README del test quando serve a rendere la prova riproducibile.

Un risultato osservato soltanto in console o riportato in conversazione non è
considerato una decisione architetturale acquisita.

## Deliverable prima della specifica implementativa

1. Brownfield impact report aggiornato con tutti i chiamanti CLI.
2. ADR “DuckDB per-run sidecar con Quack”.
3. Prototipo client/server multipiattaforma.
4. Report benchmark Quack vs Parquet e memory vs file-backed.
5. Strategia per i consumer attivi del db path.
6. ADR e threat model per identità, token, bootstrap, transport security e
   codice utente.
7. Decisione su packaging extension e version pin.
8. Piano di migrazione per famiglie di RuntimeSpec.
9. Elenco test di parità e release gate.

Solo dopo questi deliverable la proposta deve essere trasformata in una nuova
feature Spec Kit con requisiti e task eseguibili.

## Riferimenti tecnici

- [DuckDB Quack overview](https://duckdb.org/docs/current/quack/overview)
- [DuckDB Quack reference](https://duckdb.org/docs/current/quack/reference)
- [DuckDB Quack security](https://duckdb.org/docs/current/quack/security)
- [DuckDB Quack reverse proxy](https://duckdb.org/docs/current/quack/setup/reverse_proxy)
- [DuckDB Quack deployment](https://duckdb.org/docs/current/quack/setup/deployment)
- [DuckDB Quack on WebAssembly](https://duckdb.org/docs/current/quack/setup/quack_wasm)
- [ADR identità, bootstrap e sicurezza worker](../architecture/adr-worker-identity-bootstrap-security.md)
- [DuckDB Quack extension status](https://duckdb.org/docs/current/core_extensions/quack)
- [DuckDB concurrency](https://duckdb.org/docs/current/connect/concurrency)
- [DuckDB connection and thread guidance](https://duckdb.org/docs/current/clients/c/connect)
- [DuckDB Rust client](https://duckdb.org/docs/current/clients/rust)
- [`duckdb-rs::Connection::try_clone`](https://docs.rs/duckdb/latest/duckdb/struct.Connection.html#method.try_clone)
- [DuckDB workload tuning](https://duckdb.org/docs/current/guides/performance/how_to_tune_workloads)
- [DuckDB out-of-memory guidance](https://duckdb.org/docs/current/guides/performance/oom)
- [Kubernetes Jobs](https://kubernetes.io/docs/concepts/workloads/controllers/job/)
- [Kubernetes probes](https://kubernetes.io/docs/concepts/workloads/pods/probes/)
- [Kubernetes Leases](https://kubernetes.io/docs/concepts/architecture/leases/)
- [Kubernetes ephemeral volumes](https://kubernetes.io/docs/concepts/storage/ephemeral-volumes/)
- [Kubernetes resource management](https://kubernetes.io/docs/concepts/configuration/manage-resources-containers/)
- [Intento funzionale Feature 002](002-universal-query-source-and-multi-input-query.md)
- [ADR affinity CLI corrente](../architecture/adr-affinity-session.md)
