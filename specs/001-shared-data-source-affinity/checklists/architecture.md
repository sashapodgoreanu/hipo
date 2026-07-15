# Architecture Requirements Checklist: Data Source condivisi e Query Source

**Purpose**: Validare completezza, chiarezza e verificabilità dei requisiti architetturali, IPC, sicurezza ed esecuzione prima dell’implementazione.
**Created**: 2026-07-15
**Feature**: [spec.md](../spec.md) · [plan.md](../plan.md)
**Audience**: reviewer tecnico durante refinement e PR planning

## Requirement Completeness

- [ ] CHK001 Sono definiti tutti i campi persistiti di `DataSourcePayload`, inclusi valori di default, campi opzionali e limiti delle `options`? [Completeness, Spec §FR-001–FR-002]
- [ ] CHK002 Sono specificate le regole di compatibilità per ogni `kind` supportato e per le Connection mancanti o incompatibili? [Completeness, Spec §FR-005, FR-020]
- [ ] CHK003 Sono descritti sia il percorso Tauri desktop sia il web runner per risoluzione dei riferimenti e consegna effimera dei secret? [Gap, Spec §Execution and Security Impact]
- [ ] CHK004 Sono definiti formato, versione e comportamento di fallback per workspace e pipeline che non contengono Data Source? [Completeness, Spec §Compatibility and Migration]

## Requirement Clarity and Consistency

- [ ] CHK005 È definita una grammatica non ambigua per identificatori e alias DuckDB, incluse parole riservate, caratteri Unicode e confronto case-insensitive? [Clarity, Spec §FR-003]
- [ ] CHK006 Sono espliciti i criteri con cui il rename aggiorna SQL, inclusi alias quotati, commenti, stringhe e riferimenti multipli? [Ambiguity, Spec §FR-004]
- [ ] CHK007 La policy di eliminazione distingue chiaramente conferma, dipendenze visualizzate, stato invalido e possibilità di annullamento? [Clarity, Spec §FR-004a, User Story 1]
- [ ] CHK008 I requisiti di affinità sono coerenti tra collegamento diretto, affinità transitiva, sottografi parziali e stage intermedi esterni? [Consistency, Spec §FR-010–FR-017]
- [ ] CHK009 È chiarito cosa significhi “stessa sessione DuckDB” e quali limiti valgano quando un gruppo attraversa stage non compatibili con il worker? [Ambiguity, Spec §FR-011, FR-016]

## Acceptance Criteria Quality

- [ ] CHK010 I criteri di accettazione misurano in modo osservabile “attach una sola volta”, “stesso contesto” e “stessa sessione”, indicando le evidenze attese? [Measurability, Spec §Acceptance Criteria]
- [ ] CHK011 I criteri distinguono errori di inizializzazione del contesto da errori di singola Query Source e definiscono l’impatto sui downstream? [Clarity, Spec §FR-021]
- [ ] CHK012 Sono definiti limiti misurabili per preview, durata, righe restituite, timeout e dimensione dei messaggi diagnostici? [Gap, Spec §FR-007, FR-018]
- [ ] CHK013 I criteri di cleanup specificano quali processi, file, WAL, secret temporanei e attachment devono risultare assenti dopo successo, errore e cancellazione? [Measurability, Spec §FR-019]

## Scenario and Edge-Case Coverage

- [ ] CHK014 Sono coperti i casi di zero Data Source, Query Source senza riferimenti e Query Source con riferimenti duplicati? [Coverage, Spec §FR-007–FR-010]
- [ ] CHK015 Sono definiti i comportamenti per alias eliminato, Connection rimossa, estensione mancante, ATTACH fallito e SQL sintatticamente valido ma non read-only? [Coverage, Edge Case, Spec §FR-009, FR-022]
- [ ] CHK016 Sono descritti retry, wait, partial run, cancellazione durante attach e cancellazione durante una query in corso? [Coverage, Recovery, Spec §FR-016–FR-019]
- [ ] CHK017 È esplicitato il comportamento quando due gruppi indipendenti tentano di usare lo stesso database temporaneo o la stessa risorsa durante la stessa run? [Gap, Edge Case, Spec §FR-011–FR-015]
- [ ] CHK018 Sono definiti i requisiti di compatibilità multipiattaforma per spawn, framing stdout/stderr e terminazione del processo DuckDB? [Completeness, Non-Functional, Spec §Execution and Security Impact]

## Security and Non-Functional Requirements

- [ ] CHK019 Sono elencati tutti i canali in cui un secret potrebbe fuoriuscire (SQL generato, stderr, eventi, history, preview, file temporanei) e la regola di redazione per ciascuno? [Completeness, Security, Spec §FR-006, Execution and Security Impact]
- [ ] CHK020 Sono quantificati o motivati i requisiti di performance per attach, preview, materializzazione e numero di Query Source per gruppo? [Gap, Non-Functional, Spec §Success Criteria]
- [ ] CHK021 Sono definite capability, permission, scope e CSP minime per i nuovi comandi IPC, senza affidarsi a permessi impliciti? [Gap, Security, Spec §Execution and Security Impact]
- [ ] CHK022 È definita una politica per connector non supportati che impedisca un fallback silenzioso capace di violare la promessa di stessa sessione? [Clarity, Security, Spec §FR-020, Assumptions]

## Dependencies, Assumptions, and Traceability

- [ ] CHK023 Ogni requisito che dipende da DuckDB CLI, estensioni o servizi esterni identifica prerequisiti, disponibilità e comportamento quando l’ambiente non è configurato? [Dependency, Spec §FR-005, FR-020]
- [ ] CHK024 Le decisioni proposte per worker persistente, framing e risoluzione dei secret sono chiaramente separate dai fatti rilevati nel brownfield scan? [Consistency, Assumption, Spec §Current State and Scope, plan §Summary]
- [ ] CHK025 Ogni evento IPC e comando di preview/test ha schema di input/output, errori e compatibilità web/Tauri tracciati a requisiti funzionali specifici? [Traceability, Spec §FR-007, FR-018]
- [ ] CHK026 Sono identificati esplicitamente i gap non coperti da test frontend, E2E o connector-gated e il criterio per accettarli come rischio residuo? [Gap, Coverage, plan §Test Plan]

## Notes

- Checklist aggiunta come file separato; `checklists/requirements.md` è stato preservato.
- Focus: architettura/runtime, contratti IPC, sicurezza, error propagation e qualità dei criteri di accettazione.
