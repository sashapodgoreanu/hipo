# Feature Specification: [FEATURE NAME]

**Feature Branch**: `[###-feature-name]`
**Created**: [DATE]
**Status**: Draft
**Input**: "$ARGUMENTS"

## Current State and Scope

**Implemented baseline**: [Describe only behavior verified in code/tests.]
**Requested change**: [User-visible outcome.]
**Out of scope**: [Explicit exclusions.]
**Behavior to preserve**: [Pipeline/runtime/UI behavior that must not regress.]
**UI continuity (when UI changes)**: [Existing components, layout patterns, tokens, typography, spacing, control states, and interactions to reuse; document any intentional deviation.]

## User Scenarios and Acceptance

### User Story 1 - [Title] (Priority: P1)

**Why this priority**: [Value.]
**Independent test**: [Observable verification.]

1. **Given** [state], **When** [action], **Then** [outcome].
2. **Given** [edge/error state], **When** [action], **Then** [safe outcome].

### Edge Cases

- [Graph, path, credential, cancellation, retry, or platform boundary.]
- [Compatibility behavior for existing workspace/pipeline data.]

## Domain and Contract Impact

| Area | Affected? | Current owner / file | Required compatibility behavior |
|---|---:|---|---|
| Pipeline / PipelineDoc | [Y/N] | `crates/metadata`, `crates/duckdb-engine/src/plan/` | [ ] |
| Node / Edge / handles / alias | [Y/N] | `metadata`, `frontend/src/pipeline-types.ts` | [ ] |
| Component ID / properties / ports | [Y/N] | palette, manifests, planner | [ ] |
| Schema / preview / lineage | [Y/N] | metadata, engine | [ ] |
| Connection / context / secrets | [Y/N] | workspace, `secrets.rs`, `context.rs` | [ ] |
| Stage / RuntimeSpec / materialization | [Y/N] | `plan/mod.rs`, engine | [ ] |
| Tauri IPC / web bridge | [Y/N] | desktop `lib.rs`, `tauri-bridge.ts` | [ ] |
| Workspace persistence / migration | [Y/N] | `workspace.ts`, engine history/state | [ ] |
| Frontend UI / design system | [Y/N] | existing `frontend/src/...` components and styles | Reuse established visual and interaction patterns; no unintended style regression. |

## Functional Requirements

- **FR-001**: [Specific behavior.]
- **FR-002**: [Specific behavior.]

## Duckle Component Invariants

For every new or modified Duckle connector, source, sink, or component:

- [ ] The MCP API is implemented and exposed through `crates/duckle-mcp`, with compatible request/response contracts and coverage.
- [ ] The node exposes both required output flows: `main` and `reject`.
- [ ] The palette/manifest, Rust metadata, planner, executor, frontend validation, and MCP contract define the same `main` and `reject` port semantics.

## Execution and Security Impact

- **Graph/planner**: [Cycles, fan-in/out, partial run, leaves, aliases, control/reject edge implications.]
- **Execution**: [Pure SQL or RuntimeSpec; batch/per-stage; materialization; retry; cancellation; cleanup.]
- **Connections/secrets**: [Resolution precedence, masking, export/log/error behavior.]
- **IPC**: [Command DTO, event/channel, filesystem/network/process side effects.]
- **Security**: [Capabilities, scope, sidecar, plugin, untrusted input, remote exposure.]
- **Multiplatform**: [Windows/macOS/Linux and binary/driver impact.]

## Compatibility and Migration

**Serialized format changed?** [No / Yes—describe.]
**Migration / fallback**: [Required behavior or N/A.]
**Existing component/pipeline behavior**: [Preservation proof.]

## Acceptance Criteria

- [ ] [Observable user outcome.]
- [ ] Existing affected pipeline(s) preserve behavior or use an approved migration.
- [ ] Every added or modified connector, source, sink, or component exposes its MCP API.
- [ ] Every added or modified node exposes both `main` and `reject` output flows.
- [ ] UI changes reuse the existing visual language and interaction patterns, or an intentional deviation is documented and approved.
- [ ] Errors do not expose secrets.
- [ ] Relevant regression, unit, integration, and build checks are identified.

## Assumptions, Gaps, and Decisions

- **Confirmed fact**: [Evidence path/test.]
- **Gap**: [What the repository does not currently establish.]
- **Recommendation / decision needed**: [Do not present a proposed design as implemented.]
