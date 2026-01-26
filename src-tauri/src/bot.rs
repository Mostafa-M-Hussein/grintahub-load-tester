//! Core bot logic shared between desktop (Tauri) and server (axum) modes.
//!
//! Contains all business logic functions and shared response types.
//! Both `commands.rs` (Tauri) and `web/routes.rs` (axum) call into this module.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{info, warn, error};

use crate::AppState;
use crate::browser::{BrowserPool, BrowserActions, BrowserError, GoogleAccount};
use crate::stats::GlobalStats;
use crate::rate::RateLimiterConfig;

// ========== Shared Response Types ==========

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BotStatus {
    pub is_running: bool,
    pub active_sessions: usize,
    pub total_clicks: u64,
    pub total_errors: u64,
    pub clicks_per_hour: f64,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyTestResult {
    pub working: bool,
    pub original_ip: String,
    pub proxy_ip: Option<String>,
    pub error: Option<String>,
    pub test_time_ms: u64,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptchaBalanceResult {
    pub balance: f64,
    pub configured: bool,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptchaTestResult {
    pub success: bool,
    pub solve_time_ms: u64,
    pub total_time_ms: u64,
    pub token_preview: String,
    pub error: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountInfo {
    pub email: String,
    pub name: String,
    pub phone: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OxylabsUsage {
    pub traffic_used_gb: f64,
    pub traffic_limit_gb: Option<f64>,
    pub traffic_remaining_gb: Option<f64>,
    pub period_start: String,
    pub period_end: String,
    pub error: Option<String>,
}

// ========== Bot Control Logic ==========

/// Start the bot - shared logic for both Tauri and web server modes.
pub async fn start_bot_logic(state: &AppState) -> Result<(), String> {
    if state.is_running.load(Ordering::Relaxed) {
        return Err("Bot is already running".into());
    }

    let config = state.config.read().await.clone();

    if config.keywords.is_empty() {
        return Err("No keywords configured".into());
    }

    if !state.proxy_manager.is_configured() {
        warn!("Starting bot without proxy configured - using direct connection");
    } else if !state.proxy_manager.is_verified() {
        warn!("Starting bot with untested proxy - connection may fail");
    }

    info!("Starting bot with {} sessions (headless: {})", config.concurrent_sessions, config.headless);

    let session_ids = state.browser_pool
        .spawn_sessions_with_options(config.concurrent_sessions, Some(config.headless))
        .await
        .map_err(|e| e.to_string())?;

    state.is_running.store(true, Ordering::Relaxed);
    state.global_stats.reset();

    let google_account: Option<GoogleAccount> = config.accounts
        .first()
        .and_then(|acc| {
            info!("Checking Google account: email='{}', has_token={}",
                acc.email,
                acc.token.as_ref().map(|t| !t.is_empty()).unwrap_or(false));

            if !acc.email.is_empty() && acc.token.as_ref().map(|t| !t.is_empty()).unwrap_or(false) {
                Some(GoogleAccount {
                    email: acc.email.clone(),
                    password: acc.token.clone().unwrap_or_default(),
                })
            } else {
                info!("Google account not configured: email empty or no password");
                None
            }
        });

    if let Some(ref account) = google_account {
        info!("Google account configured for sessions: {}", account.email);
    } else {
        info!("No Google account configured (accounts count: {})", config.accounts.len());
    }

    let browser_pool = state.browser_pool.clone();
    let global_stats = state.global_stats.clone();
    let rate_config = state.rate_config.clone();
    let is_running = state.is_running.clone();
    let keywords = config.keywords.clone();
    let max_clicks = config.max_clicks_per_session;
    let auto_rotate_ip = config.auto_rotate_ip;
    let captcha_api_key = config.captcha_api_key.clone();

    info!("Bot loop started with {} sessions (auto_rotate_ip: {})", session_ids.len(), auto_rotate_ip);

    for session_id in session_ids {
        spawn_session_task_safe(
            session_id,
            browser_pool.clone(),
            global_stats.clone(),
            rate_config.clone(),
            is_running.clone(),
            keywords.clone(),
            max_clicks,
            google_account.clone(),
            auto_rotate_ip,
            captcha_api_key.clone(),
        );
    }

    let supervisor_handle = crate::supervisor::SessionSupervisor::start(
        state.is_running.clone(),
        state.browser_pool.clone(),
        state.global_stats.clone(),
        state.rate_config.clone(),
        state.config.clone(),
        crate::supervisor::SupervisorConfig::default(),
    );

    {
        let mut handle = state.supervisor_handle.lock().await;
        *handle = Some(supervisor_handle);
    }

    info!("Session supervisor started");
    Ok(())
}

/// Stop the bot - shared logic for both Tauri and web server modes.
pub async fn stop_bot_logic(state: &AppState) -> Result<(), String> {
    info!("Stopping bot");
    state.is_running.store(false, Ordering::Relaxed);

    {
        let mut handle = state.supervisor_handle.lock().await;
        if let Some(h) = handle.take() {
            h.abort();
            info!("Supervisor stopped");
        }
    }

    state.browser_pool.close_all().await.map_err(|e| e.to_string())?;

    let killed = crate::supervisor::cleanup_zombie_chromes(&state.browser_pool).await;
    if killed > 0 {
        info!("Final zombie cleanup: killed {} orphaned Chrome processes", killed);
    }

    state.global_stats.set_active_sessions(0);
    Ok(())
}

/// Get bot status - shared logic.
pub async fn get_bot_status_logic(state: &AppState) -> BotStatus {
    let stats = state.global_stats.snapshot();
    let sessions = state.browser_pool.session_count().await;

    BotStatus {
        is_running: state.is_running.load(Ordering::Relaxed),
        active_sessions: sessions,
        total_clicks: stats.total_success,
        total_errors: stats.total_errors,
        clicks_per_hour: stats.clicks_per_hour,
    }
}

// ========== Proxy Testing Logic ==========

/// Fetch IP without proxy
pub async fn fetch_ip_without_proxy() -> Result<String, String> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| format!("Failed to create client: {}", e))?;

    let response = client
        .get("https://api.ipify.org/?format=json")
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;

    let data: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse response: {}", e))?;

    data.get("ip")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "No IP in response".to_string())
}

/// Fetch IP with proxy using explicit basic auth
pub async fn fetch_ip_with_proxy(proxy_host: &str, username: &str, password: &str) -> Result<String, String> {
    info!("Proxy test: connecting via {} (user: {}...)", proxy_host, &username[..username.len().min(30)]);

    let proxy = reqwest::Proxy::all(proxy_host)
        .map_err(|e| format!("Invalid proxy URL: {}", e))?
        .basic_auth(username, password);

    let client = reqwest::Client::builder()
        .proxy(proxy)
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| format!("Failed to create proxy client: {:?}", e))?;

    let response = client
        .get("http://api.ipify.org/?format=json")
        .send()
        .await
        .map_err(|e| format!("Proxy request failed: {:?}", e))?;

    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }

    let data: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse response: {}", e))?;

    data.get("ip")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "No IP in response".to_string())
}

