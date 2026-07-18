# Runner Requirements Quality Checklist: Quack Sidecar Database Runner

**Purpose**: Valutare chiarezza, completezza, coerenza e misurabilità dei requisiti del runner Quack prima dell'implementazione.
**Created**: 2026-07-18
**Feature**: [spec.md](../spec.md)

**Focus**: review pre-implementazione su pool elastico, risorse, gate di cutover, sicurezza e benchmark.

## Requirement Completeness

- [x] CHK001 Sono definiti tutti gli entry point considerati “produttivi”, inclusi inspect, drift, branch/diff e CI? [Completeness, Spec §FR-036/FR-042]
- [x] CHK002 Sono esplicitati owner, evidenze richieste e autorità di approvazione del gate che abilita il runner ufficiale? [Completeness, Spec §FR-042, SC-001–SC-011]
- [x] CHK003 La spec definisce il comportamento del backend di compatibilità per ogni entry point fino al cutover? [Completeness, Spec §FR-038/FR-042]
- [x] CHK004 Sono definiti per il profilo risorse i limiti host/pool/licenza, la precedenza e le ragioni possibili di clamp o rifiuto? [Completeness, Spec §FR-043/FR-044]
- [x] CHK005 Sono definite le metriche obbligatorie di memoria, CPU e spill, comprese unità, frequenza, retention e destinatari? [Completeness, Spec §FR-021/FR-034]
- [x] CHK006 Sono documentati requisiti di failure e recovery per quota spill, spazio disco insufficiente e applicazione incompleta del profilo? [Completeness, Spec §FR-021/FR-022/FR-048]
- [x] CHK007 La decision table SQL remoto/Quack/Parquet indica tutti gli input, gli output e le eccezioni richiesti? [Completeness, Spec §FR-016]
- [x] CHK008 Sono definiti workload, hardware, dataset, warm-up e raccolta risultati per il benchmark di cutover? [Completeness, Spec §SC-009/SC-010]
- [x] CHK009 Sono specificate le informazioni storiche che devono restare dopo la rimozione dello spike Phase 0? [Completeness, Spec §FR-039/FR-040]

## Requirement Clarity

- [x] CHK010 Il termine “percorso produttivo” è delimitato in modo non ambiguo rispetto a test, compatibilità, sviluppo locale e CI? [Clarity, Spec §FR-042]
- [x] CHK011 Il significato di “gate approvato” è misurabile e distingue risultati obbligatori, soglie e deroghe? [Clarity, Spec §FR-042, SC-001–SC-011]
- [x] CHK012 Il requisito “stesso hardware” del benchmark stabilisce una configurazione identificabile e le tolleranze ammesse? [Clarity, Spec §SC-010]
- [x] CHK013 Il requisito dei “100 avvii” chiarisce se l'unità osservata è l'avvio del supervisor, il worker o un'altra entità? [Clarity, Spec §SC-013]
- [x] CHK014 La formula del target definisce in modo univoco come sono conteggiate run fallite, cancellate durante bootstrap e burst inferiori a cinque secondi? [Clarity, Spec §FR-033/FR-054]
- [x] CHK015 Il termine “subito” nelle modifiche del profilo e nell'on-demand è delimitato da un punto di sincronizzazione osservabile? [Clarity, Spec §FR-047/FR-049/FR-051]
- [x] CHK016 Il requisito di metriche “sanitizzate” identifica quali campi sono consentiti, redatti o aggregati? [Clarity, Spec §FR-021/FR-027/FR-050]

## Requirement Consistency

- [x] CHK017 Sono coerenti l'assenza di budget/admission queue e l'esistenza del gate FIFO limitato alle query di una singola run? [Consistency, Spec §FR-010/FR-033/FR-051]
- [x] CHK018 Sono coerenti le regole “single-use” dei worker warm e on-demand con la definizione di capacità warm? [Consistency, Spec §FR-031/FR-032/FR-052]
- [x] CHK019 Il cutover unico per runner, CLI, affinity e spike è coerente con la possibilità di usare il runner solo in test/compatibilità prima dell'approvazione? [Consistency, Spec §FR-038/FR-040/FR-042]
- [x] CHK020 I requisiti di persistenza del profilo sono coerenti con l'affermazione che picco e target oltre la base sono effimeri? [Consistency, Spec §FR-030/FR-033/FR-043]
- [x] CHK021 Sono coerenti gli eventi richiesti per ogni azione di autoscaling con i vincoli di redazione di token, endpoint, PID, path e SQL? [Consistency, Spec §FR-027/FR-050/FR-053]

