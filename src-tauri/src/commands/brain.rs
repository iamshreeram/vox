//! Brain commands (P3, DESIGN Rev 3-6). Main window only: `capabilities/chat.json` never grants
//! them. No command returns or accepts the stored token except `set_brain_token`, which stores
//! the token the user typed. Errors are fixed texts and never echo an address or server reply.

use crate::managers::brain::{self, BrainFeature, BrainTestResult};
use crate::settings::{get_settings, write_settings_gated, AppSettings, SecretString};
use tauri::AppHandle;

/// Shown before any network request when the user has not acknowledged what a test sends.
const ACK_REQUIRED: &str = "Acknowledge what the test sends first.";

/// Saves the Brain provider, endpoint address and model. A new destination clears the test
/// acknowledgement and the transcript consent.
#[tauri::command]
#[specta::specta]
pub fn change_brain_connection_setting(
    app: AppHandle,
    provider_id: String,
    base_url: String,
    model: String,
) -> Result<(), String> {
    let mut outcome = Ok(());
    write_settings_gated(&app, |settings| {
        outcome = brain::apply_connection(settings, &provider_id, &base_url, &model);
    });
    outcome
}

/// Stores the Brain access token the user typed. An empty value clears it. The value is trimmed.
#[tauri::command]
#[specta::specta]
pub fn set_brain_token(app: AppHandle, token: String) -> Result<(), String> {
    let token = token.trim().to_string();
    write_settings_gated(&app, |settings| {
        settings.brain_token_set = !token.is_empty();
        settings.brain_token = SecretString::from(token.clone());
    });
    Ok(())
}

/// Records that the user has read what a connection test sends to the current destination.
/// Requires a complete connection (provider, endpoint and model).
#[tauri::command]
#[specta::specta]
pub fn brain_acknowledge_test(app: AppHandle) -> Result<(), String> {
    let mut outcome = Ok(());
    write_settings_gated(&app, |settings| {
        outcome = brain::acknowledge_test(settings);
    });
    outcome
}

/// Makes `feature` inherit the Brain (`use_brain`) or use its own settings again. Inheriting
/// requires a complete connection; the only piece it may still lack is transcript consent,
/// which this call records for the current destination.
#[tauri::command]
#[specta::specta]
pub fn brain_use_for_feature(
    app: AppHandle,
    feature: BrainFeature,
    use_brain: bool,
) -> Result<(), String> {
    let mut outcome = Ok(());
    write_settings_gated(&app, |settings| {
        outcome = if use_brain {
            brain::use_brain_checked(settings, feature)
        } else {
            brain::use_own_for(settings, feature);
            Ok(())
        };
    });
    outcome
}

/// Sends the fixed 1-token test request to the acknowledged endpoint. The token is read from the
/// store here, not received from the webview.
#[tauri::command]
#[specta::specta]
pub async fn brain_test_connection(app: AppHandle) -> Result<BrainTestResult, String> {
    let connection = network_connection(&app)?;
    Ok(brain::run_test(&connection).await)
}

/// Lists the models of the acknowledged endpoint. The token is read from the store here.
#[tauri::command]
#[specta::specta]
pub async fn brain_fetch_models(app: AppHandle) -> Result<Vec<String>, String> {
    let connection = network_connection(&app)?;
    brain::list_models(&connection)
        .await
        .map_err(|kind| brain::test_kind_message(kind).to_string())
}

/// The connection for a network command. Refuses unless the test acknowledgement was given for
/// the current destination, so no request is ever made before the user has seen the disclosure.
fn network_connection(app: &AppHandle) -> Result<brain::BrainConnection, String> {
    let settings: AppSettings = get_settings(app);
    if !brain::ack_allows_network(&settings) {
        return Err(ACK_REQUIRED.to_string());
    }
    brain::connection_for_network(&settings)
}