/// Test proxy connectivity - shared logic.
pub async fn test_proxy_logic(state: &AppState) -> ProxyTestResult {
    info!("Testing proxy connectivity...");
    let start = std::time::Instant::now();

    let original_ip = match fetch_ip_without_proxy().await {
        Ok(ip) => {
            info!("Original IP: {}", ip);
            ip
        }
        Err(e) => {
            error!("Failed to get original IP: {}", e);
            return ProxyTestResult {
                working: false,
                original_ip: "Unknown".to_string(),
                proxy_ip: None,
                error: Some(format!("Failed to get original IP: {}", e)),
                test_time_ms: start.elapsed().as_millis() as u64,
            };
        }
    };

    let config = state.config.read().await;
    if config.proxy_customer.is_empty() || config.proxy_password.is_empty() {
        return ProxyTestResult {
            working: false,
            original_ip,
            proxy_ip: None,
            error: Some("Proxy credentials not configured".into()),
            test_time_ms: start.elapsed().as_millis() as u64,
        };
    }

    let sessid = format!("test{}", std::process::id());
    let proxy_host = "http://pr.oxylabs.io:60000";
    let proxy_username = format!(
        "customer-{}-cc-{}-sessid-{}-sesstime-10",
        config.proxy_customer, config.proxy_country, sessid,
    );
    let proxy_password = config.proxy_password.clone();
    drop(config);

    info!("Testing HTTP proxy @ pr.oxylabs.io:60000 with sessid={}", sessid);

    match fetch_ip_with_proxy(proxy_host, &proxy_username, &proxy_password).await {
        Ok(proxy_ip) => {
            let test_time_ms = start.elapsed().as_millis() as u64;
            let working = original_ip != proxy_ip;

            if working {
                info!("Proxy test SUCCESS: Original={}, Proxy={}, Time={}ms", original_ip, proxy_ip, test_time_ms);
                state.proxy_manager.set_verified(true);
                {
                    let mut cfg = state.config.write().await;
                    cfg.proxy_verified = true;
                    cfg.save();
                }
            } else {
                warn!("Proxy test FAILED: IPs are the same ({}), proxy not routing correctly", original_ip);
            }

            ProxyTestResult {
                working,
                original_ip,
                proxy_ip: Some(proxy_ip),
                error: if !working { Some("Proxy not routing - IPs are the same".into()) } else { None },
                test_time_ms,
            }
        }
        Err(e) => {
            let test_time_ms = start.elapsed().as_millis() as u64;
            error!("Proxy connection failed: {}", e);
            ProxyTestResult {
                working: false,
                original_ip,
                proxy_ip: None,
                error: Some(format!("Proxy connection failed: {}", e)),
                test_time_ms,
            }
        }
    }
}

