from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected one match, found {count}")
    return text.replace(old, new, 1)


main_path = Path("crates/duckle-runner/src/main.rs")
main = main_path.read_text(encoding="utf-8")
main = replace_once(
    main,
    "use duckle_duckdb_engine::{DuckdbEngine, PipelineDoc};",
    "use duckle_duckdb_engine::PipelineDoc;",
    "main import",
)
main = replace_once(
    main,
    "mod manifest;\n",
    "mod manifest;\nmod runner_controller;\n",
    "main module",
)
main = replace_once(
    main,
    "    let engine = DuckdbEngine::new(duckdb);\n    let result = engine.execute_pipeline_named(&doc, &name);",
    "    let engine = runner_controller::engine_for_workspace(duckdb, &workspace).for_new_run();\n    let result = engine.execute_pipeline_named(&doc, &name);",
    "main engine route",
)
main_path.write_text(main, encoding="utf-8")

serve_path = Path("crates/duckle-runner/src/serve.rs")
serve = serve_path.read_text(encoding="utf-8")
serve = replace_once(
    serve,
    "//! Runs execute in-process through the same engine as `duckle-runner run`, are\n//! serialized by a single lock (so a manual run and a scheduled run never\n//! collide on the shared workspace env), and append the same run history\n",
    "//! Runs execute through one workspace-owned controller and independent\n//! per-run cancellation scopes. Manual, scheduled, and browser requests may\n//! overlap without a global admission queue, and append the same run history\n",
    "serve module docs",
)
serve = replace_once(
    serve,
    "    duckdb: PathBuf,\n    /// Serializes pipeline execution: the shared workspace env vars and DuckDB\n    /// process make concurrent runs unsafe, so manual + scheduled runs queue.\n    run_lock: Mutex<()>,\n",
    "    duckdb: PathBuf,\n    /// Base engine carrying the single workspace-owned runner controller. Each\n    /// execution derives an independent run scope with `for_new_run`.\n    engine: DuckdbEngine,\n",
    "management state controller",
)
serve = replace_once(
    serve,
    "    // Set the workspace env once for the process; runs are serialized so these\n    // stay consistent for every execution (matches the runner's run path).\n",
    "    // Set immutable workspace-scoped environment once for this server.\n    // Concurrent runs all target the same workspace and use independent engines.\n",
    "management environment comment",
)
serve = replace_once(
    serve,
    "    apply_workspace_memory_limit(&workspace);\n\n    let state = Arc::new(State {\n        workspace: workspace.clone(),\n        duckdb: duckdb.clone(),\n        run_lock: Mutex::new(()),\n        running: Mutex::new(std::collections::HashSet::new()),\n",
    "    apply_workspace_memory_limit(&workspace);\n    let engine = crate::runner_controller::engine_for_workspace(duckdb.clone(), &workspace);\n\n    let state = Arc::new(State {\n        workspace: workspace.clone(),\n        duckdb: duckdb.clone(),\n        engine,\n        running: Mutex::new(std::collections::HashSet::new()),\n",
    "management state initialization",
)
serve = replace_once(
    serve,
    "    /// Bind host, for the cross-origin / DNS-rebind guard on POST routes.\n    host: String,\n    /// Serialize runs: the shared workspace env + DuckDB process make concurrent\n    /// executions unsafe, so browser run requests queue.\n    run_lock: Mutex<()>,\n",
    "    /// Bind host, for the cross-origin / DNS-rebind guard on POST routes.\n    host: String,\n    /// Base engine carrying the single workspace-owned runner controller.\n    engine: DuckdbEngine,\n",
    "web state controller",
)
serve = replace_once(
    serve,
    "    apply_workspace_memory_limit(&workspace);\n    let state = Arc::new(WebState {\n        workspace: workspace.clone(),\n        duckdb: duckdb.clone(),\n        dist: dist.clone(),\n        host: args.host.clone(),\n        run_lock: Mutex::new(()),\n    });\n",
    "    apply_workspace_memory_limit(&workspace);\n    let engine = crate::runner_controller::engine_for_workspace(duckdb.clone(), &workspace);\n    let state = Arc::new(WebState {\n        workspace: workspace.clone(),\n        duckdb: duckdb.clone(),\n        dist: dist.clone(),\n        host: args.host.clone(),\n        engine,\n    });\n",
    "web state initialization",
)
serve = replace_once(
    serve,
    "        // streamed in the MVP. Runs are serialized via run_lock.\n",
    "        // streamed in the MVP. The workspace controller admits each run\n        // directly; there is no web-side queue or global execution lock.\n",
    "web command comment",
)
serve = replace_once(
    serve,
    "            let _guard = state.run_lock.lock().unwrap_or_else(|p| p.into_inner());\n            let engine = DuckdbEngine::new(state.duckdb.clone());\n            let result = engine.execute_pipeline_named(&doc, &name);",
    "            let engine = state.engine.for_new_run();\n            let result = engine.execute_pipeline_named(&doc, &name);",
    "web command execution",
)
serve = replace_once(
    serve,
    "    let _guard = state.run_lock.lock().unwrap_or_else(|p| p.into_inner());\n    // A second handle to the same socket for the event callback (the run is\n",
    "    // A second handle to the same socket for the event callback (the run is\n",
    "stream lock removal",
)
serve = replace_once(
    serve,
    "    let engine = DuckdbEngine::new(state.duckdb.clone());\n    let result = engine.execute_pipeline_with_events(&doc, target.as_deref(), Some(&name), |evt| {",
    "    let engine = state.engine.for_new_run();\n    let result = engine.execute_pipeline_with_events(&doc, target.as_deref(), Some(&name), |evt| {",
    "stream controller execution",
)
serve = replace_once(
    serve,
    "/// append a run-history record, and return a result summary. Serialized by the\n/// run lock so a scheduled run never overlaps a manual one.\n",
    "/// append a run-history record, and return a result summary. Manual and\n/// scheduled runs may overlap; WorkerPoolControl owns admission and allocation.\n",
    "management execution docs",
)
serve = replace_once(
    serve,
    "    let _guard = state.run_lock.lock().map_err(|_| \"run lock poisoned\".to_string())?;\n\n",
    "",
    "management lock removal",
)
serve = replace_once(
    serve,
    "    let engine = DuckdbEngine::new(state.duckdb.clone());\n    let result = engine.execute_pipeline_named(&doc, &id);",
    "    let engine = state.engine.for_new_run();\n    let result = engine.execute_pipeline_named(&doc, &id);",
    "management controller execution",
)
if "run_lock" in serve:
    raise SystemExit("serve still contains run_lock after verified replacements")
serve_path.write_text(serve, encoding="utf-8")
