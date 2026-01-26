//! HTTP route handlers for the web server.
//!
//! Maps all Tauri commands to REST API endpoints.
//! All business logic is delegated to `crate::bot`.

use std::sync::Arc;
use axum::{
    extract::{Extension, Json},
    http::StatusCode,
    middleware,
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use tracing::info;

use crate::AppState;
use crate::AppConfig;
use crate::bot;
use crate::scheduler::ScheduleConfig;

/// JSON error response helper
fn err_response(status: StatusCode, msg: &str) -> impl IntoResponse {
    (status, Json(serde_json::json!({ "error": msg })))
}

/// Build the API router with all endpoints.
pub fn api_router(state: Arc<AppState>) -> Router {
    Router::new()
        // Config
        .route("/config", get(get_config).post(configure))
        // Bot control
        .route("/bot/start", post(start_bot))
        .route("/bot/stop", post(stop_bot))
        .route("/bot/status", get(get_bot_status))
        // Stats & Sessions
        .route("/stats", get(get_global_stats))
        .route("/sessions", get(get_session_info))
        .route("/sessions/close", post(close_session))
        // Proxy
        .route("/proxy/test", post(test_proxy))
        .route("/proxy/verified", get(is_proxy_verified))
        .route("/proxy/usage", get(get_oxylabs_usage))
        // Schedule
        .route("/schedule", post(set_schedule))
        .route("/schedule/status", get(get_schedule_status))
        // IPs & Logs
        .route("/ips/detect", post(detect_ips))
        .route("/logs/dir", get(get_log_dir))
        // CAPTCHA
        .route("/captcha/balance", get(get_captcha_balance))
        .route("/captcha/test", post(test_captcha))
        // Accounts
        .route("/accounts", get(get_saved_accounts).delete(delete_account))
        .route("/accounts/register", post(register_account))
        .route("/accounts/batch", post(batch_register_accounts))
        .route("/accounts/login", post(login_account))
        // Test browser (less useful in server mode, but available)
        .route("/browser/open", post(open_test_browser))
        .route("/browser/close", post(close_test_browser))
        // Auth middleware (only if GRINTAHUB_WEB_PASS is set)
        .layer(middleware::from_fn(super::auth::basic_auth_middleware))
        .layer(Extension(state))
}

// ========== Config Handlers ==========

async fn get_config(
    Extension(state): Extension<Arc<AppState>>,
) -> impl IntoResponse {
    let config = state.config.read().await.clone();
    Json(config)
}

async fn configure(
    Extension(state): Extension<Arc<AppState>>,
    Json(config): Json<AppConfig>,
) -> impl IntoResponse {
    info!("Configuring application via web API");
    state.configure(config).await;
    StatusCode::OK
}

// ========== Bot Control Handlers ==========

async fn start_bot(
    Extension(state): Extension<Arc<AppState>>,
) -> impl IntoResponse {
    match bot::start_bot_logic(&state).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => err_response(StatusCode::BAD_REQUEST, &e).into_response(),
    }
}

async fn stop_bot(
    Extension(state): Extension<Arc<AppState>>,
) -> impl IntoResponse {
    match bot::stop_bot_logic(&state).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => err_response(StatusCode::INTERNAL_SERVER_ERROR, &e).into_response(),
    }
}

async fn get_bot_status(
    Extension(state): Extension<Arc<AppState>>,
) -> impl IntoResponse {
    Json(bot::get_bot_status_logic(&state).await)
}

// ========== Stats & Sessions Handlers ==========

async fn get_global_stats(
    Extension(state): Extension<Arc<AppState>>,
) -> impl IntoResponse {
    Json(state.global_stats.snapshot())
}

async fn get_session_info(
    Extension(state): Extension<Arc<AppState>>,
) -> impl IntoResponse {
    Json(state.browser_pool.get_all_session_info().await)
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct CloseSessionRequest {
    session_id: String,
}

async fn close_session(
    Extension(state): Extension<Arc<AppState>>,
    Json(req): Json<CloseSessionRequest>,
) -> impl IntoResponse {
    info!("Closing session via web API: {}", req.session_id);
    match state.browser_pool.close_session(&req.session_id).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => err_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()).into_response(),
    }
}

// ========== Proxy Handlers ==========

async fn test_proxy(
    Extension(state): Extension<Arc<AppState>>,
) -> impl IntoResponse {
    Json(bot::test_proxy_logic(&state).await)
}

async fn is_proxy_verified(
    Extension(state): Extension<Arc<AppState>>,
) -> impl IntoResponse {
    Json(state.proxy_manager.is_verified())
}

async fn get_oxylabs_usage(
    Extension(state): Extension<Arc<AppState>>,
) -> impl IntoResponse {
    Json(bot::get_oxylabs_usage_logic(&state).await)
}

// ========== Schedule Handlers ==========

async fn set_schedule(
    Extension(state): Extension<Arc<AppState>>,
    Json(config): Json<ScheduleConfig>,
) -> impl IntoResponse {
    info!("Setting schedule via web API");
    state.scheduler.set_config(config).await;
    StatusCode::OK
}