// ========== CAPTCHA Logic ==========

pub async fn get_captcha_balance_logic(state: &AppState) -> Result<CaptchaBalanceResult, String> {
    let config = state.config.read().await;

    if config.captcha_api_key.is_empty() {
        return Ok(CaptchaBalanceResult {
            balance: 0.0,
            configured: false,
        });
    }

    let solver = crate::captcha::CaptchaSolver::new(&config.captcha_api_key)
        .map_err(|e| e.to_string())?;

    let balance = solver.get_balance().await.map_err(|e| e.to_string())?;

    Ok(CaptchaBalanceResult {
        balance,
        configured: true,
    })
}

pub async fn test_captcha_logic(state: &AppState) -> Result<CaptchaTestResult, String> {
    info!("Testing CAPTCHA solving...");
    let start = std::time::Instant::now();

    let config = state.config.read().await;

    if config.captcha_api_key.is_empty() {
        return Err("2Captcha API key not configured".into());
    }

    let solver = crate::captcha::CaptchaSolver::new(&config.captcha_api_key)
        .map_err(|e| e.to_string())?;

    let request = crate::captcha::CaptchaRequest::grintahub_login();

    match solver.solve(&request).await {
        Ok(result) => {
            let test_time_ms = start.elapsed().as_millis() as u64;
            info!("CAPTCHA test SUCCESS: solved in {}ms", result.solve_time_ms);

            Ok(CaptchaTestResult {
                success: true,
                solve_time_ms: result.solve_time_ms,
                total_time_ms: test_time_ms,
                token_preview: result.token[..result.token.len().min(50)].to_string(),
                error: None,
            })
        }
        Err(e) => {
            let test_time_ms = start.elapsed().as_millis() as u64;
            error!("CAPTCHA test FAILED: {}", e);

            Ok(CaptchaTestResult {
                success: false,
                solve_time_ms: 0,
                total_time_ms: test_time_ms,
                token_preview: String::new(),
                error: Some(e.to_string()),
            })
        }
    }
}

// ========== Auth Logic ==========

pub async fn register_account_logic(
    state: &AppState,
    name: Option<String>,
    email: Option<String>,
    phone: Option<String>,
    password: String,
) -> Result<AccountInfo, String> {
    info!("Registering new account...");

    let config = state.config.read().await;

    if config.captcha_api_key.is_empty() {
        return Err("2Captcha API key not configured".into());
    }

    let solver = crate::captcha::CaptchaSolver::new(&config.captcha_api_key)
        .map_err(|e| e.to_string())?;

    let auth_client = crate::auth::AuthClient::new(60)
        .map_err(|e| e.to_string())?;

    let name = name.unwrap_or_else(crate::auth::FakeData::random_name);
    let email = email.unwrap_or_else(crate::auth::FakeData::random_email);
    let phone = phone.unwrap_or_else(crate::auth::FakeData::random_saudi_phone);

    drop(config);

    let account = auth_client
        .register_with_captcha(&solver, &name, &email, &phone, &password)
        .await
        .map_err(|e| e.to_string())?;

    {
        let mut config = state.config.write().await;
        config.accounts.push(account.clone());
        config.save();
    }

    info!("Account registered: {}", account.email);

    Ok(AccountInfo {
        email: account.email,
        name: account.name,
        phone: account.phone,
    })
}

