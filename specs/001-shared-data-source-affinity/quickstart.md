# Quickstart di validazione

Prerequisiti: Rust/Cargo, Node/npm, DuckDB CLI configurato in `DUCKLE_DUCKDB_BIN` (o disponibile nel path), e un workspace con una Connection valida. Nella prima release i Data Source supportano soltanto `duckdb` e `postgres`; gli altri connector restano disponibili tramite gli Source esistenti. Le verifiche non richiedono modifiche al codice.

1. Crea due Data Source (`sales`, `customers`) che riferiscono Connection compatibili e due nodi `src.query` che condividono `sales`.
2. Esegui la pipeline: gli eventi devono mostrare un solo `contextId`, un attach per Data Source e due relazioni materializzate.
3. Aggiungi una Query Source che riferisce `customers`: il gruppo deve fondersi transitivamente. Un ramo senza riferimenti condivisi può avere un context distinto.
4. Inserisci uno stage esterno in un ramo indipendente e verifica che il gruppo affine continui. Un errore Query Source deve fallire solo i downstream; un errore di inizializzazione deve bloccare il gruppo.
5. Prova preview, partial run, cancellation e cleanup; verifica assenza di processi/file residui e di credenziali nei log.
6. Verifica rename alias (conferma e aggiornamento SQL), delete con dipendenze (conferma e Query Source invalide) e rifiuto di `CREATE`, `INSERT` o SQL con `;` multipli.

Comandi di parità CI:

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets --exclude duckle-lance
cargo test --workspace --exclude duckle-lance
npm --prefix frontend run lint
npm --prefix frontend run build
```

Test connector-specifici e DuckDB sono condizionati all’ambiente disponibile; non è stata rilevata una suite E2E frontend.
