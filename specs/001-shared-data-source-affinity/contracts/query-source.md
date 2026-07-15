# Query Source contract (proposto)

Esempio di proprietà `src.query`:

```json
{
  "dataSourceRefs": ["ds-sales", "ds-customers"],
  "sql": "SELECT * FROM sales.orders JOIN customers.accounts USING (id)",
  "previewLimit": 100
}
```

Il parser accetta un solo statement read-only: `SELECT`, `WITH`, letture di tabelle/funzioni DuckDB. Rifiuta DDL, DML e statement multipli prima della pianificazione. Alias e riferimenti vengono risolti nel sottografo eseguito; il risultato è materializzato nel database temporaneo della run e restituisce schema, righe limitate e durata per preview.

La preview ha timeout massimo di 30 secondi e restituisce al massimo 1000 righe. Il cleanup della run/context ha budget massimo di 10 secondi; la latenza di attach/query remota viene riportata come diagnostica e non costituisce SLA.

Errori tipizzati distinguono riferimento mancante, alias ambiguo, SQL non read-only, Connection incompatibile, attach/extension failure e session failure; nessun errore contiene secret.
