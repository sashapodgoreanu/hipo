# Research — Quack Sidecar Database Runner

## Confine Quack e versione

**Decision:** creare il binario ufficiale duckle-db-runner e pinare una sola coppia DuckDB/Quack client-server, inizialmente quella validata dallo spike; alzare il MSRV workspace a Rust 1.85.1.

**Rationale:** lo spike usa duckdb-rs 1.10504.0 / DuckDB 1.5.4 e richiede Rust 1.85.1. Quack è sperimentale, quindi wrapper interno e coppie atomiche riducono l'area di aggiornamento.

**Alternatives considered:** CLI, Quack non pinato/autoinstall, REST/JSON.

Fonti: [spike](../../spikes/quack-sidecar/Cargo.toml), [Quack extension](https://duckdb.org/docs/current/core_extensions/quack), [Quack overview](https://duckdb.org/docs/current/quack/overview).

## Controller obbligatorio e autoscaling

**Decision:** ogni run usa WorkerPoolControl acquire; il controller assegna ready o provisioning on-demand. Target ogni 5 s: max(base, ceil(peak_5m × 1,20)); base 3, finestra 5 min, scale-in solo ready, restart dalla base.

**Rationale:** nessun entry point può aggirare lease, stato e telemetria; dopo 100 run il target è 120, non scatti progressivi.

**Alternatives considered:** crescita 70%/step base, crescita 50% on-demand, queue/budget worker.

## Client stateless e concorrenza

**Decision:** RunSession mantiene un master client privato; ogni stage ammesso usa clone breve e quack_query stateless. QuackPermitGate per-run limita 1..=8 richieste e non è un pool.

**Rationale:** lo spike prova 2/4/8-way con query stateless. Batch multi-statement resta deciso dall'orchestratore per TEMP/SET.

**Alternatives considered:** ATTACH client sticky, pool connessioni, esecuzione nel main.

Fonti: [report Phase 0](../../docs/architecture/quack-sidecar-phase-0-report.md), [Quack reference](https://duckdb.org/docs/current/quack/reference).

## Sicurezza e bootstrap

**Decision:** provider locale loopback, pipe anonime ereditate/handle allowlist, credenziale casuale e handshake autenticato prima della readiness.

**Rationale:** Quack espone SQL completo; token non possono passare in argv, env, file ready, IPC o log. Containment di processo consente cleanup isolato.

**Alternatives considered:** environment/ready file dello spike, listener non locale, endpoint esposto ai runtime.

Fonti: [ADR bootstrap](../../docs/architecture/adr-worker-identity-bootstrap-security.md), [Quack security](https://duckdb.org/docs/current/quack/security).

## Profilo risorse live e rollout

**Decision:** introdurre RunnerResourcesProfile atomico/versionato, non environment globale; aggiornare prima ADR/intent, poi adapter di parità, infine cutover unico CLI/affinity/spike.

**Rationale:** setter sequenziali non garantiscono drain-safe apply; desktop-only lascerebbe runner, MCP e scheduler sulla CLI.

**Alternatives considered:** setter separati, riavvio leased, fallback CLI permanente.