async fn get_schedule_status(
    Extension(state): Extension<Arc<AppState>>,
) -> impl IntoResponse {
    Json(state.scheduler.status().await)
}

// ========== IP & Logs Handlers ==========

async fn detect_ips(
    Extension(state): Extension<Arc<AppState>>,
) -> impl IntoResponse {
    info!("Detecting IPs via web API");
    Json(state.browser_pool.detect_all_ips().await)
}

async fn get_log_dir() -> impl IntoResponse {
    match crate::log_dir() {
        Some(p) => Json(serde_json::json!({ "path": p.to_string_lossy() })).into_response(),
        None => err_response(StatusCode::INTERNAL_SERVER_ERROR, "Could not determine log directory").into_response(),
    }
}

// ========== CAPTCHA Handlers ==========

async fn get_captcha_balance(
    Extension(state): Extension<Arc<AppState>>,
) -> impl IntoResponse {
    match bot::get_captcha_balance_logic(&state).await {
        Ok(result) => Json(result).into_response(),
        Err(e) => err_response(StatusCode::INTERNAL_SERVER_ERROR, &e).into_response(),
    }
}

async fn test_captcha(
    Extension(state): Extension<Arc<AppState>>,
) -> impl IntoResponse {
    match bot::test_captcha_logic(&state).await {
        Ok(result) => Json(result).into_response(),
        Err(e) => err_response(StatusCode::BAD_REQUEST, &e).into_response(),
    }
}

// ========== Account Handlers ==========

#[derive(serde::Deserialize)]
struct RegisterRequest {
    name: Option<String>,
    email: Option<String>,
    phone: Option<String>,
    password: String,
}

async fn register_account(
    Extension(state): Extension<Arc<AppState>>,
    Json(req): Json<RegisterRequest>,
) -> impl IntoResponse {
    match bot::register_account_logic(&state, req.name, req.email, req.phone, req.password).await {
        Ok(account) => Json(account).into_response(),
        Err(e) => err_response(StatusCode::BAD_REQUEST, &e).into_response(),
    }
}

#[derive(serde::Deserialize)]
struct LoginRequest {
    email: String,
    password: String,
}

async fn login_account(
    Extension(state): Extension<Arc<AppState>>,
    Json(req): Json<LoginRequest>,
) -> impl IntoResponse {
    match bot::login_account_logic(&state, req.email, req.password).await {
        Ok(account) => Json(account).into_response(),
        Err(e) => err_response(StatusCode::BAD_REQUEST, &e).into_response(),
    }
}

#[derive(serde::Deserialize)]
struct BatchRegisterRequest {
    count: usize,
    password: String,
}

async fn batch_register_accounts(
    Extension(state): Extension<Arc<AppState>>,
    Json(req): Json<BatchRegisterRequest>,
) -> impl IntoResponse {
    match bot::batch_register_logic(&state, req.count, req.password).await {
        Ok(accounts) => Json(accounts).into_response(),
        Err(e) => err_response(StatusCode::BAD_REQUEST, &e).into_response(),
    }
}

async fn get_saved_accounts(
    Extension(state): Extension<Arc<AppState>>,
) -> impl IntoResponse {
    let config = state.config.read().await;
    let accounts: Vec<bot::AccountInfo> = config.accounts.iter().map(|a| bot::AccountInfo {
        email: a.email.clone(),
        name: a.name.clone(),
        phone: a.phone.clone(),
    }).collect();
    Json(accounts)
}

#[derive(serde::Deserialize)]
struct DeleteAccountRequest {
    email: String,
}

async fn delete_account(
    Extension(state): Extension<Arc<AppState>>,
    Json(req): Json<DeleteAccountRequest>,
) -> impl IntoResponse {
    let mut config = state.config.write().await;
    config.accounts.retain(|a| a.email != req.email);
    config.save();
    info!("Account deleted via web API: {}", req.email);
    StatusCode::OK
}

// ========== Test Browser Handlers ==========

async fn open_test_browser(
    Extension(state): Extension<Arc<AppState>>,
) -> impl IntoResponse {
    info!("Opening test browser via web API");

    if !state.proxy_manager.is_configured() {
        return err_response(StatusCode::BAD_REQUEST, "Proxy not configured").into_response();
    }

    // Always headless in server mode
    match state.browser_pool.spawn_sessions_with_options(1, Some(true)).await {
        Ok(ids) => {
            if let Some(session_id) = ids.first() {
                if let Some(session) = state.browser_pool.get_session(session_id).await {
                    let _ = session.navigate("https://api.ipify.org/").await;
                }
                Json(serde_json::json!({ "sessionId": session_id })).into_response()
            } else {
                err_response(StatusCode::INTERNAL_SERVER_ERROR, "No session created").into_response()
            }
        }
        Err(e) => err_response(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed: {}", e)).into_response(),
    }
}

async fn close_test_browser(
    Extension(state): Extension<Arc<AppState>>,
    Json(req): Json<CloseSessionRequest>,
) -> impl IntoResponse {
    info!("Closing test browser via web API: {}", req.session_id);
    match state.browser_pool.close_session(&req.session_id).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => err_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()).into_response(),
    }
}