pub async fn login_account_logic(
    state: &AppState,
    email: String,
    password: String,
) -> Result<AccountInfo, String> {
    info!("Logging in: {}", email);

    let config = state.config.read().await;

    if config.captcha_api_key.is_empty() {
        return Err("2Captcha API key not configured".into());
    }

    let solver = crate::captcha::CaptchaSolver::new(&config.captcha_api_key)
        .map_err(|e| e.to_string())?;

    let auth_client = crate::auth::AuthClient::new(60)
        .map_err(|e| e.to_string())?;

    drop(config);

    let account = auth_client
        .login_with_captcha(&solver, &email, &password)
        .await
        .map_err(|e| e.to_string())?;

    info!("Login successful: {}", account.email);

    Ok(AccountInfo {
        email: account.email,
        name: account.name,
        phone: account.phone,
    })
}

pub async fn batch_register_logic(
    state: &AppState,
    count: usize,
    password: String,
) -> Result<Vec<AccountInfo>, String> {
    info!("Batch registering {} accounts...", count);

    let config = state.config.read().await;

    if config.captcha_api_key.is_empty() {
        return Err("2Captcha API key not configured".into());
    }

    let solver = crate::captcha::CaptchaSolver::new(&config.captcha_api_key)
        .map_err(|e| e.to_string())?;

    let auth_client = crate::auth::AuthClient::new(60)
        .map_err(|e| e.to_string())?;

    drop(config);

    let results = auth_client
        .batch_register(&solver, count, &password, Some(2000))
        .await;

    let mut accounts = Vec::new();
    let mut config = state.config.write().await;

    for result in results {
        match result {
            Ok(account) => {
                config.accounts.push(account.clone());
                accounts.push(AccountInfo {
                    email: account.email,
                    name: account.name,
                    phone: account.phone,
                });
            }
            Err(e) => {
                warn!("Failed to register account: {}", e);
            }
        }
    }

    config.save();
    info!("Batch registration complete: {}/{} accounts created", accounts.len(), count);

    Ok(accounts)
}

// ========== Oxylabs Usage Logic ==========

