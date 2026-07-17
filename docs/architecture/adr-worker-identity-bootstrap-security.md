# ADR: identità, bootstrap e sicurezza dei worker DuckDB

## Stato

**Proposed.** Il contratto è richiesto prima dell'integrazione produttiva del
provider locale. Il PoC Phase 0 ora usa readiness token-free, secret scoped,
`ATTACH` sticky e test negativi di autenticazione; resta non conforme perché il
bootstrap usa temporaneamente l'environment del child invece delle pipe anonime.

## Contesto

Ogni pipeline run acquisisce un worker esclusivo e single-use che incorpora
DuckDB ed espone Quack. Conoscere l'endpoint del worker non deve essere
sufficiente per collegarsi. Allo stesso tempo, scheduler e `RunDatabase` non
devono dipendere dal modo in cui un provider avvia il worker o consegna le sue
credenziali.

Quack espone tutta la superficie SQL visibile alla sessione server. Il suo
default autentica il client con un token, mentre l'autorizzazione successiva è
permissiva. La credenziale deve quindi essere trattata come una capability con
pieno accesso al database del worker.

Il profilo della feature è `execution_trusted_full_sql_v1`. L'autorizzazione
full SQL è intenzionale per il supervisor della pipeline, ma deve essere
impostata e dichiarata esplicitamente: non deve sembrare una policy read-only e
non è riutilizzabile per browser o pubblicazione dati.

La prima implementazione è locale. La possibilità futura di usare un provider
remoto richiede però che `localhost`, pipe, token, certificati e Pod non entrino
nel contratto di scheduling.

## Decisione

Duckle adotta un unico contratto semantico per identità, endpoint verificato,
credenziali opache, readiness e revoca. Ogni `WorkerProvider` realizza quel
contratto con meccanismi propri.

```rust
struct VerifiedWorker {
    worker_id: WorkerId,
    endpoint: QuackEndpoint,
    expected_identity: WorkerIdentity,
    credentials: CredentialHandle,
    transport: TransportSecurity,
}

enum TransportSecurity {
    LocalLoopback,
    ServerTls { trust: TrustBundleHandle },
    MutualTls {
        trust: TrustBundleHandle,
        client_identity: ClientIdentityHandle,
    },
}
```

`CredentialHandle`, `TrustBundleHandle` e `ClientIdentityHandle` sono opachi,
non serializzabili, non loggabili e accessibili soltanto al connector Quack. Il
contratto non espone campi generici `token`, `certificate_path` o `pod_name`.

`WorkerPoolControl` riceve un `VerifiedWorker`; non riceve separatamente porta e
segreto. `QuackRunDatabase` chiede al connector di aprire una connessione verso
quel worker senza conoscere l'origine della credenziale.

## Invarianti comuni

Ogni provider deve garantire:

1. identità univoca e non riutilizzata del worker;
2. credenziali uniche e limitate alla vita del worker;
3. separazione tra endpoint osservabile e credenziale segreta;
4. readiness infrastrutturale seguita da handshake applicativo autenticato;
5. verifica che endpoint, identità attesa e worker raggiunto coincidano;
6. nessuna credenziale in command line, environment, file di readiness, log,
   errori, history, eventi UI o payload serializzati;
7. revoca terminale tramite distruzione del worker e delle sue credenziali;
8. nuove identità e credenziali per ogni sostituto;
9. terminazione e cleanup idempotenti;
10. nessuna consegna automatica delle credenziali a codice utente o runtime
    esterni.

## Threat model locale

Il provider locale deve proteggere da:

- processi ordinari che scoprono o scansionano la porta loopback;
- lettura accidentale del token da argomenti, environment, filesystem o log;
- riuso di credenziali appartenute a un worker terminato;
- apertura del canale bootstrap da parte di un processo non correlato;
- pubblicazione prematura di un processo che ascolta ma non ha completato
  autenticazione e protocol handshake;
- perdita del token attraverso SQL esportato, profiler o log Quack/HTTP.

Non costituisce un confine assoluto contro un amministratore, un debugger con
privilegi equivalenti o superiori, un processo capace di leggere la memoria di
Duckle o un host già compromesso. Proteggere anche da quel modello richiede un
confine OS più forte e non può essere ottenuto soltanto cambiando il token.

## Bootstrap sicuro del provider locale

### Generazione

Il supervisor genera 32 byte tramite CSPRNG per ogni nuovo worker. Il token non
deriva da PID, worker ID, run ID, timestamp o porta. Il worker è single-use, per
cui una capability per-worker diventa di fatto una capability per una sola run
dopo l'assegnazione.

Il valore vive in un tipo segreto che:

