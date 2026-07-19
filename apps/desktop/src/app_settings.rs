//! Per-workspace app settings (currently just an HTTP/HTTPS proxy), persisted to
//! `<workspace>/.duckle/settings.json`. The proxy is pushed into the engine's
//! shared HTTP layer via `duckle_duckdb_engine::tls::set_proxy`, so a user on a
//! locked-down corporate machine can route REST / cloud connectors and the
//! in-app updater through a proxy WITHOUT setting any system environment
//! variable (issue #80). Applied on startup and on every workspace switch.

use duckle_db_runner::resources::{
    LegacyRunnerResources, ResourceClampReason, RunnerResourcesProfile,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct AppSettings {
    /// HTTP/HTTPS proxy URL, e.g. "http://user:pass@proxy:8080". None / empty
    /// means a direct connection.
    https_proxy: Option<String>,
    /// #92: optional external OpenAI-compatible endpoint for the Duckie AI
    /// assistant (base URL, e.g. https://api.openai.com or an Ollama/LM Studio
    /// URL). When set, chat goes to it instead of the local Qwen model.
    ai_base_url: Option<String>,
    /// Model id for the external endpoint (e.g. "gpt-4o-mini", "llama3.1").
    ai_model: Option<String>,
    /// API key for the external endpoint (sent as `Authorization: Bearer ...`).
    /// Stored alongside the proxy creds in the workspace's local .duckle dir.
    ai_api_key: Option<String>,
    /// #102: total DuckDB memory cap in MB, applied as DUCKLE_MEMORY_LIMIT for
    /// every run in this workspace (batched and per-stage). None = DuckDB
    /// default (~80% of RAM). Stages run sequentially, so this caps peak RAM.
    memory_limit_mb: Option<u32>,
    /// Complete Feature 003 runner profile. It is optional only so existing
    /// workspaces deserialize unchanged; `memory_limit_mb` is migrated on read.
    runner_resources: Option<RunnerResourcesProfile>,
    /// Path to a key/value file (.env / .properties / .csv / .json) whose
    /// entries auto-load into the global context for every run in this
    /// workspace, so ${KEY} resolves without wiring a node. Relative paths
    /// resolve against the workspace root.
    context_file: Option<String>,
    /// #143: allow loading UNSIGNED / community DuckDB extensions (e.g. a custom
    /// `quack` build). Applied as DUCKLE_ALLOW_UNSIGNED_EXTENSIONS, which makes the
    /// engine pass `-unsigned` to the DuckDB CLI. None/false = signed-only (default).
    allow_unsigned_extensions: Option<bool>,
}

/// The external-AI config returned to the Settings UI. camelCase for JS.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiConfig {
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub api_key: Option<String>,
}

/// Additive DTO for Settings. `effective` is the currently accepted complete
/// profile; a live WorkerPoolControl refines it per worker before readiness.
/// It deliberately contains no runner endpoint, PID, path, SQL or credential.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunnerResourcesConfig {
    pub requested: RunnerResourcesProfile,
    pub effective: RunnerResourcesProfile,
    pub diagnostics: Vec<ResourceClampReason>,
}

fn settings_path(workspace: &Path) -> PathBuf {
    workspace.join(".duckle").join("settings.json")
}

fn load(workspace: &Path) -> AppSettings {
    match std::fs::read(settings_path(workspace)) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
        Err(_) => AppSettings::default(),
    }
}

fn store(workspace: &Path, s: &AppSettings) -> Result<(), String> {
    let dir = workspace.join(".duckle");
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let json = serde_json::to_string_pretty(s).map_err(|e| e.to_string())?;
    std::fs::write(settings_path(workspace), json).map_err(|e| format!("write settings: {e}"))
}

fn runner_resources(settings: &AppSettings) -> RunnerResourcesProfile {
    settings.runner_resources.clone().unwrap_or_else(|| {
        RunnerResourcesProfile::from_legacy(LegacyRunnerResources {
            memory_limit_mb: settings.memory_limit_mb,
        })
    })
}

/// Load the runner resources profile for a workspace, with legacy migration.
pub fn load_runner_resources(workspace: &Path) -> RunnerResourcesProfile {
    runner_resources(&load(workspace))
}

fn runner_resources_config(settings: &AppSettings) -> RunnerResourcesConfig {
    let profile = runner_resources(settings);
    RunnerResourcesConfig {
        requested: profile.clone(),
        effective: profile,
        diagnostics: Vec::new(),
    }
}

/// Load the workspace's saved proxy and apply it to the engine HTTP layer.
/// Best-effort: a missing / unreadable settings file leaves the current
/// (environment-derived) proxy in place.
pub fn apply_for_workspace(workspace: &str) {
    if workspace.is_empty() {
        return;
    }
    let s = load(Path::new(workspace));
    let proxy = s
        .https_proxy
        .clone()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if proxy.is_some() {
        duckle_duckdb_engine::tls::set_proxy(proxy);
    }
    // #102: apply the per-workspace memory cap as DUCKLE_MEMORY_LIMIT (the env
    // var the engine reads in both batched and per-stage modes).
    if let Some(mb) = s.memory_limit_mb.filter(|m| *m > 0) {
        std::env::set_var("DUCKLE_MEMORY_LIMIT", format!("{}MB", mb));
    }
    // #143: opt-in unsigned-extension loading for this workspace.
    if s.allow_unsigned_extensions == Some(true) {
        std::env::set_var("DUCKLE_ALLOW_UNSIGNED_EXTENSIONS", "1");
    }
}

