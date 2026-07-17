# Query Source contract (proposto)

Esempio di proprietà `src.query`:

```json
{
  "dataSourceRefs": ["ds-sales", "ds-customers"],
  "sql": "SELECT * FROM sales.orders JOIN customers.accounts USING (id)",
  "previewLimit": 100
}
```

Il parser accetta qualsiasi singolo statement DuckDB e rifiuta statement multipli prima della pianificazione. Alias e riferimenti vengono risolti nel sottografo eseguito. Gli statement che producono righe sono materializzati nel database temporaneo della run; DDL/DML e gli altri statement senza righe vengono eseguiti una volta ed espongono una relazione Source vuota. La preview restituisce schema, righe limitate e durata soltanto per statement che producono righe.

La preview ha timeout massimo di 30 secondi e restituisce al massimo 1000 righe. Il cleanup della run/context ha budget massimo di 10 secondi; la latenza di attach/query remota viene riportata come diagnostica e non costituisce SLA.

Errori tipizzati distinguono riferimento mancante, alias ambiguo, SQL multi-statement, Connection incompatibile, attach/extension failure e session failure; nessun errore contiene secret.
