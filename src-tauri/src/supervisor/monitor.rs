//! Session supervisor that monitors and auto-recovers browser sessions.
//!
//! Periodically checks if the number of active sessions matches the target
//! and spawns replacements for any that died (panic, crash, etc.).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{info, warn, error};

use crate::AppConfig;
use crate::browser::BrowserPool;
use crate::stats::GlobalStats;
use crate::rate::RateLimiterConfig;

/// Supervisor configuration
pub struct SupervisorConfig {
    /// How often to check session count (default: 10s)
    pub check_interval: Duration,
    /// Delay before first check after startup (default: 30s)
    pub initial_delay: Duration,
    /// Max sessions to spawn in a single recovery cycle (default: 3)
    pub max_recovery_batch: usize,
}

impl Default for SupervisorConfig {
    fn default() -> Self {
        Self {
            check_interval: Duration::from_secs(10),
            initial_delay: Duration::from_secs(30),
            max_recovery_batch: 3,
        }
    }
}

/// Session supervisor that auto-recovers missing sessions
pub struct SessionSupervisor;

impl SessionSupervisor {
    /// Start the supervisor background task.
    ///
    /// Runs until `is_running` becomes false.
    /// Periodically checks `pool.session_count()` vs `config.concurrent_sessions`
    /// and spawns replacements if sessions are missing.
    pub fn start(
        is_running: Arc<AtomicBool>,
        browser_pool: Arc<BrowserPool>,
        global_stats: Arc<GlobalStats>,
        rate_config: Arc<RwLock<RateLimiterConfig>>,
        app_config: Arc<RwLock<AppConfig>>,
        supervisor_config: SupervisorConfig,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            // Wait for initial sessions to stabilize
            tokio::time::sleep(supervisor_config.initial_delay).await;

            if !is_running.load(Ordering::Relaxed) {
                return;
            }

            info!("[Supervisor] Started monitoring sessions (check every {}s)",
                supervisor_config.check_interval.as_secs());

            let mut tick_counter: u64 = 0;

            while is_running.load(Ordering::Relaxed) {
                tokio::time::sleep(supervisor_config.check_interval).await;

                if !is_running.load(Ordering::Relaxed) {
                    break;
                }

                tick_counter += 1;

                // Read config for target session count
                let config = app_config.read().await;
                let target = config.concurrent_sessions;
                let keywords = config.keywords.clone();
                let max_clicks = config.max_clicks_per_session;
                let auto_rotate_ip = config.auto_rotate_ip;
                let captcha_api_key = config.captcha_api_key.clone();
                let headless = config.headless;

                // Extract Google account
                let google_account: Option<crate::browser::GoogleAccount> = config.accounts
                    .first()
                    .and_then(|acc| {
                        if !acc.email.is_empty() && acc.token.as_ref().map(|t| !t.is_empty()).unwrap_or(false) {
                            Some(crate::browser::GoogleAccount {
                                email: acc.email.clone(),
                                password: acc.token.clone().unwrap_or_default(),
                            })
                        } else {
                            None
                        }
                    });
                drop(config);

                // Check current session count (ground truth from pool HashMap)
                let current = browser_pool.session_count().await;

                if current < target {
                    let deficit = target - current;
                    let to_spawn = deficit.min(supervisor_config.max_recovery_batch);

                    warn!(
                        "[Supervisor] Session deficit: have {}, target {}, recovering {} sessions",
                        current, target, to_spawn
                    );

                    // Correct the active_sessions counter to match reality
                    global_stats.set_active_sessions(current as u64);

                    // Spawn replacement sessions
                    match browser_pool.spawn_sessions_with_options(to_spawn, Some(headless)).await {
                        Ok(session_ids) => {
                            info!("[Supervisor] Spawned {} replacement sessions", session_ids.len());

                            // Start session loops for each replacement
                            for session_id in session_ids {
                                crate::bot::spawn_session_task_safe(
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
                        }
                        Err(e) => {
                            error!("[Supervisor] Failed to spawn replacement sessions: {}", e);
                        }
                    }
                }

                // Zombie Chrome cleanup every ~60 seconds (every 6th tick at 10s interval)
                if tick_counter % 6 == 0 {
                    let killed = super::zombie::cleanup_zombie_chromes(&browser_pool).await;
                    if killed > 0 {
                        warn!("[Supervisor] Cleaned up {} zombie Chrome processes", killed);
                    }
                }
            }

            info!("[Supervisor] Stopped monitoring");
        })
    }
}