#[tauri::command]
pub fn settings_get_proxy(workspace: String) -> Option<String> {
    if workspace.is_empty() {
        return None;
    }
    load(Path::new(&workspace))
        .https_proxy
        .filter(|s| !s.trim().is_empty())
}

#[tauri::command]
pub fn settings_set_proxy(workspace: String, url: Option<String>) -> Result<(), String> {
    if workspace.is_empty() {
        return Err("no workspace is open".into());
    }
    let url = url.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let mut s = load(Path::new(&workspace));
    s.https_proxy = url.clone();
    store(Path::new(&workspace), &s)?;
    // Apply immediately so the current session uses it without a relaunch.
    duckle_duckdb_engine::tls::set_proxy(url);
    Ok(())
}

#[tauri::command]
pub fn settings_get_memory_limit(workspace: String) -> Option<u32> {
    if workspace.is_empty() {
        return None;
    }
    load(Path::new(&workspace))
        .memory_limit_mb
        .filter(|m| *m > 0)
}

#[tauri::command]
pub fn settings_set_memory_limit(workspace: String, mb: Option<u32>) -> Result<(), String> {
    if workspace.is_empty() {
        return Err("no workspace is open".into());
    }
    let mb = mb.filter(|m| *m > 0);
    let mut s = load(Path::new(&workspace));
    s.memory_limit_mb = mb;
    // Keep the legacy UI command coherent with the complete profile while the
    // CLI compatibility route is still present. The profile version advances
    // atomically even when only its migrated memory field is changed.
    let mut profile = runner_resources(&s);
    profile.version = profile.version.saturating_add(1).max(1);
    profile.memory = mb
        .filter(|value| *value > 0)
        .map(|value| {
            duckle_db_runner::resources::ResourceLimit::Bytes(u64::from(value) * 1024 * 1024)
        })
        .unwrap_or(duckle_db_runner::resources::ResourceLimit::Automatic);
    s.runner_resources = Some(profile);
    store(Path::new(&workspace), &s)?;
    // Apply immediately so the current session's runs use it without a relaunch.
    match mb {
        Some(m) => std::env::set_var("DUCKLE_MEMORY_LIMIT", format!("{}MB", m)),
        None => std::env::remove_var("DUCKLE_MEMORY_LIMIT"),
    }
    Ok(())
}

#[tauri::command]
pub fn settings_get_runner_resources(workspace: String) -> RunnerResourcesConfig {
    if workspace.is_empty() {
        return runner_resources_config(&AppSettings::default());
    }
    runner_resources_config(&load(Path::new(&workspace)))
}

#[tauri::command]
pub fn settings_set_runner_resources(
    workspace: String,
    mut profile: RunnerResourcesProfile,
) -> Result<RunnerResourcesConfig, String> {
    if workspace.is_empty() {
        return Err("no workspace is open".into());
    }
    let mut settings = load(Path::new(&workspace));
    let current = runner_resources(&settings);
    // The persisted generation is the authority. A UI may resend the profile
    // it just read, so turn a non-advancing version into the next generation.
    if profile.version <= current.version {
        profile.version = current.version.saturating_add(1).max(1);
    }
    profile
        .validate()
        .map_err(|_| "invalid_runner_resources".to_string())?;
    settings.runner_resources = Some(profile);
    store(Path::new(&workspace), &settings)?;
    Ok(runner_resources_config(&settings))
}

#[tauri::command]
pub fn settings_get_allow_unsigned(workspace: String) -> bool {
    if workspace.is_empty() {
        return false;
    }
    load(Path::new(&workspace)).allow_unsigned_extensions == Some(true)
}

#[tauri::command]
pub fn settings_set_allow_unsigned(workspace: String, allow: bool) -> Result<(), String> {
    if workspace.is_empty() {
        return Err("no workspace is open".into());
    }
    let mut s = load(Path::new(&workspace));
    s.allow_unsigned_extensions = if allow { Some(true) } else { None };
    store(Path::new(&workspace), &s)?;
    // Apply immediately so the current session's runs use it without a relaunch.
    if allow {
        std::env::set_var("DUCKLE_ALLOW_UNSIGNED_EXTENSIONS", "1");
    } else {
        std::env::remove_var("DUCKLE_ALLOW_UNSIGNED_EXTENSIONS");
    }
    Ok(())
}

#[tauri::command]
pub fn settings_get_context_file(workspace: String) -> Option<String> {
    if workspace.is_empty() {
        return None;
    }
    load(Path::new(&workspace))
        .context_file
        .filter(|s| !s.trim().is_empty())
}

