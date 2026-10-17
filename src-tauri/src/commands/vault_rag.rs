//! Tauri commands for the vault RAG settings, status, reindex and preview (DESIGN Rev 5, 3.6).
//! Each command body delegates to a pure `*_inner` fn that takes explicit arguments so the logic
//! is testable without a Tauri app. The status and preview reads go through the runtime's gate,
//! so they run on a plain thread and never block the UI or an async task. The probe command
//! runs one bounded model turn and records the result with its route key. Nothing here logs note
//! text, queries, paths or route keys.

use std::sync::Arc;
use std::thread;

use tauri::{AppHandle, Manager};

use crate::commands::conversation_records::validate_vault_path;
use crate::managers::vault_rag_runtime::{VaultHitDto, VaultIndexStatusDto, VaultRagRuntime};
use crate::settings::{get_settings, parse_rag_folders, write_settings, AppSettings};

/// Fixed message when the runtime is not managed (init failure, DESIGN 3.6).
pub const MSG_UNAVAILABLE: &str = "vault search is unavailable";
/// Fixed message when the feature is off.
pub const MSG_NOT_ENABLED: &str = "vault search is not enabled";

/// Status shown when the runtime is not managed (DESIGN 3.6).
fn unavailable_status() -> VaultIndexStatusDto {
    VaultIndexStatusDto {
        notes: 0,
        chunks: 0,
        skipped: 0,
        last_refresh_ms: None,
        refreshing: false,
        last_error_kind: Some("unavailable".to_string()),
        tool_support: "untested".to_string(),
    }
}

/// Sets the RAG switch and, when a runtime exists, applies the new snapshot once (DESIGN 3.6).
/// Turning RAG on needs a runtime: without one the change is refused and nothing is stored.
pub(crate) fn change_rag_enabled_inner(
    settings: &mut AppSettings,
    runtime: Option<&VaultRagRuntime>,
    enabled: bool,
) -> Result<(), String> {
    if enabled && runtime.is_none() {
        return Err(MSG_UNAVAILABLE.to_string());
    }
    settings.vault_rag_enabled = enabled;
    if let Some(runtime) = runtime {
        runtime.apply(settings);
    }
    Ok(())
}

/// Parses and stores the folder list. A parse error returns Err and changes nothing (DESIGN 3.6).
pub(crate) fn change_rag_folders_inner(
    settings: &mut AppSettings,
    runtime: Option<&VaultRagRuntime>,
    folders: &str,
) -> Result<(), String> {
    let parsed = parse_rag_folders(folders)?;
    if runtime.is_none() {
        return Err(MSG_UNAVAILABLE.to_string());
    }
    settings.vault_rag_folders = parsed.join(",");
    if let Some(runtime) = runtime {
        runtime.apply(settings);
    }
    Ok(())
}

/// Stores a validated vault path and applies the new snapshot (DESIGN 3.6). Without a runtime the
/// path is still stored, as before the RAG existed; only the apply step is skipped.
pub(crate) fn change_vault_path_inner(
    settings: &mut AppSettings,
    runtime: Option<&VaultRagRuntime>,
    raw_path: &str,
) -> Result<(), String> {
    settings.vault_path = validate_vault_path(raw_path).map_err(str::to_string)?;
    if let Some(runtime) = runtime {
        runtime.apply(settings);
    }
    Ok(())
}

/// Status DTO; `None` (init failure) gives zeros with last_error_kind "unavailable" (DESIGN 3.6).
pub(crate) fn status_inner(runtime: Option<&VaultRagRuntime>) -> VaultIndexStatusDto {
    match runtime {
        Some(runtime) => runtime.status_dto(),
        None => unavailable_status(),
    }
}

/// Requests a refresh-only pass. Off gives `MSG_NOT_ENABLED`; no runtime gives `MSG_UNAVAILABLE`.
pub(crate) fn reindex_inner(
    settings: &AppSettings,
    runtime: Option<&VaultRagRuntime>,
) -> Result<(), String> {
    let runtime = runtime.ok_or_else(|| MSG_UNAVAILABLE.to_string())?;
    if !settings.vault_rag_enabled {
        return Err(MSG_NOT_ENABLED.to_string());
    }
    runtime.request_reindex();
    Ok(())
}

/// Preview search. Requires enabled (no index call otherwise); no runtime gives `MSG_UNAVAILABLE`.
pub(crate) fn preview_inner(
    settings: &AppSettings,
    runtime: Option<&VaultRagRuntime>,
    query: &str,
) -> Result<Vec<VaultHitDto>, String> {
    let runtime = runtime.ok_or_else(|| MSG_UNAVAILABLE.to_string())?;
    if !settings.vault_rag_enabled {
        return Err(MSG_NOT_ENABLED.to_string());
    }
    runtime.preview(query)
}

/// Returns the managed runtime, if the init succeeded (DESIGN 3.6). The state holds an
/// `Option`, so an init failure is a managed `None`, never a missing state.
pub(crate) fn runtime_of(app: &AppHandle) -> Option<Arc<VaultRagRuntime>> {
    app.try_state::<Option<Arc<VaultRagRuntime>>>()
        .and_then(|state| state.inner().clone())
}

/// Runs a gated read on a plain thread and waits for it without blocking the caller. The gate
/// drives Tokio, so it must not run inside a Tokio task. `None` when the thread or the work
/// failed.
async fn run_detached<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    thread::Builder::new()
        .name("vault-rag-query".to_string())
        .spawn(move || {
            let _ = sender.send(work());
        })
        .ok()?;
    receiver.await.ok()
}

/// Turns the RAG switch on or off (DESIGN 3.6).
#[tauri::command]
#[specta::specta]
pub fn change_vault_rag_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = get_settings(&app);
    let runtime = runtime_of(&app);
    change_rag_enabled_inner(&mut settings, runtime.as_deref(), enabled)?;
    write_settings(&app, settings);
    Ok(())
}

