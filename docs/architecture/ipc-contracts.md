# Tauri IPC contracts

> Source of truth for this inventory: `apps/desktop/src/lib.rs`, the Tauri
> command modules under `apps/desktop/src/`, and `frontend/src/tauri-bridge.ts`.

## Common contract rules

- Commands returning `Result<T, String>` serialize failures as strings.
- Commands returning plain values have no typed error channel in their Rust
  signature; invocation failures can still reject at the Tauri boundary.
- Pipeline, install, chat and self-update progress uses Tauri `Channel<T>`.
- The web runner maps pipeline events to HTTP/SSE rather than invoking Tauri.
- Filesystem, process and network effects below are observed from the command
  implementation; they are not security recommendations.

## Liveness, inspection and execution

| Command | Input → output | Effects, events and cancellation |
|---|---|---|
| `ping` | none → `"pong"` | none. |
| `autodetect_schema` | `format`, `options` → columns/sample payload | DuckDB inspection; CSV/TSV fallback connector; may read files or remote connector inputs; no event channel. |
| `run_pipeline` | `PipelineDoc`, optional ids/name/workspace, `Channel<PipelineEvent>` → `RunResult` | Context/env application, planner and engine, DuckDB CLI, workspace history; stage events; cancellable via `cancel_pipeline`. |
| `run_pipeline_partial` | `PipelineDoc`, `target_node_id`, ids/name/workspace, channel → `RunResult` | Upstream partial compile, per-stage execution and preview; same history/events/cancellation. |
| `run_history` | `workspace_path`, `pipeline_id` → `Vec<RunRecord>` | Reads workspace history JSON. |
| `watermark_list` | workspace + pipeline name → entries | Reads incremental state. |
| `watermark_set` | workspace, pipeline, node, kind, value, optional type → `()` | Writes watermark/snapshot state JSON. |
| `watermark_clear` | workspace, pipeline, node → `()` | Removes saved watermark state. |
| `cancel_pipeline` | none → `()` | Sets the active run cancellation flag and terminates the in-flight CLI at the engine boundary. |
| `compile_pipeline` | `PipelineDoc` → `Vec<StageSql>` | Planner/SQL compilation only; no external run. |
| `pipeline_column_lineage` | `PipelineDoc` → lineage map | Read-only planner/lineage analysis; the command accepts no workspace or secret input. |
| `pipeline_trust_report` | pipeline JSON plus optional workspace → trust report | Inspects component/runtime/security requirements; may resolve context and inspect engine state. |

## Scheduler and engines

| Command | Input → output | Effects, events and cancellation |
|---|---|---|
| `schedule_set_workspace` | path → `()` | Loads/saves `schedules.json`, rebuilds file watchers. |
| `schedule_list` | none → `Vec<Schedule>` | Reads scheduler memory. |
| `schedule_upsert` | `Schedule` → saved `Schedule` | Validates cron/interval/watch path, writes `schedules.json`, rebuilds watchers. |
| `schedule_delete` | id → `()` | Removes schedule and persists JSON. |
| `schedule_run_now` | id → `RunResult` | Resolves workspace context/env, executes engine asynchronously, updates schedule and run history. |
| `engine_status` | Tauri app handle → `Vec<EngineStatus>` | Reads app-data engine installations. |
| `engine_install` | engine id + `Channel<InstallProgress>` → installed path | Downloads/extracts engine into app-data; progress events; process/network/filesystem effects. |
| `dbt_status` | app handle → bool | Reads app-data dbt installation. |
| `dbt_install` | app handle → path | Provisions dbt via `uv`; network/filesystem/process effects. |
| `seed_sample_workspace` | app handle + workspace → bool | Writes sample workspace/pipelines and may generate data with DuckDB. |

## Settings and secrets

