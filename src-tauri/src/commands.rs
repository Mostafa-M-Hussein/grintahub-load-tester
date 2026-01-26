//! Tauri commands - thin wrappers around shared bot logic.
//!
//! This module is only compiled when the `desktop` feature is enabled.
//! All business logic lives in `crate::bot`.

use tauri::State;
use tracing::info;

use crate::AppState;
use crate::AppConfig;
use crate::browser::SessionInfo;
use crate::stats::GlobalStatsSnapshot;
use crate::scheduler::{ScheduleConfig, ScheduleStatus};
use crate::bot;

/// Configure the application
#[tauri::command]
pub async fn configure(
    state: State<'_, AppState>,
    config: AppConfig,
) -> Result<(), String> {
    info!("Configuring application");
    state.configure(config).await;
    Ok(())
}

/// Get current configuration
#[tauri::command]
pub async fn get_config(state: State<'_, AppState>) -> Result<AppConfig, String> {
    Ok(state.config.read().await.clone())
}

/// Start browser sessions
#[tauri::command]
pub async fn start_sessions(
    state: State<'_, AppState>,
    count: usize,
) -> Result<Vec<String>, String> {
    info!("Starting {} browser sessions", count);
    state.browser_pool
        .spawn_sessions(count)
        .await
        .map_err(|e| e.to_string())
}

/// Stop all browser sessions
#[tauri::command]
pub async fn stop_sessions(state: State<'_, AppState>) -> Result<(), String> {
    info!("Stopping all browser sessions");
    state.browser_pool.close_all().await.map_err(|e| e.to_string())
}

/// Get information about all sessions
#[tauri::command]
pub async fn get_session_info(state: State<'_, AppState>) -> Result<Vec<SessionInfo>, String> {
    Ok(state.browser_pool.get_all_session_info().await)
}

/// Get global statistics
#[tauri::command]
pub async fn get_global_stats(state: State<'_, AppState>) -> Result<GlobalStatsSnapshot, String> {
    Ok(state.global_stats.snapshot())
}

/// Get bot status
#[tauri::command]
pub async fn get_bot_status(state: State<'_, AppState>) -> Result<bot::BotStatus, String> {
    Ok(bot::get_bot_status_logic(&state).await)
}

/// Open a manual test browser with proxy
#[tauri::command]
pub async fn open_test_browser(state: State<'_, AppState>) -> Result<String, String> {
    info!("Opening manual test browser with proxy");

    if !state.proxy_manager.is_configured() {
        return Err("Proxy not configured. Please set up proxy credentials first.".into());
    }

    let session_ids = state.browser_pool
        .spawn_sessions_with_options(1, Some(false))
        .await
        .map_err(|e| format!("Failed to open browser: {}", e))?;

    let session_id = session_ids.first()
        .ok_or("No session created")?
        .clone();

    info!("Manual test browser opened: {}", session_id);

    if let Some(session) = state.browser_pool.get_session(&session_id).await {
        if let Err(e) = session.navigate("https://api.ipify.org/").await {
            tracing::warn!("Could not navigate to IP check: {}", e);
        }
    }

    Ok(session_id)
}