- non implementa `Debug`, `Display` o serializzazione;
- evita copie implicite;
- azzera le copie controllate dall'applicazione al drop;
- non viene inserito in errori con contesto o panic payload.

### Canale

Il supervisor crea due pipe anonime, oppure un canale duplex equivalente:

```text
bootstrap: supervisor -> worker
control:   worker -> supervisor
```

Il child eredita soltanto l'handle di lettura bootstrap e quello di scrittura
control. Su Windows la creazione deve usare una allowlist esplicita
`PROC_THREAD_ATTRIBUTE_HANDLE_LIST`; abilitare genericamente l'ereditarietà di
tutti gli handle è vietato. Gli estremi non destinati al child sono marcati non
ereditabili e chiusi subito dopo l'avvio.

Il token non passa tramite stdin generico, command line, environment, file o
named pipe apribile per nome. Il payload è length-prefixed, versionato e con un
limite massimo; letture parziali, versione sconosciuta o trailing bytes causano
la terminazione del worker.

### Bind e readiness

Il worker effettua direttamente il bind su `127.0.0.1` e porta `0`; non esiste
una prenotazione della porta nel parent seguita da riapertura nel child. Quack
resta configurato per non accettare hostname non locali.

Il worker restituisce sulla control pipe soltanto metadati non segreti:

```text
worker_id, pid, endpoint, protocol_version, duckdb_version, server_nonce
```

Il target produttivo non usa `ready.json`. Dopo aver ricevuto il messaggio, il
supervisor crea un client con la capability e verifica tramite Quack almeno:

- worker ID;
- protocol version Duckle;
- versione DuckDB/Quack prevista;
- nonce o challenge del bootstrap;
- capacità di eseguire una query minima autenticata.

Soltanto questo handshake consente la transizione `starting -> ready`.

### Uso nel client

La credenziale resta privata a `QuackConnector`/`QuackRunDatabase`. La versione
pin è stata verificata con `CREATE TEMPORARY SECRET` parameterizzato e scoped
all'esatto endpoint. Ogni richiesta invia lo statement completo con
`quack_query(uri, sql)` senza token inline. `remote.query(...)` e
`quack_query_by_name(...)` non sono esposti dal connector produttivo perché
richiedono un client `ATTACH TYPE quack` sticky.

Il contratto ordinario è stateless: stato `TEMP`/`SET` necessario a più
statement deve restare nello stesso batch remoto, mentre lo stato condiviso usa
il catalogo del sidecar. Il catalogo client raw resta privato al connector:
esporlo al planner consentirebbe query federate con operatori eseguiti
accidentalmente nel client.

Il logging Quack/HTTP e il profiling sono disabilitati nei percorsi che possono
contenere credenziali. La query applicativa può essere osservabile soltanto
dopo una decisione esplicita di redazione e senza includere il bootstrap.

### Fine vita

Alla conclusione o cancellazione della run:

1. non vengono create nuove connessioni;
2. le connessioni client vengono chiuse;
3. le copie applicative della credenziale vengono azzerate;
4. il worker viene terminato anche se lo shutdown cooperativo fallisce;
5. il processo sostitutivo riceve nuovo worker ID, token e porta;
6. la capability precedente non può autenticarsi sul sostituto.

## Hardening locale aggiuntivo

Il provider locale deve inoltre:

- avviare il sidecar tramite path assoluto verificato;
- applicare Windows Job Object o process group con kill-on-parent-death;
- limitare tramite DACL gli accessi non necessari al processo worker, inclusi
  lettura/scrittura memoria e duplicazione handle dove applicabile;
- usare una directory per-worker accessibile soltanto all'utente corrente;
- impedire il caricamento di librerie dalla current working directory;
- evitare crash dump applicativi contenenti segreti o documentarne chiaramente
  il rischio residuo;
- non consegnare il `CredentialHandle` a Python, Rust user code, JavaScript o
  altri runtime non trusted. Questi usano API controllate, stream o snapshot.

Il process hardening riduce l'attacco da processi ordinari, ma non supera i
privilegi di un amministratore o `SeDebugPrivilege`.

## Compatibilità con provider remoti

Questa ADR non implementa un provider remoto. Il contratto consente però una
realizzazione che sostituisca il bootstrap locale con credenziali e identità di
workload e che esponga un endpoint HTTPS verificabile.

Poiché Quack non termina TLS direttamente, un provider remoto può mantenere
Quack su loopback accanto a un reverse proxy TLS. Il primo profilo compatibile è
`HTTPS + capability token`: il certificato autentica il server e protegge il
token in transito; il token autentica il client Quack. `mTLS + token` resta una
possibile difesa aggiuntiva, soggetta a uno spike che verifichi il supporto del
client HTTP DuckDB per certificati client.