## Acceptance Criteria Quality

- [x] CHK022 Ogni criterio di successo collegato al gate di cutover identifica una fonte dati e un esito oggettivamente decidibile? [Measurability, Spec §SC-001–SC-011]
- [x] CHK023 Le soglie di benchmark e il criterio di approvazione sono documentati senza dipendere da valori da definire durante il cutover? [Measurability, Spec §SC-009/SC-010]
- [x] CHK024 Il criterio di cleanup entro dieci secondi definisce inizio, fine e artefatti inclusi nella misurazione? [Measurability, Spec §FR-019, SC-005]
- [x] CHK025 Il criterio dei 100 worker/base 3 specifica risorse disponibili, timeout readiness e trattamento dei failure di startup? [Measurability, Spec §SC-013]
- [x] CHK026 I criteri del picco 100→120 e della seconda ondata definiscono quando il target deve essere “pronto” e come trattare startup ancora in corso? [Measurability, Spec §SC-018/SC-019]
- [x] CHK027 I criteri di redazione identificano in modo esaustivo tutte le superfici persistenti e transitorie da esaminare? [Coverage, Spec §FR-027, SC-004]

## Scenario and Edge-Case Coverage

- [x] CHK028 Sono definite le conseguenze del rifiuto del gate di cutover, inclusi stato di compatibilità, diagnostica e nuova valutazione? [Coverage, Gap]
- [x] CHK029 Sono definiti i requisiti per modifica del profilo mentre un worker è starting, ready, leased e terminating? [Coverage, Spec §FR-047–FR-049, Edge Cases]
- [x] CHK030 Sono documentate le condizioni di recovery quando il benchmark non supera la soglia ma la sicurezza e la parità sono soddisfatte? [Coverage, Gap]
- [x] CHK031 Sono affrontate le collisioni fra shutdown dell'istanza, scale-out, worker on-demand in bootstrap e modifica del profilo? [Edge Case Coverage, Spec §FR-019/FR-033/FR-047/FR-051]
- [x] CHK032 La spec distingue il comportamento richiesto quando la capacità base diminuisce con soli worker leased, soli ready o worker starting? [Coverage, Spec §FR-033/FR-049, Edge Cases]
- [x] CHK033 Sono definiti requisiti di compatibilità per workspace che contengono impostazioni legacy del profilo oltre a SlothDB e xf.dbt? [Coverage, Spec §FR-041/FR-043]
- [x] CHK034 Sono definite le aspettative quando un entry point non può risolvere un profilo effettivo valido o non può accedere al bundle offline? [Exception Flow, Spec §FR-035/FR-044/FR-045]

## Dependencies and Assumptions

- [x] CHK035 La dipendenza dalla coppia DuckDB/Quack documenta ownership, licenza, aggiornamento, supporto Windows/macOS/Linux e piano di incompatibilità? [Dependency, Spec §FR-035/FR-037]
- [x] CHK036 Gli assunti sul singolo workspace per istanza definiscono il comportamento richiesto per tentativi di apertura concorrente o riapertura? [Assumption, Spec §FR-030]
- [x] CHK037 I documenti ADR e feature intent sono individuati come sorgenti da riallineare senza lasciare regole concorrenti sul budget o sulla coda? [Consistency, Spec §Assumptions, Gaps, and Decisions]
- [x] CHK038 Le soglie prestazionali ancora aperte hanno un owner e una condizione esplicita che impedisce il cutover finché non vengono approvate? [Dependency, Spec §Assumptions, Gaps, and Decisions]

## Notes

- Spuntare gli elementi solo dopo aver valutato la qualità della formulazione nei documenti di feature.
- Annotare eventuali ambiguità o proposte direttamente accanto all'elemento interessato.