/// Close a specific browser session (for test browser)
#[tauri::command]
pub async fn close_test_browser(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<(), String> {
    info!("Closing test browser: {}", session_id);
    state.browser_pool.close_session(&session_id).await.map_err(|e| e.to_string())
}

/// Close a specific session by ID
#[tauri::command]
pub async fn close_session(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<(), String> {
    info!("User closing session: {}", session_id);
    state.browser_pool
        .close_session(&session_id)
        .await
        .map_err(|e| e.to_string())
}

/// Start the bot
#[tauri::command]
pub async fn start_bot(state: State<'_, AppState>) -> Result<(), String> {
    bot::start_bot_logic(&state).await
}

/// Stop the bot
#[tauri::command]
pub async fn stop_bot(state: State<'_, AppState>) -> Result<(), String> {
    bot::stop_bot_logic(&state).await
}

/// Detect IPs for all sessions
#[tauri::command]
pub async fn detect_ips(
    state: State<'_, AppState>,
) -> Result<std::collections::HashMap<String, Result<String, String>>, String> {
    info!("Detecting IPs for all sessions");
    Ok(state.browser_pool.detect_all_ips().await)
}

/// Set schedule configuration
#[tauri::command]
pub async fn set_schedule(
    state: State<'_, AppState>,
    config: ScheduleConfig,
) -> Result<(), String> {
    info!("Setting schedule configuration");
    state.scheduler.set_config(config).await;
    Ok(())
}

/// Get schedule status
#[tauri::command]
pub async fn get_schedule_status(state: State<'_, AppState>) -> Result<ScheduleStatus, String> {
    Ok(state.scheduler.status().await)
}

/// Test proxy connectivity
#[tauri::command]
pub async fn test_proxy(state: State<'_, AppState>) -> Result<bot::ProxyTestResult, String> {
    Ok(bot::test_proxy_logic(&state).await)
}

/// Check if proxy is verified
#[tauri::command]
pub async fn is_proxy_verified(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(state.proxy_manager.is_verified())
}

/// Get the log directory path
#[tauri::command]
pub async fn get_log_dir() -> Result<String, String> {
    crate::log_dir()
        .map(|p| p.to_string_lossy().to_string())
        .ok_or_else(|| "Could not determine log directory".to_string())
}

// ========== CAPTCHA Commands ==========

/// Get 2Captcha account balance
#[tauri::command]
pub async fn get_captcha_balance(state: State<'_, AppState>) -> Result<bot::CaptchaBalanceResult, String> {
    bot::get_captcha_balance_logic(&state).await
}

/// Test CAPTCHA solving
#[tauri::command]
pub async fn test_captcha(state: State<'_, AppState>) -> Result<bot::CaptchaTestResult, String> {
    bot::test_captcha_logic(&state).await
}

// ========== Auth Commands ==========

/// Register a new GrintaHub account
#[tauri::command]
pub async fn register_account(
    state: State<'_, AppState>,
    name: Option<String>,
    email: Option<String>,
    phone: Option<String>,
    password: String,
) -> Result<bot::AccountInfo, String> {
    bot::register_account_logic(&state, name, email, phone, password).await
}

/// Login to a GrintaHub account
#[tauri::command]
pub async fn login_account(
    state: State<'_, AppState>,
    email: String,
    password: String,
) -> Result<bot::AccountInfo, String> {
    bot::login_account_logic(&state, email, password).await
}

/// Batch register multiple accounts
#[tauri::command]
pub async fn batch_register_accounts(
    state: State<'_, AppState>,
    count: usize,
    password: String,
) -> Result<Vec<bot::AccountInfo>, String> {
    bot::batch_register_logic(&state, count, password).await
}

/// Get saved accounts
#[tauri::command]
pub async fn get_saved_accounts(state: State<'_, AppState>) -> Result<Vec<bot::AccountInfo>, String> {
    let config = state.config.read().await;
    let accounts = config.accounts.iter().map(|a| bot::AccountInfo {
        email: a.email.clone(),
        name: a.name.clone(),
        phone: a.phone.clone(),
    }).collect();
    Ok(accounts)
}

/// Delete a saved account
#[tauri::command]
pub async fn delete_account(state: State<'_, AppState>, email: String) -> Result<(), String> {
    let mut config = state.config.write().await;
    config.accounts.retain(|a| a.email != email);
    config.save();
    info!("Account deleted: {}", email);
    Ok(())
}

/// Get Oxylabs traffic usage
#[tauri::command]
pub async fn get_oxylabs_usage(state: State<'_, AppState>) -> Result<bot::OxylabsUsage, String> {
    Ok(bot::get_oxylabs_usage_logic(&state).await)
}