La scelta futura di Secret store, CA, reverse proxy, workload identity e
network policy appartiene al provider e non modifica `VerifiedWorker` o il
lifecycle del lease.

Il reverse proxy deve preservare HTTP/1.1 keep-alive, disabilitare il buffering
dello streaming FETCH e configurare body size e timeout compatibili con query
lunghe e APPEND. Quack non termina TLS direttamente. Un client DuckDB-Wasm
richiede inoltre HTTPS e CORS se l'endpoint non è same-origin.

## Vincolo futuro di publication plane

La futura pubblicazione di Book è fuori scope. L'architettura conserva però un
vincolo: un Book non si collega mai a un worker di pipeline né riceve una
capability `execution_trusted_full_sql_v1`.

Un eventuale `publication_read_only_v1` richiederà una feature e un threat model
separati, con database/snapshot dedicato, reverse proxy, identità utente o
capability Book-scoped e autorizzazione non permissiva. La documentazione Quack
avverte che il filtro read-only basato sul solo prefisso SQL è aggirabile; il
profilo pubblico dovrà usare query approvate oppure parsing degli statement in
una authorization function nativa, oltre a sandboxare filesystem, extension e
accesso di rete. Il supporto Quack in DuckDB-Wasm rende possibile il client
browser, ma non risolve autenticazione, autorizzazione, CORS o isolamento dati.

## Test e gate di accettazione

Il provider locale non può essere usato in produzione finché non passano test
automatici che dimostrano:

1. porta nota senza token: autenticazione rifiutata;
2. token errato: autenticazione rifiutata;
3. token appartenuto al worker precedente: autenticazione rifiutata;
4. token assente da command line, environment, filesystem, ready metadata,
   stdout/stderr, log, errori, history, profiler ed export SQL;
5. un processo sibling non può aprire il bootstrap channel;
6. il child eredita soltanto gli handle dichiarati;
7. un bootstrap troncato o malformato termina il worker;
8. nessun worker entra in `ready` prima dell'handshake autenticato;
9. un'identità o protocol version inattesi impediscono l'assegnazione;
10. cancellation e crash chiudono client, worker e canali senza residui;
11. il replacement usa PID/worker ID/porta/token nuovi;
12. error injection e panic non stampano il secret.

Sono inoltre richiesti test Windows per handle inheritance, DACL e Job Object,
più equivalenti Unix per file descriptor inheritance e process containment.

## Conseguenze

### Positive

- scoprire la porta non concede accesso;
- il token non attraversa superfici persistenti o osservabili ordinarie;
- scheduler e database API restano indipendenti dal provider;
- endpoint security può evolvere da loopback a TLS/mTLS senza cambiare il
  contratto di lease;
- kill-and-replace fornisce revoca semplice e verificabile.

### Negative

- il process spawning Windows richiede API native e test specifici;
- il secret esiste inevitabilmente nella memoria di client e server;
- Quack ha authorization permissiva dopo l'autenticazione e il token equivale a
  pieno accesso al worker;
- TLS remoto richiede un reverse proxy perché Quack non lo termina direttamente;
- mTLS diretto richiede una verifica separata delle capacità del client DuckDB.

## Open questions

1. Quali log DuckDB/Quack sono abilitati implicitamente nel build embedded?
2. È necessario un auth callback Duckle con capability monouso per connessione,
   oppure il token per-worker è sufficiente al threat model locale?
3. Quale hardening dei crash dump è applicabile senza modificare impostazioni
   globali dell'utente?
4. Il client HTTP DuckDB supporta certificati client mTLS nel profilo futuro?

## Riferimenti

- [DuckDB Quack security](https://duckdb.org/docs/current/quack/security)
- [DuckDB Quack overview](https://duckdb.org/docs/current/quack/overview)
- [DuckDB Quack reverse proxy](https://duckdb.org/docs/current/quack/setup/reverse_proxy)
- [DuckDB Quack deployment](https://duckdb.org/docs/current/quack/setup/deployment)
- [DuckDB Quack on WebAssembly](https://duckdb.org/docs/current/quack/setup/quack_wasm)
- [DuckDB HTTPS support](https://duckdb.org/docs/current/core_extensions/httpfs/https)
- [Windows pipe handle inheritance](https://learn.microsoft.com/en-us/windows/win32/ipc/pipe-handle-inheritance)
- [Windows process handle inheritance](https://learn.microsoft.com/en-us/windows/win32/procthread/inheritance)
- [Windows process security](https://learn.microsoft.com/en-us/windows/win32/procthread/process-security-and-access-rights)
- [Kubernetes TLS certificates](https://kubernetes.io/docs/tasks/tls/managing-tls-in-a-cluster/)
- [Quack sidecar ADR](adr-quack-sidecar-runner.md)