/// Stores the vault folder list from the settings form (DESIGN 3.6).
#[tauri::command]
#[specta::specta]
pub fn change_vault_rag_folders_setting(app: AppHandle, folders: String) -> Result<(), String> {
    let mut settings = get_settings(&app);
    let runtime = runtime_of(&app);
    change_rag_folders_inner(&mut settings, runtime.as_deref(), &folders)?;
    write_settings(&app, settings);
    Ok(())
}

/// Index counters and the tool-support state for the settings page (DESIGN 3.6).
#[tauri::command]
#[specta::specta]
pub async fn vault_index_status(app: AppHandle) -> VaultIndexStatusDto {
    let runtime = runtime_of(&app);
    let settings = get_settings(&app);
    run_detached(move || status_with_support(runtime.as_deref(), &settings))
        .await
        .unwrap_or_else(unavailable_status)
}

/// The status DTO with `tool_support` for the CURRENT route key (DESIGN 3.6). "untested" when no
/// tool route is active.
fn status_with_support(
    runtime: Option<&VaultRagRuntime>,
    settings: &AppSettings,
) -> VaultIndexStatusDto {
    let mut dto = status_inner(runtime);
    if let Some(runtime) = runtime {
        dto.tool_support = match tool_route(settings) {
            Some(prepared) => runtime
                .probe_state_for(&route_key_of(&prepared))
                .as_str()
                .to_string(),
            None => "untested".to_string(),
        };
    }
    dto
}

/// Asks for a refresh of the applied vault (DESIGN 3.6).
#[tauri::command]
#[specta::specta]
pub fn vault_reindex(app: AppHandle) -> Result<(), String> {
    let settings = get_settings(&app);
    reindex_inner(&settings, runtime_of(&app).as_deref())
}

/// Runs a preview search over the applied vault (DESIGN 3.6).
#[tauri::command]
#[specta::specta]
pub async fn vault_search_preview(
    app: AppHandle,
    query: String,
) -> Result<Vec<VaultHitDto>, String> {
    let settings = get_settings(&app);
    let runtime = runtime_of(&app);
    run_detached(move || preview_inner(&settings, runtime.as_deref(), &query))
        .await
        .unwrap_or_else(|| Err(MSG_UNAVAILABLE.to_string()))
}

// ---------------------------------------------------------------------
// Tool probe command (DESIGN 3.6).
// ---------------------------------------------------------------------

use crate::managers::converse::BoxFuture;
use crate::managers::converse_platform::{
    resolve_tool_proxy, route_key_of, run_probe_with_proxy, tool_route,
};
use crate::managers::llm_stream::ProxyMode;
use crate::managers::llm_tools::{ToolRisk, ToolSpec};
use crate::managers::realtime_complete::Prepared;
use crate::managers::tool_loop::{ToolExecutor, ToolOutput};
use serde_json::json;
use std::time::Duration;

/// Turn budget of the tool probe (DESIGN 3.6).
const PROBE_DEADLINE: Duration = Duration::from_secs(15);

/// The probe echo tool: returns the trusted word "ping" (DESIGN 3.6).
struct ProbeEcho;

impl ToolExecutor for ProbeEcho {
    fn specs(&self) -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: "vox_probe_echo".to_string(),
            description: "Echoes the probe word back.".to_string(),
            input_schema: json!({"type": "object",
                "properties": {"word": {"type": "string"}}, "required": ["word"]}),
            risk: ToolRisk::ReadOnly,
        }]
    }

    fn call<'a>(
        &'a self,
        _name: &'a str,
        _input: &'a serde_json::Value,
    ) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async {
            ToolOutput {
                text: "ping".to_string(),
                is_error: false,
                untrusted: false,
            }
        })
    }
}

/// Probe logic with explicit inputs (DESIGN 3.6). `route` is `tool_route(settings)`; `exec` is the
/// probe echo executor. Records the result with its route key in `runtime` (when managed) and
/// returns the classification label, or the Err text for a refused probe.
#[cfg(test)]
pub(crate) async fn probe_inner(
    runtime: Option<&VaultRagRuntime>,
    route: Option<Prepared>,
    deadline: Duration,
    exec: Arc<dyn ToolExecutor>,
) -> Result<String, String> {
    probe_inner_with_proxy(runtime, route, deadline, exec, ProxyMode::SystemDefault).await
}

/// `probe_inner` with the proxy a real turn would use (DESIGN 3.6).
async fn probe_inner_with_proxy(
    runtime: Option<&VaultRagRuntime>,
    route: Option<Prepared>,
    deadline: Duration,
    exec: Arc<dyn ToolExecutor>,
    proxy: ProxyMode,
) -> Result<String, String> {
    let key = route.as_ref().map(route_key_of);
    let support = run_probe_with_proxy(route, deadline, exec, proxy).await?;
    if let (Some(runtime), Some(key)) = (runtime, key) {
        runtime.record_probe(key, support);
    }
    Ok(support.as_str().to_string())
}

// Tests whether the current route supports tool calls (DESIGN 3.6). Plain comment: specta emits
// no doc block for it, which keeps the bindings splice to the wrapper itself.
#[tauri::command]
#[specta::specta]
pub async fn vault_rag_probe(app: AppHandle) -> Result<String, String> {
    let settings = get_settings(&app);
    let runtime = runtime_of(&app);
    let route = tool_route(&settings);
    let proxy = resolve_tool_proxy(&settings).await;
    let exec: Arc<dyn ToolExecutor> = Arc::new(ProbeEcho);
    probe_inner_with_proxy(runtime.as_deref(), route, PROBE_DEADLINE, exec, proxy).await
}
