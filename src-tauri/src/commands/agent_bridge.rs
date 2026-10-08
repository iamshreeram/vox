use crate::managers::agent_bridge::{AgentBridgeError, AgentReply, AgentWorker, CliAgentWorker};
use crate::settings::AppSettings;
use tauri::AppHandle;

pub(crate) fn invoke_with_worker(
    settings: &AppSettings,
    prompt: &str,
    worker: &dyn AgentWorker,
) -> Result<String, String> {
    if !settings.agent_bridge_enabled {
        return Err("The agent bridge is disabled.".to_string());
    }
    if prompt.trim().is_empty() {
        return Err("The prompt must not be empty.".to_string());
    }
    worker
        .invoke(prompt)
        .map(|reply: AgentReply| reply.text)
        .map_err(|error| match error {
            AgentBridgeError::NotConfigured => {
                "No agent binary is configured. Set a binary path in Settings.".to_string()
            }
            AgentBridgeError::BinaryNotFound => {
                "The configured agent binary was not found. Check its path in Settings.".to_string()
            }
            AgentBridgeError::Timeout => "The agent process timed out.".to_string(),
            AgentBridgeError::NonZeroExit { code, stderr } => match code {
                Some(code) => format!("The agent process exited with code {code}: {stderr}"),
                None => format!("The agent process terminated unsuccessfully: {stderr}"),
            },
            AgentBridgeError::EmptyReply => "The agent returned an empty reply.".to_string(),
            AgentBridgeError::Io(error) => {
                format!("Could not run the configured agent binary: {error}")
            }
        })
}

#[tauri::command]
#[specta::specta]
pub async fn agent_invoke(app: AppHandle, prompt: String) -> Result<String, String> {
    let settings = crate::settings::get_settings(&app);
    if !settings.agent_bridge_enabled {
        return Err("The agent bridge is disabled.".to_string());
    }
    if prompt.trim().is_empty() {
        return Err("The prompt must not be empty.".to_string());
    }
    let worker = CliAgentWorker::from_settings(
        settings.agent_bridge_binary_path.clone(),
        settings.agent_bridge_prompt_flag.clone(),
        settings.agent_bridge_timeout_secs,
    )
    .map_err(|error| match error {
        AgentBridgeError::NotConfigured => {
            "No agent binary is configured. Set a binary path in Settings.".to_string()
        }
        AgentBridgeError::BinaryNotFound => {
            "The configured agent binary was not found. Check its path in Settings.".to_string()
        }
        _ => "Could not configure the agent bridge.".to_string(),
    })?;
    tauri::async_runtime::spawn_blocking(move || invoke_with_worker(&settings, &prompt, &worker))
        .await
        .map_err(|error| format!("Agent bridge task failed: {error}"))?
}