| Command | Input → output | Effects, events and cancellation |
|---|---|---|
| `settings_get_proxy` / `settings_set_proxy` | workspace [,+ optional URL] → option/`()` | Reads or writes workspace app settings. |
| `settings_get_memory_limit` / `settings_set_memory_limit` | workspace [,+ optional MB] → option/`()` | Reads or writes execution setting. |
| `settings_get_allow_unsigned` / `settings_set_allow_unsigned` | workspace [,+ bool] → bool/`()` | Reads or writes unsigned-extension policy. |
| `settings_get_context_file` / `settings_set_context_file` | workspace [,+ optional path] → option/`()` | Reads or writes context-file setting. |
| `settings_load_context_vars` | workspace → map | Reads configured context variables. |
| `settings_get_ai` / `settings_set_ai` | workspace [,+ `AiConfig`] → config/`()` | Reads or writes AI endpoint/model/key settings; key is sensitive. |
| `connection_encrypt_payload` | workspace + JSON payload → encrypted JSON | Writes no file itself; encrypts sensitive fields using the workspace key. |
| `connection_decrypt_payload` | workspace + JSON payload → decrypted JSON | Decrypts sensitive fields into caller memory; no event/cancellation. |

## AI and workspace Git

| Command | Input → output | Effects, events and cancellation |
|---|---|---|
| `chat_send` | history, optional workspace, `Channel<ChatEvent>` → `()` | Starts/reuses local `llama-server` or calls configured OpenAI-compatible endpoint; network/process effects; streamed token/error events. |
| `chat_extract_pipeline` | assistant text → JSON value | Parses fenced pipeline JSON; no execution. |
| `workspace_git_status` | workspace → `GitStatus` | Reads Git working tree. |
| `workspace_git_init` | workspace → `()` | Runs Git init and writes repository metadata. |
| `workspace_git_commit` | workspace + message → commit id/string | Runs Git and writes repository state. |
| `workspace_git_push` / `workspace_git_pull` | workspace → string | Runs Git network operations; PAT may be used. |
| `workspace_git_branches` | workspace → branch names | Reads Git metadata. |
| `workspace_git_branch_create` / `workspace_git_branch_checkout` | workspace + name → `()` | Mutates Git branch state. |
| `workspace_git_remote_set` | workspace + URL → `()` | Writes Git remote configuration. |
| `workspace_git_save_pat` / `workspace_git_clear_pat` | workspace [+ token] → `()` | Encrypts/removes PAT; secret side effect, no event channel. |
| `workspace_ci_status` | workspace → `CiStatus` | Network poll of GitHub/GitLab CI. |

## Update, bundle, web and MCP integration

| Command | Input → output | Effects, events and cancellation |
|---|---|---|
| `check_for_update` | none → `UpdateInfo` | Network request to release metadata; non-fatal update result. |
| `self_update` | `Channel<Progress>` → `()`/restart | Downloads, verifies checksum, replaces executable and restarts; progress events; process/filesystem/network effects. |
| `build_capabilities` | none → `BuildCapabilities` | Reports supported bundle targets/features. |
| `build_pipeline_bundle` | workspace, pipeline, output, context/secrets options → output path | Starts embedded runner, resolves context/secrets mode, stages DuckDB/sidecars and writes artifact; process/filesystem effects. |
| `open_web_panel` | app handle + workspace → URL/path | Starts or reuses local web panel process and opens a window. |
| `mcp_connection_info` | app handle → `McpConnInfo` | Stages MCP/runner binaries and returns paths/config JSON; filesystem/process metadata. |
| `connect_claude_code` | app handle → CLI output | Runs `claude mcp add`; writes external client configuration through the CLI. |
| `mcp_inject_config` | app handle + client id → config path | Reads/merges/writes Claude Desktop or Cursor MCP config; filesystem/process paths, no event channel. |

## Capabilities and missing guarantees

The default capability grants filesystem scope `**`, dialog, clipboard and
opener permissions. CSP is `null`. The IPC layer currently exposes many
commands that can spawn processes or access network/filesystem. New commands
must document these effects and their serialization in the feature plan.

No uniform generated schema or machine-checked contract exists between the
Rust command signatures and `tauri-bridge.ts`; the two sides are manually
mirrored. No frontend IPC/E2E test harness was detected.
