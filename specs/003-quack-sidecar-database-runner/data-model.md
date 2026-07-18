# Data Model — Feature 003

## RunnerResourcesProfile persistito

| Field | Meaning | Validation |
|---|---|---|
| version | Generazione desiderata | Monotona; coalescing/apply atomico. |
| memory | Memoria per worker | automatico, percentuale o quantità; memory_limit_mb legacy = quantità assoluta. |
| cpu_threads | Thread DuckDB worker | automatico o intero positivo; non è worker count. |
| spill | Quota/temp spill | automatico, percentuale o quantità. |
| quack_parallelism | Query concorrenti nella run | automatico o 1..=8, default 8; non è pool. |
| base_capacity | Floor warm | Intero positivo, default 3. |

Il profilo non entra in PipelineDoc ed è comune a desktop, headless, scheduler e MCP.

## Entità effimere

| Entity | Identity | Invariante |
|---|---|---|
| AcquireRequest | run id, attempt, profile version | Una sola richiesta per run al controller. |
| WorkerPoolControl | istanza/workspace | Unico decisore di worker ready o on-demand. |
| Worker | worker id opaco | Warm: starting → ready → leased → terminating → terminated. On-demand non diventa ready. |
| WorkerLease | lease id, worker id, run id | Esclusivo; release/cancel/crash termina worker single-use. |
| RunSession | run id + lease | Espone cancel/profilo/RunDatabase, non endpoint/token/PID. |
| RunDatabase | interno session | SQL, setup, preview, materializzazione e trasferimenti controllati. |
| DemandWindow | istanza/workspace | Picco 5 min; reset al restart. |

target_warm = max(base_capacity, ceil(peak_5m × 1.20)). Il picco conta warm e on-demand; capacità warm conta solo starting/ready/leased.

