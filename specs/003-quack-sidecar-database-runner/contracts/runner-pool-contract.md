# Internal Contract: Worker Pool and Run Database

## Ownership

WorkerPoolControl è l'unico owner di provisioning e assegnazione. Ogni entry point chiama semanticamente acquire(run_id, profile_version, cancellation); non riceve PID, porta, path, token, capability o provider handle.

## Outcomes

| Condition | Outcome |
|---|---|
| worker ready disponibile | Transizione atomica ready → leased e una RunSession. |
| nessun ready | Avvio, handshake e assegnazione di un worker on-demand, poi RunSession. |
| bootstrap/version failure | Failure sanitizzata della sola run; nessun worker pubblicato. |
| cancel prima del lease | Cancel provisioning e cleanup processo. |
| cancel/crash dopo lease | Termina process scope e completa run cancelled/runner_crashed. |

RunSession possiede un solo worker. Espone SQL/batch, setup server-side, trasferimenti, preview e cancel; non espone una connessione DuckDB o attachment Quack grezzo.

## Events

Eventi additivi correlati a run/attempt/lease/worker opachi: richiesta, decisione warm/on-demand, provisioning, readiness, lease, release, failure, scale, profile apply e cleanup. Nessun secret, SQL, endpoint, port, PID o path.

