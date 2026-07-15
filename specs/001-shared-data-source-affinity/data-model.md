# Data model

## Persisted entities

`RepoItem` resta l’involucro autorevole (`id`, `type`, `name`, parent e metadati). Il nuovo `type: "data_source"` usa `DataSourcePayload`:

| Campo | Tipo | Invariante |
|---|---|---|
| `sqlAlias` | string | obbligatorio, identificatore DuckDB, unique case-insensitive nel workspace |
| `kind` | enum | `duckdb`, `postgres` nella prima release |
| `connectionRef` | RepoItem id | deve puntare a Connection compatibile |
| `readOnly` | bool | default `true`; non concede scrittura al connector |
| `defaultCatalog`, `defaultSchema` | string? | opzionali |
| `options` | object | opzioni non sensibili del connector |

Credenziali e stringhe segrete non appartengono al payload; sono risolte dalla Connection al confine di esecuzione.

## Pipeline entities

`src.query` aggiunge alle proprietà del nodo `dataSourceRefs: string[]`, `sql: string`, `previewLimit?: number`, `schema?: SchemaMetadata` e impostazioni di materializzazione supportate. Il nodo produce una relazione come gli altri Source e non copia `ConnectionPayload`.

## Run-local entities (proposti)

- `AffinityGroup`: id, query source ids, data source ids, topological order, session state.
- `AffinityPlan`: gruppi, stage esterni, dipendenze e mapping node→group.
- `AffinitySessionState`: `created | initializing | ready | failed | cancelling | closed`.
- `DataSourceAttachment`: data source id, alias, attach status, duration, sanitized diagnostic.

Le entità run-local non vengono persistite nel workspace; eventi e run log contengono solo id, alias e diagnostica sanitizzata.

## Relationships and lifecycle

Un Data Source riferisce una Connection. Una Query Source riferisce zero o più Data Source; le componenti connesse del grafo bipartito definiscono un gruppo. Rename aggiorna dipendenze SQL dopo conferma. Delete confermata rimuove l’item e marca `src.query` dipendenti invalidi. Durante una run: resolve → validate → create group → attach once → execute/materialize → close/cleanup.
