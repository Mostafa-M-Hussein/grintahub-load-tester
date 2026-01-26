//! Tauri commands
//!
//! IPC commands for frontend communication.

use std::sync::atomic::Ordering;
use std::time::Duration;
use tauri::State;
use tracing::{info, warn, error};
use reqwest::Proxy;

use crate::{AppConfig, AppState};
use crate::browser::{SessionInfo, BrowserActions, BrowserError};
use crate::stats::GlobalStatsSnapshot;
use crate::scheduler::{ScheduleConfig, ScheduleStatus};

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

    let config = state.config.read().await;

    // Spawn sessions
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

/// Bot status response
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BotStatus {
    pub is_running: bool,
    pub active_sessions: usize,
    pub total_clicks: u64,
    pub clicks_per_hour: f64,
}

/// Get bot status
#[tauri::command]
pub async fn get_bot_status(state: State<'_, AppState>) -> Result<BotStatus, String> {
    let stats = state.global_stats.snapshot();
    let sessions = state.browser_pool.session_count().await;

    Ok(BotStatus {
        is_running: state.is_running.load(Ordering::Relaxed),
        active_sessions: sessions,
        total_clicks: stats.total_clicks,
        clicks_per_hour: stats.clicks_per_hour,
    })
}

/// Open a manual test browser with proxy (stays open for user to test manually)
#[tauri::command]
pub async fn open_test_browser(state: State<'_, AppState>) -> Result<String, String> {
    info!("Opening manual test browser with proxy");

    // Check if proxy is configured
    if !state.proxy_manager.is_configured() {
        return Err("Proxy not configured. Please set up proxy credentials first.".into());
    }

    // Spawn a single browser session (non-headless for manual testing)
    let session_ids = state.browser_pool
        .spawn_sessions_with_options(1, Some(false)) // Always visible for manual test
        .await
        .map_err(|e| format!("Failed to open browser: {}", e))?;

    let session_id = session_ids.first()
        .ok_or("No session created")?
        .clone();

    info!("Manual test browser opened: {}", session_id);

    // Navigate to Google to start
    if let Some(session) = state.browser_pool.get_session(&session_id).await {
        // Navigate to IP check first so user can verify proxy
        if let Err(e) = session.navigate("https://api.ipify.org/").await {
            warn!("Could not navigate to IP check: {}", e);
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

/// Close a specific session by ID (user can close any session from UI)
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
    if state.is_running.load(Ordering::Relaxed) {
        return Err("Bot is already running".into());
    }

    let config = state.config.read().await.clone();

    if config.keywords.is_empty() {
        return Err("No keywords configured".into());
    }

    // Proxy is optional - warn if not configured but don't block
    if !state.proxy_manager.is_configured() {
        warn!("Starting bot without proxy configured - using direct connection");
    } else if !state.proxy_manager.is_verified() {
        warn!("Starting bot with untested proxy - connection may fail");
    }

    info!("Starting bot with {} sessions (headless: {})", config.concurrent_sessions, config.headless);

    // Start sessions with headless setting from config
    let session_ids = state.browser_pool
        .spawn_sessions_with_options(config.concurrent_sessions, Some(config.headless))
        .await
        .map_err(|e| e.to_string())?;

    state.is_running.store(true, Ordering::Relaxed);

    // Extract Google account from config if available
    // The frontend stores the Google password in the 'token' field of the first account
    let google_account: Option<crate::browser::GoogleAccount> = config.accounts
        .first()
        .and_then(|acc| {
            info!("Checking Google account: email='{}', has_token={}",
                acc.email,
                acc.token.as_ref().map(|t| !t.is_empty()).unwrap_or(false));

            if !acc.email.is_empty() && acc.token.as_ref().map(|t| !t.is_empty()).unwrap_or(false) {
                Some(crate::browser::GoogleAccount {
                    email: acc.email.clone(),
                    password: acc.token.clone().unwrap_or_default(), // Token holds password temporarily
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

    // Start bot loop for each session
    let browser_pool = state.browser_pool.clone();
    let global_stats = state.global_stats.clone();
    let rate_config = state.rate_config.clone();
    let is_running = state.is_running.clone();
    let keywords = config.keywords.clone();
    let max_clicks = config.max_clicks_per_session;
    let auto_rotate_ip = config.auto_rotate_ip;

    tokio::spawn(async move {
        info!("Bot loop started with {} sessions (auto_rotate_ip: {})", session_ids.len(), auto_rotate_ip);

        // Create tasks for each session
        let mut handles = Vec::new();

        for session_id in session_ids {
            let pool = browser_pool.clone();
            let stats = global_stats.clone();
            let rate_cfg = rate_config.clone();
            let google_acc = google_account.clone();
            let running = is_running.clone();
            let kws = keywords.clone();

            let handle = tokio::spawn(async move {
                run_session_loop(session_id, pool, stats, rate_cfg, running, kws, max_clicks, google_acc, auto_rotate_ip).await;
            });

            handles.push(handle);
        }

        // Wait for all sessions to complete
        for handle in handles {
            let _ = handle.await;
        }

        info!("Bot loop completed");
    });

    Ok(())
}

/// Run the bot loop for a single session
async fn run_session_loop(
    mut session_id: String,
    pool: std::sync::Arc<crate::browser::BrowserPool>,
    stats: std::sync::Arc<crate::stats::GlobalStats>,
    rate_config: std::sync::Arc<tokio::sync::RwLock<crate::rate::RateLimiterConfig>>,
    is_running: std::sync::Arc<std::sync::atomic::AtomicBool>,
    keywords: Vec<String>,
    max_clicks: u32,
    google_account: Option<crate::browser::GoogleAccount>,
    auto_rotate_ip: bool,
) {
    info!("Session {} bot loop starting (max_clicks: {}, google_login: {}, auto_rotate_ip: {})",
        session_id,
        if max_clicks == 0 { "unlimited".to_string() } else { max_clicks.to_string() },
        google_account.is_some(),
        auto_rotate_ip
    );

    let mut session = match pool.get_session(&session_id).await {
        Some(s) => s,
        None => {
            warn!("Session {} not found", session_id);
            return;
        }
    };

    // Create rate limiter for this session
    let config = rate_config.read().await.clone();
    let mut rate_limiter = crate::rate::RateLimiter::new(config);

    let mut keyword_index = 0;
    let mut session_clicks: u32 = 0;
    let mut captcha_restarts: u32 = 0;
    let mut already_logged_in = false; // Track Google login state across cycles
    let mut ip_rotation_count: u32 = 0; // Track IP rotations for this session
    const MAX_CAPTCHA_RESTARTS: u32 = 5; // Max restarts due to CAPTCHA before giving up
    let keyword_count = keywords.len();

    while is_running.load(Ordering::Relaxed) && session.is_alive() {
        // Check if we've reached the max clicks limit for this session
        if max_clicks > 0 && session_clicks >= max_clicks {
            info!("Session {} reached max clicks limit ({}), closing", session_id, max_clicks);
            break;
        }

        // Wait according to rate limiter
        rate_limiter.wait().await;

        // Increment cycle count
        session.increment_cycles();

        // Get next keyword (rotate through list)
        let keyword = &keywords[keyword_index % keywords.len()];
        keyword_index += 1;

        // Run a cycle (with optional Google login)
        let start = std::time::Instant::now();
        match BrowserActions::run_cycle_with_login(
            &session,
            keyword,
            rate_limiter.config().min_delay_ms,
            rate_limiter.config().max_delay_ms,
            google_account.as_ref(),
            &mut already_logged_in,
        ).await {
            Ok(clicked) => {
                let latency = start.elapsed().as_millis() as u64;
                if clicked {
                    stats.record_click(latency);
                    rate_limiter.record_success();
                    session_clicks += 1;
                    // Reset error counter on success - gives fresh chances for next cycles
                    captcha_restarts = 0;
                }

                // Check if we should auto-rotate IP after completing all keywords
                if auto_rotate_ip && keyword_count > 0 && keyword_index % keyword_count == 0 && keyword_index > 0 {
                    info!("Session {} completed {} keywords, rotating IP (#{}/{})",
                        session_id, keyword_count, ip_rotation_count + 1, session.ip_change_count() + 1);

                    // Close current session
                    let _ = pool.close_session(&session_id).await;

                    // Short delay before spawning new session
                    tokio::time::sleep(Duration::from_millis(1000)).await;

                    // Spawn new session with new IP
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
                                }
                                None => {
                                    error!("New session {} not found - stopping", session_id);
                                    break;
                                }
                            }
                        }
                        Ok(_) | Err(_) => {
                            error!("Failed to spawn replacement - stopping");
                            break;
                        }
                    }
                }
            }
            Err(BrowserError::CaptchaDetected(msg)) => {
                // IMMEDIATE IP CHANGE - no delay, fast restart
                warn!("Session {} BLOCKED: {} - IMMEDIATE IP change", session_id, msg);
                stats.record_error();
                session.increment_errors();

                // Check if we've exceeded max restarts (prevent infinite loop)
                captcha_restarts += 1;
                if captcha_restarts >= MAX_CAPTCHA_RESTARTS {
                    error!("Session {} exceeded max restarts ({}), stopping this session", session_id, MAX_CAPTCHA_RESTARTS);
                    break;
                }

                // Close current session IMMEDIATELY
                let _ = pool.close_session(&session_id).await;

                // Minimal delay - just enough for cleanup (500ms instead of 2000ms)
                tokio::time::sleep(Duration::from_millis(500)).await;

                // Spawn new session with new IP
                match pool.spawn_sessions(1).await {
                    Ok(new_ids) if !new_ids.is_empty() => {
                        let new_id = new_ids.into_iter().next().unwrap();
                        info!("Session {} -> {} (IP change #{}, reason: {})",
                            session_id, new_id, captcha_restarts, msg);
                        session_id = new_id;

                        match pool.get_session(&session_id).await {
                            Some(s) => {
                                session = s;
                                already_logged_in = false; // Reset login state
                                // Short delay before continuing (1s instead of 3s)
                                tokio::time::sleep(Duration::from_millis(1000)).await;
                            }
                            None => {
                                error!("New session {} not found - stopping", session_id);
                                break;
                            }
                        }
                    }
                    Ok(_) => {
                        error!("Failed to spawn replacement session - stopping");
                        break;
                    }
                    Err(e) => {
                        error!("Error spawning replacement: {} - stopping", e);
                        break;
                    }
                }
            }
            Err(BrowserError::ElementNotFound(msg)) if msg.contains("need new IP") => {
                // No ad found - change IP and retry
                warn!("Session {} no ad found: {} - changing IP", session_id, msg);

                // Record as failed attempt (for accurate success rate)
                stats.record_error();
                session.increment_errors();

                // Track consecutive "no ad" errors to prevent infinite loop
                captcha_restarts += 1;
                if captcha_restarts >= MAX_CAPTCHA_RESTARTS {
                    error!("Session {} exceeded max 'no ad found' restarts ({}), stopping", session_id, MAX_CAPTCHA_RESTARTS);
                    break;
                }

                // Close current session
                if let Err(e) = pool.close_session(&session_id).await {
                    warn!("Failed to close session {}: {}", session_id, e);
                }

                // Short delay before spawning new session (1-2s)
                tokio::time::sleep(Duration::from_millis(1000 + rand::random::<u64>() % 1000)).await;

                // Spawn a new session with new IP
                match pool.spawn_sessions(1).await {
                    Ok(new_ids) if !new_ids.is_empty() => {
                        let new_id = new_ids.into_iter().next().unwrap();
                        info!("Session {} replaced with new session {} (no ad found - new IP)",
                            session_id, new_id);
                        session_id = new_id;

                        // Get the new session
                        match pool.get_session(&session_id).await {
                            Some(s) => {
                                session = s;
                                already_logged_in = false; // Reset login state for new session
                                // Short wait before continuing (1s instead of 2-4s)
                                tokio::time::sleep(Duration::from_millis(1000)).await;
                            }
                            None => {
                                warn!("New session {} not found after spawn", session_id);
                                break;
                            }
                        }
                    }
                    Ok(_) => {
                        warn!("Failed to spawn replacement session");
                        break;
                    }
                    Err(e) => {
                        warn!("Error spawning replacement session: {}", e);
                        break;
                    }
                }
            }
            Err(BrowserError::Timeout(msg)) |
            Err(BrowserError::NavigationFailed(msg)) |
            Err(BrowserError::ConnectionLost(msg)) => {
                // Network/timeout errors - change IP immediately
                warn!("Session {} network error: {} - changing IP", session_id, msg);
                stats.record_error();
                session.increment_errors();

                // Track consecutive network errors to prevent infinite loop
                captcha_restarts += 1;
                if captcha_restarts >= MAX_CAPTCHA_RESTARTS {
                    error!("Session {} exceeded max network error restarts ({}), stopping", session_id, MAX_CAPTCHA_RESTARTS);
                    break;
                }

                // Close current session
                let _ = pool.close_session(&session_id).await;

                // Short delay before spawning new session (1s)
                tokio::time::sleep(Duration::from_millis(1000)).await;

                // Spawn new session with new IP
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
                            }
                            None => {
                                error!("New session {} not found - stopping", session_id);
                                break;
                            }
                        }
                    }
                    Ok(_) | Err(_) => {
                        error!("Failed to spawn replacement after network error - stopping");
                        break;
                    }
                }
            }
            Err(e) => {
                // Other errors - just log and continue (don't change IP)
                warn!("Session {} cycle error: {}", session_id, e);
                stats.record_error();
                rate_limiter.record_error();
                session.increment_errors();
            }
        }
    }

    // Close this session when done
    if let Err(e) = pool.close_session(&session_id).await {
        warn!("Failed to close session {}: {}", session_id, e);
    }

    info!("Session {} bot loop ended (clicks: {}, captcha_restarts: {})", session_id, session_clicks, captcha_restarts);
}