pub async fn get_oxylabs_usage_logic(state: &AppState) -> OxylabsUsage {
    let config = state.config.read().await;

    if config.proxy_customer.is_empty() || config.proxy_password.is_empty() {
        return OxylabsUsage {
            traffic_used_gb: 0.0,
            traffic_limit_gb: None,
            traffic_remaining_gb: None,
            period_start: String::new(),
            period_end: String::new(),
            error: Some("Proxy credentials not configured".into()),
        };
    }

    let customer = config.proxy_customer.clone();
    let password = config.proxy_password.clone();
    drop(config);

    info!("Fetching Oxylabs usage for customer: {}", customer);

    let client = reqwest::Client::new();

    let login_resp = match client
        .post("https://residential-api.oxylabs.io/v1/login")
        .basic_auth(&customer, Some(&password))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return OxylabsUsage {
                traffic_used_gb: 0.0,
                traffic_limit_gb: None,
                traffic_remaining_gb: None,
                period_start: String::new(),
                period_end: String::new(),
                error: Some(format!("Login request failed: {}", e)),
            };
        }
    };

    if !login_resp.status().is_success() {
        let status = login_resp.status();
        return OxylabsUsage {
            traffic_used_gb: 0.0,
            traffic_limit_gb: None,
            traffic_remaining_gb: None,
            period_start: String::new(),
            period_end: String::new(),
            error: Some(format!("Login failed: HTTP {}", status)),
        };
    }

    let login_data: serde_json::Value = match login_resp.json().await {
        Ok(d) => d,
        Err(e) => {
            return OxylabsUsage {
                traffic_used_gb: 0.0,
                traffic_limit_gb: None,
                traffic_remaining_gb: None,
                period_start: String::new(),
                period_end: String::new(),
                error: Some(format!("Failed to parse login response: {}", e)),
            };
        }
    };

    let token = match login_data.get("token").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => {
            return OxylabsUsage {
                traffic_used_gb: 0.0,
                traffic_limit_gb: None,
                traffic_remaining_gb: None,
                period_start: String::new(),
                period_end: String::new(),
                error: Some("No token in login response".into()),
            };
        }
    };

    let user_id = match login_data.get("user_id").and_then(|v| v.as_str()) {
        Some(u) => u,
        None => {
            return OxylabsUsage {
                traffic_used_gb: 0.0,
                traffic_limit_gb: None,
                traffic_remaining_gb: None,
                period_start: String::new(),
                period_end: String::new(),
                error: Some("No user_id in login response".into()),
            };
        }
    };

    let stats_url = format!("https://residential-api.oxylabs.io/v1/users/{}/client-stats", user_id);

    let stats_resp = match client
        .get(&stats_url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return OxylabsUsage {
                traffic_used_gb: 0.0,
                traffic_limit_gb: None,
                traffic_remaining_gb: None,
                period_start: String::new(),
                period_end: String::new(),
                error: Some(format!("Stats request failed: {}", e)),
            };
        }
    };

    if !stats_resp.status().is_success() {
        let status = stats_resp.status();
        return OxylabsUsage {
            traffic_used_gb: 0.0,
            traffic_limit_gb: None,
            traffic_remaining_gb: None,
            period_start: String::new(),
            period_end: String::new(),
            error: Some(format!("Stats request failed: HTTP {}", status)),
        };
    }

    let stats_data: serde_json::Value = match stats_resp.json().await {
        Ok(d) => d,
        Err(e) => {
            return OxylabsUsage {
                traffic_used_gb: 0.0,
                traffic_limit_gb: None,
                traffic_remaining_gb: None,
                period_start: String::new(),
                period_end: String::new(),
                error: Some(format!("Failed to parse stats response: {}", e)),
            };
        }
    };

    let traffic_used_gb = stats_data.get("traffic")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);

    let traffic_limit_gb = stats_data.get("traffic_limit")
        .and_then(|v| v.as_f64());

    let traffic_remaining_gb = traffic_limit_gb.map(|limit| (limit - traffic_used_gb).max(0.0));

    let period_start = stats_data.get("date_from")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let period_end = stats_data.get("date_to")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    info!("Oxylabs usage: {:.2} GB used, limit: {:?} GB", traffic_used_gb, traffic_limit_gb);

    OxylabsUsage {
        traffic_used_gb,
        traffic_limit_gb,
        traffic_remaining_gb,
        period_start,
        period_end,
        error: None,
    }
}

// ========== Session Task Management ==========

/// Spawn a session task with panic safety.
///
/// Wraps `run_session_loop` so that if it panics:
/// 1. The active_sessions counter is decremented
/// 2. The browser session is cleaned up from the pool
/// 3. The panic is logged (not propagated)
///
/// The supervisor will detect the missing session and respawn it.
pub fn spawn_session_task_safe(
    session_id: String,
    pool: Arc<BrowserPool>,
    stats: Arc<GlobalStats>,
    rate_config: Arc<RwLock<RateLimiterConfig>>,
    is_running: Arc<AtomicBool>,
    keywords: Vec<String>,
    max_clicks: u32,
    google_account: Option<GoogleAccount>,
    auto_rotate_ip: bool,
    captcha_api_key: String,
) -> tokio::task::JoinHandle<()> {
    let pool_cleanup = pool.clone();
    let stats_cleanup = stats.clone();
    let sid_cleanup = session_id.clone();

    tokio::spawn(async move {
        let result = std::panic::AssertUnwindSafe(
            run_session_loop(
                session_id, pool, stats, rate_config,
                is_running, keywords, max_clicks, google_account,
                auto_rotate_ip, captcha_api_key,
            )
        );

        use futures::FutureExt;
        match result.catch_unwind().await {
            Ok(()) => {
                // Normal exit
            }
            Err(panic_info) => {
                let panic_msg = if let Some(s) = panic_info.downcast_ref::<&str>() {
                    s.to_string()
                } else if let Some(s) = panic_info.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "Unknown panic".to_string()
                };

                error!(
                    "[PanicSafety] Session {} panicked: {}. Cleaning up.",
                    sid_cleanup, panic_msg
                );

                stats_cleanup.remove_session();

                if let Err(e) = pool_cleanup.close_session(&sid_cleanup).await {
                    warn!("[PanicSafety] Failed to close panicked session {}: {}", sid_cleanup, e);
                }
            }
        }
    })
}

