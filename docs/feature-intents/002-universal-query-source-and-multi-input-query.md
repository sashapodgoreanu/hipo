# Intento funzionale: Query Source universale e Query multi-input

## Stato

**Deferred — da rispecificare dopo il nuovo database runner.**

La proposta architetturale che deve precedere questa feature è descritta in
[DuckDB sidecar con protocollo Quack](003-quack-sidecar-database-runner.md).

Questo documento conserva l'intento della precedente bozza Spec Kit
`002-session-aware-query-runner`, ma non costituisce una specifica pronta per
planning o implementazione. Requisiti tecnici, criteri di accettazione e
sequenza delle attività dovranno essere riscritti dopo la validazione della
nuova infrastruttura DuckDB/Quack.

## Perché viene sospesa

La bozza 002 era costruita attorno all'esecutore DuckDB CLI e al relativo
modello di affinity. La direzione architetturale è cambiata:

- Duckle main agirà come Quack client;
- un processo sidecar `duckle-db-runner` agirà come Quack server;
- ogni pipeline avrà una sola istanza DuckDB, posseduta dal sidecar per tutta
  la durata del run;
- il sidecar gestirà connessioni concorrenti, catalogo, memoria e spill su
  disco;
- la cancellazione del run terminerà l'intero processo sidecar;
- le query complete dovranno essere inviate al server, evitando che join e
  materializzazioni pesanti vengano eseguiti nel processo principale.

Queste scelte cambiano sessioni, affinity, materializzazione, parallelismo,
runtime esterni, cancellation e cleanup. Dettagliare ora tali comportamenti
nella 002 produrrebbe una specifica probabilmente da riscrivere.

## Obiettivo da conservare

Query Source deve diventare una Source universale. Qualsiasi componente
presente o futuro che dichiari un normale contratto di input/output deve poter
consumare il suo risultato senza essere aggiunto a una whitelist basata sul
`component_id`.

Devono quindi funzionare, tra gli altri:

- transform SQL;
- runtime Python, Rust e JavaScript;
- sink;
- nodi di controllo;
- chiamate a pipeline figlie;
- nuovi componenti compatibili aggiunti in futuro.

Affinity deve descrivere la condivisione e l'ownership di una risorsa, non
selezionare manualmente quali tipi di nodo sono ammessi dopo Query Source.

L'universalità si applica ai componenti attivi e supportati. `xf.dbt` e SlothDB
sono temporaneamente disabilitati dalla proposta del nuovo runner e non fanno
parte dei criteri di compatibilità della futura 002. I documenti esistenti che
li citano devono restare leggibili, ma la loro esecuzione viene rifiutata con
una diagnostica esplicita e senza fallback silenzioso.

## Nodo Query multi-input

Rimane desiderato un nodo Query con uno o più input. Ogni input dovrà avere un
alias stabile e rendere disponibili le relazioni dichiarate dall'upstream.

Il caso principale da supportare è una query eseguita nel sidecar che combini,
nello stesso statement, dati provenienti da origini differenti:

```sql
SELECT *
FROM querysource1.tabella a
JOIN querysource2.tabella b ON a.id = b.id
JOIN duckdb_source.catalogo.schema.tabella c ON a.id = c.id
```

Le dipendenze del DAG dovranno continuare a derivare dagli edge e dalle risorse
dichiarate, non esclusivamente dall'analisi libera del testo SQL. Alias,
namespace e relazioni disponibili dovranno essere validati prima
dell'esecuzione.

## Comportamenti funzionali da preservare

Quando la feature verrà rispecificata dovrà ancora coprire almeno questi casi:

1. Query Source alimenta qualsiasi nodo compatibile senza whitelist o
   blacklist di component ID.
2. Query accetta fan-in multiplo e può unire Query Source, Source DuckDB e
   risultati intermedi.
3. Un nodo disabilitato mono-input viene bypassato senza lasciare downstream
   collegati a una relazione inesistente.
4. Un bypass ambiguo, per esempio su nodi multi-input o multi-output, produce
   una diagnostica invece di scegliere silenziosamente un ramo.
5. Runtime esterni mantengono output, tipi, valori null, zero righe, reject e
   relazioni nominate necessari ai downstream.
6. Preview e partial run utilizzano soltanto il sottografo e le risorse
   necessarie.
7. Errori bloccano i downstream dipendenti e non devono impedire la conclusione
   di rami realmente indipendenti.
8. Token, secret e connection string non compaiono in eventi, errori, history,
   preview o artefatti temporanei persistenti.

## Decisioni rinviate alla nuova infrastruttura

La nuova specifica dovrà basarsi su risultati verificati per decidere:

- se il database del run sarà in-memory compresso, file-backed temporaneo o
  ibrido;
- memory limit, directory di spill e limite massimo dello spill;
- modalità con cui più connessioni Quack leggono e scrivono in parallelo;
- quali oggetti sono condivisi nel catalogo del run e quali sono legati a una
  singola connessione;
- come vengono registrati namespace e relazioni di ogni stage;
- quando un runtime esterno usa Quack e quando conviene uno snapshot Parquet;
- quali operazioni possono procedere in parallelo e quali richiedono accesso
  esclusivo;
- comportamento di retry in caso di conflitti DuckDB;
- gestione di preview, metriche, crash, cleanup e processi figli;
- compatibilità e packaging dell'estensione Quack su Windows, macOS e Linux.

## Condizioni per riattivare la feature

La 002 dovrà essere ricreata come nuova specifica Spec Kit solamente dopo che:

1. esiste un ADR approvato per il runner DuckDB sidecar;
2. un prototipo dimostra Duckle main come Quack client e il sidecar come Quack
   server;
3. sono stati misurati Quack, database temporaneo e snapshot Parquet sui carichi
   rappresentativi;
4. concorrenza, memoria, spill, crash e cancellazione mediante terminazione del
   sidecar sono stati verificati;
5. è definito un contratto stabile per relazioni, namespace e runtime esterni.

A quel punto questo documento sarà un input di prodotto per una nuova
specifica, non il piano tecnico da implementare direttamente.