/// Stop the bot
#[tauri::command]
pub async fn stop_bot(state: State<'_, AppState>) -> Result<(), String> {
    info!("Stopping bot");
    state.is_running.store(false, Ordering::Relaxed);

    // Close all sessions
    state.browser_pool.close_all().await.map_err(|e| e.to_string())?;

    Ok(())
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

/// Proxy test result
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyTestResult {
    pub working: bool,
    pub original_ip: String,
    pub proxy_ip: Option<String>,
    pub error: Option<String>,
    pub test_time_ms: u64,
}

/// Test proxy connection
/// Fetch IP without proxy
async fn fetch_ip_without_proxy() -> Result<String, String> {
    let client = reqwest::Client::builder()
        .no_proxy() // Disable system proxy settings (important on Windows)
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

/// Fetch IP with proxy using URL-embedded credentials
async fn fetch_ip_with_proxy(proxy_url: &str) -> Result<String, String> {
    info!("Proxy test: connecting via {}", &proxy_url[..proxy_url.len().min(60)]);

    let proxy = Proxy::all(proxy_url)
        .map_err(|e| format!("Invalid proxy URL: {}", e))?;

    let client = reqwest::Client::builder()
        .proxy(proxy)
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| format!("Failed to create proxy client: {:?}", e))?;

    // Use HTTP (not HTTPS) to avoid CONNECT tunnel issues
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

/// Test proxy connectivity by comparing IPs with and without proxy
#[tauri::command]
pub async fn test_proxy(state: State<'_, AppState>) -> Result<ProxyTestResult, String> {
    info!("Testing proxy connectivity...");
    let start = std::time::Instant::now();

    // Step 1: Get original IP (without proxy)
    let original_ip = match fetch_ip_without_proxy().await {
        Ok(ip) => {
            info!("Original IP: {}", ip);
            ip
        }
        Err(e) => {
            error!("Failed to get original IP: {}", e);
            return Ok(ProxyTestResult {
                working: false,
                original_ip: "Unknown".to_string(),
                proxy_ip: None,
                error: Some(format!("Failed to get original IP: {}", e)),
                test_time_ms: start.elapsed().as_millis() as u64,
            });
        }
    };

    // Step 2: Build HTTP proxy URL for testing (HTTP mode is more reliable for testing)
    let config = state.config.read().await;
    if config.proxy_customer.is_empty() || config.proxy_password.is_empty() {
        return Ok(ProxyTestResult {
            working: false,
            original_ip,
            proxy_ip: None,
            error: Some("Proxy credentials not configured".into()),
            test_time_ms: start.elapsed().as_millis() as u64,
        });
    }

    // Build proxy URL with embedded credentials (HTTP mode port 60000)
    let sessid = format!("test{}", std::process::id());
    let password_encoded = urlencoding::encode(&config.proxy_password);
    let proxy_url = format!(
        "http://customer-{}-cc-{}-sessid-{}-sesstime-10:{}@pr.oxylabs.io:60000",
        config.proxy_customer,
        config.proxy_country,
        sessid,
        password_encoded
    );
    drop(config);

    info!("Testing HTTP proxy @ pr.oxylabs.io:60000 with sessid={}", sessid);

    // Step 3: Get IP through proxy
    match fetch_ip_with_proxy(&proxy_url).await {
        Ok(proxy_ip) => {
            let test_time_ms = start.elapsed().as_millis() as u64;
            let working = original_ip != proxy_ip;

            if working {
                info!("Proxy test SUCCESS: Original={}, Proxy={}, Time={}ms", original_ip, proxy_ip, test_time_ms);

                // Mark proxy as verified
                state.proxy_manager.set_verified(true);

                // Save verified status to config
                {
                    let mut cfg = state.config.write().await;
                    cfg.proxy_verified = true;
                    cfg.save();
                }
            } else {
                warn!("Proxy test FAILED: IPs are the same ({}), proxy not routing correctly", original_ip);
            }

            Ok(ProxyTestResult {
                working,
                original_ip,
                proxy_ip: Some(proxy_ip),
                error: if !working { Some("Proxy not routing - IPs are the same".into()) } else { None },
                test_time_ms,
            })
        }
        Err(e) => {
            let test_time_ms = start.elapsed().as_millis() as u64;
            error!("Proxy connection failed: {}", e);
            Ok(ProxyTestResult {
                working: false,
                original_ip,
                proxy_ip: None,
                error: Some(format!("Proxy connection failed: {}", e)),
                test_time_ms,
            })
        }
    }
}

/// Check if proxy is verified
#[tauri::command]
pub async fn is_proxy_verified(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(state.proxy_manager.is_verified())
}

/// Get the log directory path so the user can find log files
#[tauri::command]
pub async fn get_log_dir() -> Result<String, String> {
    crate::log_dir()
        .map(|p| p.to_string_lossy().to_string())
        .ok_or_else(|| "Could not determine log directory".to_string())
}

// ========== CAPTCHA Commands ==========

/// CAPTCHA balance result
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptchaBalanceResult {
    pub balance: f64,
    pub configured: bool,
}

/// Get 2Captcha account balance
#[tauri::command]
pub async fn get_captcha_balance(state: State<'_, AppState>) -> Result<CaptchaBalanceResult, String> {
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

/// Test CAPTCHA solving
#[tauri::command]
pub async fn test_captcha(state: State<'_, AppState>) -> Result<CaptchaTestResult, String> {
    info!("Testing CAPTCHA solving...");
    let start = std::time::Instant::now();

    let config = state.config.read().await;

    if config.captcha_api_key.is_empty() {
        return Err("2Captcha API key not configured".into());
    }

    let solver = crate::captcha::CaptchaSolver::new(&config.captcha_api_key)
        .map_err(|e| e.to_string())?;

    // Test with GrintaHub login CAPTCHA
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

/// CAPTCHA test result
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptchaTestResult {
    pub success: bool,
    pub solve_time_ms: u64,
    pub total_time_ms: u64,
    pub token_preview: String,
    pub error: Option<String>,
}

// ========== Auth Commands ==========

/// Account info for frontend
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountInfo {
    pub email: String,
    pub name: String,
    pub phone: Option<String>,
}

/// Register a new GrintaHub account
#[tauri::command]
pub async fn register_account(
    state: State<'_, AppState>,
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

    // Use provided values or generate fake data
    let name = name.unwrap_or_else(crate::auth::FakeData::random_name);
    let email = email.unwrap_or_else(crate::auth::FakeData::random_email);
    let phone = phone.unwrap_or_else(crate::auth::FakeData::random_saudi_phone);

    drop(config);

    let account = auth_client
        .register_with_captcha(&solver, &name, &email, &phone, &password)
        .await
        .map_err(|e| e.to_string())?;

    // Save account to config
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

/// Login to a GrintaHub account
#[tauri::command]
pub async fn login_account(
    state: State<'_, AppState>,
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

/// Batch register multiple accounts
#[tauri::command]
pub async fn batch_register_accounts(
    state: State<'_, AppState>,
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

/// Get saved accounts
#[tauri::command]
pub async fn get_saved_accounts(state: State<'_, AppState>) -> Result<Vec<AccountInfo>, String> {
    let config = state.config.read().await;

    let accounts = config.accounts.iter().map(|a| AccountInfo {
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
