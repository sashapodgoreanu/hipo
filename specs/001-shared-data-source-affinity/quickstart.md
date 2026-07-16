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

La validazione frontend non introduce un nuovo test runner in questa fase: gli
helper puri di dipendenza/rename/invalidation in `frontend/src/workspace.ts`
sono mantenuti senza side effect e vengono verificati tramite lint/build e la
verifica manuale dei flussi descritti sopra. La copertura planner/engine resta
nei test Rust.

Checkpoint implementazione 2026-07-15: `npm --prefix frontend run lint`,
`npm --prefix frontend run build`, `cargo check -p duckle-runner` e
`cargo check -p duckle-duckdb-engine --tests` e `cargo check -p duckle-desktop --lib`
passano. `cargo fmt --all --check`
segnala differenze preesistenti in altri moduli del workspace; il test Cargo
filtrato per affinity e il test filtrato di `duckle-secrets` hanno superato il
limite locale di linking senza produrre un errore di compilazione osservabile.

Il worker CLI di affinità è verificato con
`cargo test -p duckle-duckdb-engine affinity_session::tests` e un
`DUCKLE_DUCKDB_BIN` configurato: statement consecutivi mantengono la stessa
tabella nella sessione e il framing usa marker su file invece di stdout.

Una pipeline SQL con due Query Source che condividono un Data Source e
convergono in un Join è coperta da
`query_sources_sharing_a_data_source_join_in_one_affinity_worker`: il catalogo
viene collegato una sola volta e i due risultati sono materializzati nel
run-db prima del Join.