/// Verify a session has a unique IP (not used by any other session).
async fn ensure_unique_ip(
    session_id: String,
    session: Arc<crate::browser::BrowserSession>,
    pool: &Arc<BrowserPool>,
    max_retries: u32,
) -> Option<(String, Arc<crate::browser::BrowserSession>)> {
    let mut current_id = session_id;
    let mut current_session = session;

    for attempt in 0..=max_retries {
        match current_session.detect_ip().await {
            Ok(ip) => {
                if pool.register_ip(&ip).await {
                    info!("Session {} verified unique IP: {}", current_id, ip);
                    return Some((current_id, current_session));
                }

                warn!("Session {} got duplicate IP {} (attempt {}/{}), retrying with new session",
                    current_id, ip, attempt + 1, max_retries + 1);

                let _ = pool.close_session(&current_id).await;
                tokio::time::sleep(Duration::from_millis(300)).await;

                match pool.spawn_sessions(1).await {
                    Ok(new_ids) if !new_ids.is_empty() => {
                        let new_id = new_ids.into_iter().next().unwrap();
                        match pool.get_session(&new_id).await {
                            Some(s) => {
                                current_id = new_id;
                                current_session = s;
                                continue;
                            }
                            None => {
                                error!("New session {} not found after spawn", new_id);
                                return None;
                            }
                        }
                    }
                    _ => {
                        error!("Failed to spawn replacement session for unique IP");
                        return None;
                    }
                }
            }
            Err(e) => {
                warn!("Session {} IP detection failed: {} - continuing with random sessid guarantee", current_id, e);
                return Some((current_id, current_session));
            }
        }
    }

    error!("Failed to get unique IP after {} retries", max_retries + 1);
    None
}

