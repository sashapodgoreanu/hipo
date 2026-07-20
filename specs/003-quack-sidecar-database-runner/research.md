# Research — Quack Sidecar Database Runner

## Confine Quack e versione

**Decision:** creare il binario ufficiale duckle-db-runner e pinare una sola coppia DuckDB/Quack client-server, inizialmente quella validata dallo spike; alzare il MSRV workspace a Rust 1.88.

**Rationale:** lo spike usa duckdb-rs 1.10504.0 / DuckDB 1.5.4 e richiede almeno Rust 1.85.1; il lockfile dell'attuale workspace include inoltre dipendenze desktop come `darling`, `mongodb`, `time` e `image` che dichiarano MSRV Rust 1.88. Quack è sperimentale, quindi wrapper interno e coppie atomiche riducono l'area di aggiornamento.

**Bundle verificato:** l'estensione Quack ufficiale 1.5.4 è pinata a DuckDB 1.5.4 e deve essere verificata prima della readiness. Gli SHA-256 correnti del bundle approvato dal codice sono: Windows AMD64 `3274bac6becc0f750497726a73f9ae858606cec7ec1a935d83a5b84ee0402122`, macOS AMD64 `85a48992d0b940f7cf1c55bbe4efd02f46c9724b67e238a990df3f3244d8e970`, Linux AMD64 `decb78a4d953ff9cc65c300cf2c8d3f3d8f4732851205684565c922113bc2b9e`. La licenza è MIT e la provenienza è `https://github.com/duckdb/duckdb-quack`; il runtime non esegue `INSTALL quack` e il release package deve stageare il file già verificato. L'identità autoritativa resta `crates/duckle-db-runner/src/bundle.rs`; ogni aggiornamento dell'estensione richiede aggiornamento atomico di codice, package smoke ed evidenza di cutover.

**Alternatives considered:** CLI, Quack non pinato/autoinstall, REST/JSON.

Fonti: [spike](../../spikes/quack-sidecar-phase0-spike/Cargo.toml), [Quack extension](https://duckdb.org/docs/current/core_extensions/quack), [Quack overview](https://duckdb.org/docs/current/quack/overview).

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
