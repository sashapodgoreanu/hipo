# Data Source contract (proposto)

Persistenza in `data-sources/<repo-item-id>.json`:

```json
{
  "sqlAlias": "sales",
  "kind": "postgres",
  "connectionRef": "conn-01",
  "readOnly": true,
  "defaultCatalog": "warehouse",
  "defaultSchema": "public",
  "options": { "extension": "postgres" }
}
```

Nella prima release `kind` può essere solo `duckdb` o `postgres`; gli altri connector vengono rifiutati come Data Source e restano disponibili tramite gli Source esistenti. `RepoItem.id` è l’identità immutabile. L’alias è unico ignorando maiuscole/minuscole e non può essere una parola riservata DuckDB. Il test di compatibilità verifica `kind` e Connection prima del run. Rename/delete con dipendenze richiedono conferma e aggiornano/invalidano le Query Source secondo la specifica.

L’alias deve rispettare `[A-Za-z_][A-Za-z0-9_]*`. La propagazione del rename opera su identificatori SQL, preservando commenti e stringhe letterali.