/// Run the bot loop for a single session
async fn run_session_loop(
    mut session_id: String,
    pool: Arc<BrowserPool>,
    stats: Arc<GlobalStats>,
    rate_config: Arc<RwLock<RateLimiterConfig>>,
    is_running: Arc<AtomicBool>,
    keywords: Vec<String>,
    max_clicks: u32,
    google_account: Option<GoogleAccount>,
    auto_rotate_ip: bool,
    captcha_api_key: String,
) {
    info!("Session {} bot loop starting (max_clicks: {}, google_login: {}, auto_rotate_ip: {})",
        session_id,
        if max_clicks == 0 { "unlimited".to_string() } else { max_clicks.to_string() },
        google_account.is_some(),
        auto_rotate_ip
    );

    stats.add_session();

    let mut session = match pool.get_session(&session_id).await {
        Some(s) => s,
        None => {
            warn!("Session {} not found", session_id);
            stats.remove_session();
            return;
        }
    };

    match ensure_unique_ip(session_id.clone(), session, &pool, 3).await {
        Some((verified_id, verified_session)) => {
            session_id = verified_id;
            session = verified_session;
        }
        None => {
            error!("Session {} could not get a unique IP, stopping", session_id);
            stats.remove_session();
            return;
        }
    }

    let config = rate_config.read().await.clone();
    let mut rate_limiter = crate::rate::RateLimiter::new(config);

    let mut keyword_index = 0;
    let mut session_clicks: u32 = 0;
    let mut consecutive_errors: u32 = 0;
    let mut already_logged_in = false;
    let mut ip_rotation_count: u32 = 0;
    let keyword_count = keywords.len();

    while is_running.load(Ordering::Relaxed) {
        if max_clicks > 0 && session_clicks >= max_clicks {
            info!("Session {} reached max clicks limit ({}), closing", session_id, max_clicks);
            break;
        }

        rate_limiter.wait().await;
        session.increment_cycles();

        let keyword = &keywords[keyword_index % keywords.len()];
        keyword_index += 1;

        let cycle_start = std::time::Instant::now();
        match BrowserActions::run_cycle_with_login(
            &session,
            keyword,
            rate_limiter.config().min_delay_ms,
            rate_limiter.config().max_delay_ms,
            google_account.as_ref(),
            &mut already_logged_in,
        ).await {
            Ok(clicked) => {
                let latency = cycle_start.elapsed().as_millis() as u64;
                if clicked {
                    stats.record_click(latency);
                    rate_limiter.record_success();
                    session_clicks += 1;
                    consecutive_errors = 0;
                }

                if auto_rotate_ip && keyword_count > 0 && keyword_index % keyword_count == 0 && keyword_index > 0 {
                    info!("Session {} completed {} keywords, rotating IP (#{}/{})",
                        session_id, keyword_count, ip_rotation_count + 1, session.ip_change_count() + 1);

                    let _ = pool.close_session(&session_id).await;
                    tokio::time::sleep(Duration::from_millis(1000)).await;

                    loop {
                        if !is_running.load(Ordering::Relaxed) { break; }
                        match pool.spawn_sessions(1).await {
                            Ok(new_ids) if !new_ids.is_empty() => {
                                let new_id = new_ids.into_iter().next().unwrap();
                                ip_rotation_count += 1;
                                info!("Session {} -> {} (auto-rotate #{})", session_id, new_id, ip_rotation_count);
                                session_id = new_id;

                                match pool.get_session(&session_id).await {
                                    Some(s) => {
                                        session = s;
                                        already_logged_in = false;
                                        tokio::time::sleep(Duration::from_millis(1000)).await;
                                        break;
                                    }
                                    None => {
                                        warn!("New session {} not found - retrying", session_id);
                                        tokio::time::sleep(Duration::from_millis(2000)).await;
                                    }
                                }
                            }
                            _ => {
                                warn!("Failed to spawn replacement - retrying in 3s");
                                tokio::time::sleep(Duration::from_millis(3000)).await;
                            }
                        }
                    }
                }
            }
            Err(BrowserError::CaptchaDetected(msg)) => {
                warn!("Session {} CAPTCHA detected: {}", session_id, msg);

                let mut solved = false;
                if !captcha_api_key.is_empty() {
                    info!("Session {} attempting to solve CAPTCHA with 2Captcha...", session_id);
                    match BrowserActions::solve_google_captcha(&session, &captcha_api_key).await {
                        Ok(true) => {
                            info!("Session {} CAPTCHA solved successfully! Continuing...", session_id);
                            solved = true;
                            consecutive_errors = 0;
                        }
                        Ok(false) => {
                            warn!("Session {} CAPTCHA solve returned false", session_id);
                        }
                        Err(e) => {
                            warn!("Session {} CAPTCHA solve failed: {}", session_id, e);
                        }
                    }
                } else {
                    warn!("Session {} no 2Captcha API key configured, falling back to IP change", session_id);
                }

                if solved {
                    consecutive_errors = 0;
                    continue;
                }

                warn!("Session {} CAPTCHA not solved - changing IP", session_id);
                stats.record_error();
                session.increment_errors();
                consecutive_errors += 1;

                if consecutive_errors > 10 {
                    let backoff = std::cmp::min(consecutive_errors as u64 * 1000, 30_000);
                    warn!("Session {} backing off {}ms ({} consecutive errors)", session_id, backoff, consecutive_errors);
                    tokio::time::sleep(Duration::from_millis(backoff)).await;
                }

                let _ = pool.close_session(&session_id).await;
                tokio::time::sleep(Duration::from_millis(500)).await;

                match pool.spawn_sessions(1).await {
                    Ok(new_ids) if !new_ids.is_empty() => {
                        let new_id = new_ids.into_iter().next().unwrap();
                        info!("Session {} -> {} (IP change #{}, reason: CAPTCHA unsolved)",
                            session_id, new_id, consecutive_errors);
                        session_id = new_id;

                        match pool.get_session(&session_id).await {
                            Some(s) => {
                                session = s;
                                already_logged_in = false;
                                tokio::time::sleep(Duration::from_millis(500)).await;
                            }
                            None => {
                                warn!("New session {} not found - retrying spawn", session_id);
                                tokio::time::sleep(Duration::from_millis(2000)).await;
                                continue;
                            }
                        }
                    }
                    _ => {
                        warn!("Failed to spawn replacement - retrying in 3s");
                        tokio::time::sleep(Duration::from_millis(3000)).await;
                        continue;
                    }
                }
            }
            Err(BrowserError::ElementNotFound(msg)) if msg.contains("need new IP") => {
                warn!("Session {} no ad found: {} - FAST IP change", session_id, msg);
                ip_rotation_count += 1;

                let _ = pool.close_session(&session_id).await;
                tokio::time::sleep(Duration::from_millis(300)).await;

                loop {
                    if !is_running.load(Ordering::Relaxed) { break; }
                    match pool.spawn_sessions(1).await {
                        Ok(new_ids) if !new_ids.is_empty() => {
                            let new_id = new_ids.into_iter().next().unwrap();
                            info!("Session {} -> {} (no ad, IP change #{})",
                                session_id, new_id, ip_rotation_count);
                            session_id = new_id;

                            match pool.get_session(&session_id).await {
                                Some(s) => {
                                    session = s;
                                    already_logged_in = false;
                                    tokio::time::sleep(Duration::from_millis(500)).await;
                                    break;
                                }
                                None => {
                                    warn!("New session {} not found - retrying", session_id);
                                    tokio::time::sleep(Duration::from_millis(2000)).await;
                                }
                            }
                        }
                        _ => {
                            warn!("Failed to spawn replacement - retrying in 3s");
                            tokio::time::sleep(Duration::from_millis(3000)).await;
                        }
                    }
                }
            }
            Err(BrowserError::Timeout(msg)) |
            Err(BrowserError::NavigationFailed(msg)) |
            Err(BrowserError::ConnectionLost(msg)) => {
                warn!("Session {} network error: {} - changing IP", session_id, msg);
                stats.record_error();
                session.increment_errors();
                consecutive_errors += 1;

                if consecutive_errors > 5 {
                    let backoff = std::cmp::min(consecutive_errors as u64 * 1000, 30_000);
                    warn!("Session {} network backoff {}ms ({} consecutive)", session_id, backoff, consecutive_errors);
                    tokio::time::sleep(Duration::from_millis(backoff)).await;
                }

                let _ = pool.close_session(&session_id).await;
                tokio::time::sleep(Duration::from_millis(1000)).await;

                loop {
                    if !is_running.load(Ordering::Relaxed) { break; }
                    match pool.spawn_sessions(1).await {
                        Ok(new_ids) if !new_ids.is_empty() => {
                            let new_id = new_ids.into_iter().next().unwrap();
                            info!("Session {} -> {} (network error: {})", session_id, new_id, msg);
                            session_id = new_id;

                            match pool.get_session(&session_id).await {
                                Some(s) => {
                                    session = s;
                                    already_logged_in = false;
                                    tokio::time::sleep(Duration::from_millis(1000)).await;
                                    break;
                                }
                                None => {
                                    warn!("New session {} not found - retrying", session_id);
                                    tokio::time::sleep(Duration::from_millis(2000)).await;
                                }
                            }
                        }
                        _ => {
                            warn!("Failed to spawn replacement - retrying in 3s");
                            tokio::time::sleep(Duration::from_millis(3000)).await;
                        }
                    }
                }
            }
            Err(e) => {
                warn!("Session {} cycle error: {}", session_id, e);
                stats.record_error();
                rate_limiter.record_error();
                session.increment_errors();
                consecutive_errors += 1;

                if !session.is_alive() {
                    warn!("Session {} died - respawning", session_id);
                    let _ = pool.close_session(&session_id).await;
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    loop {
                        if !is_running.load(Ordering::Relaxed) { break; }
                        match pool.spawn_sessions(1).await {
                            Ok(new_ids) if !new_ids.is_empty() => {
                                let new_id = new_ids.into_iter().next().unwrap();
                                info!("Session {} -> {} (respawned after error)", session_id, new_id);
                                session_id = new_id;
                                match pool.get_session(&session_id).await {
                                    Some(s) => {
                                        session = s;
                                        already_logged_in = false;
                                        break;
                                    }
                                    None => {
                                        tokio::time::sleep(Duration::from_millis(2000)).await;
                                    }
                                }
                            }
                            _ => {
                                tokio::time::sleep(Duration::from_millis(3000)).await;
                            }
                        }
                    }
                }
            }
        }
    }

    if let Err(e) = pool.close_session(&session_id).await {
        warn!("Failed to close session {}: {}", session_id, e);
    }

    stats.remove_session();

    info!("Session {} bot loop ended (clicks: {}, ip_rotations: {}, errors: {})", session_id, session_clicks, ip_rotation_count, consecutive_errors);
}
