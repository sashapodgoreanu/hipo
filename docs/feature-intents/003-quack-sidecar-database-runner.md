# Feature intent: DuckDB sidecar con protocollo Quack

## Stato

**Implementata con attivazione diretta del runtime Quack.**  
Data decisione finale: 2026-07-21.

## Intento

Duckle non deve eseguire DuckDB tramite una CLI avviata per stage o tramite una
sessione stdin persistente. Ogni pipeline run possiede un processo isolato
`duckle-db-sidecar` che incorpora DuckDB e mantiene un solo catalogo per tutta la
durata del run.

Il processo principale conserva planner, DAG, eventi, history, retry e runtime
non-DuckDB. Il sidecar possiede database, relazioni, attachment, memoria e spill.
Il trasporto SQL ordinario è Quack su loopback; non viene introdotta una REST API
dati parallela.

## Decisioni definitive

1. Esiste un sidecar dedicato e single-use per ogni pipeline run.
2. Ogni run acquisisce il worker esclusivamente tramite `WorkerPoolControl`.
3. Un worker ready viene leased atomicamente; in assenza di capacità ready il
   controller crea un worker on-demand.
4. Non esistono budget worker, hard maximum, admission queue o backend alternativo.
5. Il target warm è `max(base, ceil(peak_5m × 1.20))`, rivalutato ogni 5 secondi.
6. La base predefinita è 3; il picco dura 5 minuti; lo scale-in termina solo ready.
7. Il worker termina a fine run o cancellazione e non torna nel ready set.
8. Query Source e setup Data Source vengono eseguiti nel catalogo sidecar senza
   affinity o worker CLI persistente.
9. Preview, partial run, scheduler, MCP, headless e desktop usano lo stesso
   controller e lo stesso `RunDatabase`.
10. `quack_parallelism` è un semaforo per-worker, `automatico | 1..=8`, separato
    dai DuckDB threads e dal numero di pipeline concorrenti.
11. Il profilo risorse completo è persistito e applicato atomicamente prima della
    readiness.
12. Cancellazione o crash terminano il processo scope; non è richiesta la
    sopravvivenza del sidecar dopo la cancellazione di una query.
13. SlothDB e `xf.dbt` restano leggibili ma disabilitati senza fallback.
14. La coppia DuckDB/Quack è pin, verificata e inclusa offline.
15. Non esistono classi runtime, manifest di cutover o variabili che scelgono un
    altro backend.

## Motivazione

Il precedente backend CLI introduceva latenza di spawn, serializzazione tramite
stdin/stdout, database temporanei condivisi tramite file e classificazione
affinity. Inoltre desktop, runner, scheduler, MCP e build artifact conoscevano il
percorso del binario.

Il sidecar per-run fornisce:

- catalogo e cache validi per tutta la pipeline;
- concorrenza 2/4/8 sullo stesso database;
- isolamento di crash, memoria, CPU e spill dal processo UI;
- cleanup deterministico;
- un confine unico per tutti gli entry point;
- package offline senza installazione della DuckDB CLI.

## Ownership

### Main / headless / scheduler / MCP

Possiedono planning, DAG, stage events, retry, history e cancellazione del run.
Non aprono il database analitico localmente e non scelgono un processo worker.

### WorkerPoolControl

Possiede stati, domanda, target warm, provisioning, readiness, lease, release e
scale-in. È l’unico oggetto chiamato pool.

### Provider

Possiede dettagli di processo, endpoint, bootstrap e terminazione. PID, porte e
path non entrano nei contratti superiori.

### RunSession / RunDatabase

Possiedono lease, profilo, cancellazione e operazioni SQL/preview/transfer tipate.
Il raw client Quack e le credenziali restano privati.

### Sidecar

Possiede DuckDB, catalogo, setup server-side, relazioni, memory limit, threads,
spill e server Quack.

## Profilo risorse

Il workspace salva un unico oggetto versionato con memoria, CPU threads, spill,
parallelismo Quack e capacità base. L’assenza dei campi equivale ai default.

Il salvataggio durante query attive:

- coalesca le versioni intermedie;
- drena le query della generazione corrente;
- applica atomicamente l’ultima generazione;
- conserva il profilo precedente se l’applicazione fallisce;
- non rende ready un worker starting con una generazione superata.

## Sicurezza

- bind solo loopback;
- credenziale casuale per worker tramite bootstrap ereditato;
- handshake di identità, protocollo, versione e health;
- nessun token in argv, environment, file ready, IPC, history o log;
- metriche limitate a campi sanitizzati;
- Job Object/process group e sweeper per parent death, cancel e orfani;
- nessuna nuova capability Tauri o CSP.

## Packaging

Il comando desktop normale deve produrre tutto:

```bat
@echo off
cd /d "%~dp0apps\desktop"
cargo tauri build
```

Il `beforeBuildCommand` compila i binari release e il frontend. Il build script
verifica l’estensione Quack, richiede la coppia adiacente e incorpora sidecar,
estensione e runner. Una coppia assente o errata interrompe la build; non viene
scaricato un motore alternativo.

## Benchmark e precedente cutover

Il gate era stato progettato per mantenere CLI e Quack attivi in parallelo fino
a un’approvazione prestazionale. Il proprietario ha scelto di non mantenere due
runtime. I test di parità e package restano obbligatori, mentre il benchmark con
la vecchia CLI è una possibile analisi futura e non può modificare il routing.

## Criteri di successo

- una lease esclusiva per run;
- nessun direct spawn dal percorso pipeline;
- 100 richieste senza ready → 100 on-demand e target 120;
- seconda ondata servita dai warm worker;
- profilo applicato prima della readiness;
- preview, partial e Query Source attraverso la sessione Quack;
- cleanup e redazione verificati;
- package offline verificato;
- `cargo tauri build` sufficiente per produrre installer ed eseguibile desktop;
- nessun fallback produttivo alla DuckDB CLI.