#[tauri::command]
pub fn settings_set_context_file(workspace: String, path: Option<String>) -> Result<(), String> {
    if workspace.is_empty() {
        return Err("no workspace is open".into());
    }
    let path = path.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let mut s = load(Path::new(&workspace));
    s.context_file = path;
    store(Path::new(&workspace), &s)
}

/// Resolve the global-context key/value file into a flat var map for the
/// desktop run path (the frontend pre-substitutes ${...} before the engine
/// sees the pipeline; the headless runner / web server resolve it engine-side
/// via context_vars_for_workspace).
#[tauri::command]
pub fn settings_load_context_vars(workspace: String) -> std::collections::HashMap<String, String> {
    if workspace.is_empty() {
        return std::collections::HashMap::new();
    }
    duckle_duckdb_engine::context::context_file_vars(Path::new(&workspace))
}

#[tauri::command]
pub fn settings_get_ai(workspace: String) -> AiConfig {
    if workspace.is_empty() {
        return AiConfig::default();
    }
    let s = load(Path::new(&workspace));
    let clean = |o: Option<String>| o.map(|x| x.trim().to_string()).filter(|x| !x.is_empty());
    AiConfig {
        base_url: clean(s.ai_base_url),
        model: clean(s.ai_model),
        api_key: clean(s.ai_api_key),
    }
}

#[tauri::command]
pub fn settings_set_ai(
    workspace: String,
    base_url: Option<String>,
    model: Option<String>,
    api_key: Option<String>,
) -> Result<(), String> {
    if workspace.is_empty() {
        return Err("no workspace is open".into());
    }
    let clean = |o: Option<String>| o.map(|x| x.trim().to_string()).filter(|x| !x.is_empty());
    let mut s = load(Path::new(&workspace));
    s.ai_base_url = clean(base_url);
    s.ai_model = clean(model);
    s.ai_api_key = clean(api_key);
    store(Path::new(&workspace), &s)
}

/// Internal: the workspace's external-AI config (base_url, model, api_key) for
/// chat routing. All None when no external endpoint is configured.
pub fn ai_config(workspace: &str) -> (Option<String>, Option<String>, Option<String>) {
    if workspace.is_empty() {
        return (None, None, None);
    }
    let s = load(Path::new(workspace));
    let clean = |o: Option<String>| o.map(|x| x.trim().to_string()).filter(|x| !x.is_empty());
    (clean(s.ai_base_url), clean(s.ai_model), clean(s.ai_api_key))
}

#[cfg(test)]
mod runner_resource_tests {
    use super::*;
    use duckle_db_runner::resources::{AutomaticOrU16, ResourceLimit};

    #[test]
    fn legacy_memory_settings_load_as_a_complete_defaulted_profile() {
        let workspace = tempfile::tempdir().unwrap();
        let settings_dir = workspace.path().join(".duckle");
        std::fs::create_dir_all(&settings_dir).unwrap();
        std::fs::write(
            settings_dir.join("settings.json"),
            r#"{"memory_limit_mb":256,"https_proxy":"http://proxy.invalid"}"#,
        )
        .unwrap();

        let config = settings_get_runner_resources(workspace.path().to_string_lossy().into());

        assert_eq!(config.requested.version, 1);
        assert_eq!(
            config.requested.memory,
            ResourceLimit::Bytes(256 * 1024 * 1024)
        );
        assert_eq!(
            config.requested.quack_parallelism,
            AutomaticOrU16::Automatic
        );
        assert_eq!(config.requested.base_capacity, 3);
        assert_eq!(config.requested, config.effective);
        assert!(config.diagnostics.is_empty());
    }

    #[test]
    fn complete_profile_save_is_atomic_and_advances_the_persisted_generation() {
        let workspace = tempfile::tempdir().unwrap();
        let workspace_path = workspace.path().to_string_lossy().into_owned();
        let profile = RunnerResourcesProfile {
            version: 1,
            memory: ResourceLimit::Percent(60),
            cpu_threads: AutomaticOrU16::Value(4),
            spill: ResourceLimit::Bytes(1_024),
            quack_parallelism: AutomaticOrU16::Value(3),
            base_capacity: 5,
        };

        let saved = settings_set_runner_resources(workspace_path.clone(), profile).unwrap();
        let loaded = settings_get_runner_resources(workspace_path);

        assert_eq!(saved.requested.version, 2);
        assert_eq!(saved.requested, loaded.requested);
        assert_eq!(saved.effective, loaded.effective);
        let persisted = std::fs::read_to_string(settings_path(workspace.path())).unwrap();
        assert!(persisted.contains("\"runner_resources\""));
        assert!(persisted.contains("\"baseCapacity\": 5"));
    }

    #[test]
    fn invalid_complete_profile_is_rejected_without_creating_settings() {
        let workspace = tempfile::tempdir().unwrap();
        let invalid = RunnerResourcesProfile {
            quack_parallelism: AutomaticOrU16::Value(9),
            ..RunnerResourcesProfile::default()
        };

        assert!(matches!(
            settings_set_runner_resources(workspace.path().to_string_lossy().into(), invalid),
            Err(ref error) if error == "invalid_runner_resources"
        ));
        assert!(!settings_path(workspace.path()).exists());
    }
}
