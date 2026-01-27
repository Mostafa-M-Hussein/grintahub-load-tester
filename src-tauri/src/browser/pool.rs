//! Browser session pool
//!
//! Manages multiple browser instances running in parallel with unique proxies.

use std::sync::Arc;
use std::collections::{HashMap, HashSet};
use tokio::sync::RwLock;
use tracing::{info, warn, error};
use uuid::Uuid;

use super::{BrowserSession, BrowserSessionConfig, BrowserError, reset_bot_counter};
use crate::proxy::GlobalProxyManager;

/// Information about a browser session (for frontend)
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub id: String,
    pub alive: bool,
    pub current_ip: Option<String>,
    pub previous_ip: Option<String>,
    pub ip_change_count: u32,
    pub click_count: u64,
    pub error_count: u64,
    pub cycle_count: u64,
    pub captcha_count: u32,
    pub status: SessionStatus,
}

/// Session status
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionStatus {
    Starting,
    Running,
    Paused,
    Error(String),
    Stopped,
}

/// Browser pool for managing multiple sessions
pub struct BrowserPool {
    /// All active sessions
    sessions: Arc<RwLock<HashMap<String, Arc<BrowserSession>>>>,
    /// Proxy manager for generating unique proxies
    proxy_manager: Arc<GlobalProxyManager>,
    /// Default session configuration
    default_config: BrowserSessionConfig,
    /// Session statuses
    statuses: Arc<RwLock<HashMap<String, SessionStatus>>>,
    /// Track all IPs used in this run to prevent duplicates
    used_ips: Arc<RwLock<HashSet<String>>>,
    /// Runtime headless override — set when bot starts so replacement sessions
    /// spawned via `spawn_sessions(1)` use the correct headless mode.
    headless_override: RwLock<Option<bool>>,
    /// Runtime captcha extension dir override — set when bot starts so all
    /// sessions load the 2Captcha solver extension.
    captcha_extension_override: RwLock<Option<String>>,
}

impl BrowserPool {
    /// Create a new browser pool
    pub fn new(proxy_manager: Arc<GlobalProxyManager>) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            proxy_manager,
            default_config: BrowserSessionConfig::default(),
            statuses: Arc::new(RwLock::new(HashMap::new())),
            used_ips: Arc::new(RwLock::new(HashSet::new())),
            headless_override: RwLock::new(None),
            captcha_extension_override: RwLock::new(None),
        }
    }

    /// Set default configuration for new sessions
    pub fn with_config(mut self, config: BrowserSessionConfig) -> Self {
        self.default_config = config;
        self
    }

    /// Set the default headless mode for all future sessions.
    ///
    /// Called when the bot starts so that replacement sessions spawned via
    /// `spawn_sessions(1)` (IP rotation, error recovery, etc.) inherit
    /// the correct headless setting from the config.
    pub async fn set_default_headless(&self, headless: bool) {
        self.headless_override.write().await.replace(headless);
    }

    /// Set the 2Captcha extension directory for all future sessions.
    pub async fn set_captcha_extension(&self, extension_dir: Option<String>) {
        *self.captcha_extension_override.write().await = extension_dir;
    }

    /// Spawn multiple browser sessions in parallel (uses default headless setting)
    pub async fn spawn_sessions(&self, count: usize) -> Result<Vec<String>, BrowserError> {
        self.spawn_sessions_with_options(count, None).await
    }

    /// Spawn multiple browser sessions in TRUE PARALLEL
    ///
    /// Each session gets a unique proxy URL to ensure different IPs.
    /// All sessions launch simultaneously for maximum speed.
    /// Optionally override headless mode.
    pub async fn spawn_sessions_with_options(&self, count: usize, headless: Option<bool>) -> Result<Vec<String>, BrowserError> {
        use std::time::Duration;
        use futures::future::join_all;

        let headless_mode = match headless {
            Some(h) => h,
            None => {
                // Check runtime override first (set by bot on start), fall back to default_config
                self.headless_override.read().await.unwrap_or(self.default_config.headless)
            }
        };
        info!("=== SPAWNING {} BROWSER SESSIONS IN PARALLEL (headless: {}) ===", count, headless_mode);

        // Get unique proxies for each session
        let proxies = if self.proxy_manager.is_enabled() {
            info!("Proxy enabled, getting {} unique proxy URLs", count);
            self.proxy_manager.next_batch(count)
        } else {
            info!("Proxy disabled, sessions will use direct connection");
            None
        };

        // Set all statuses to starting
        {
            let mut s = self.statuses.write().await;
            for i in 0..count {
                s.insert(format!("pending_{}", i), SessionStatus::Starting);
            }
        }

        // Prepare all session configs
        let mut spawn_tasks = Vec::with_capacity(count);
        for i in 0..count {
            let proxy = proxies.as_ref().map(|p| p[i].clone());
            let unique_id = format!("{}_{}", Uuid::new_v4().to_string()[..8].to_string(), i);
            // Use captcha extension override if set, otherwise fall back to default config
            let ext_dir = self.captcha_extension_override.read().await.clone()
                .or_else(|| self.default_config.captcha_extension_dir.clone());

            let config = BrowserSessionConfig::for_session(&unique_id)
                .headless(headless_mode)
                .proxy(proxy.clone())
                .chrome_path(self.default_config.chrome_path.clone())
                .timeout(self.default_config.timeout_secs)
                .captcha_extension(ext_dir);

            if let Some(ref p) = proxy {
                info!("Session {} will use proxy: {}", i + 1, p.split('@').last().unwrap_or("unknown"));
            }

            // Spawn each session launch as a concurrent task
            spawn_tasks.push(tokio::spawn(async move {
                let result = tokio::time::timeout(
                    Duration::from_secs(45), // Reduced timeout for faster failure detection
                    BrowserSession::new(config)
                ).await;
                (i, result)
            }));
        }

        info!("All {} session launches started simultaneously, waiting for completion...", count);

        // Wait for all sessions to complete in parallel
        let results = join_all(spawn_tasks).await;
        let mut session_ids = Vec::with_capacity(count);

        for result in results {
            match result {
                Ok((i, Ok(Ok(session)))) => {
                    let session_id = session.id.clone();
                    info!("<<< Session {}/{} created: {}", i + 1, count, session_id);
                    let session = Arc::new(session);

                    // Store session
                    {
                        let mut s = self.sessions.write().await;
                        s.insert(session_id.clone(), session);
                    }

                    // Update status
                    {
                        let mut s = self.statuses.write().await;
                        s.remove(&format!("pending_{}", i));
                        s.insert(session_id.clone(), SessionStatus::Running);
                    }

                    session_ids.push(session_id);
                }
                Ok((i, Ok(Err(e)))) => {
                    error!("!!! Session {}/{} FAILED: {}", i + 1, count, e);
                    let mut s = self.statuses.write().await;
                    s.remove(&format!("pending_{}", i));
                    s.insert(format!("failed_{}", i), SessionStatus::Error(e.to_string()));
                }
                Ok((i, Err(_))) => {
                    error!("!!! Session {}/{} TIMED OUT (45s)", i + 1, count);
                    let mut s = self.statuses.write().await;
                    s.remove(&format!("pending_{}", i));
                    s.insert(format!("failed_{}", i), SessionStatus::Error("Browser launch timed out".to_string()));
                }
                Err(e) => {
                    error!("!!! Session task panicked: {}", e);
                }
            }
        }

        info!("=== {} of {} sessions launched successfully ===", session_ids.len(), count);

        if session_ids.is_empty() && count > 0 {
            return Err(BrowserError::PoolError("All session launches failed".into()));
        }

        Ok(session_ids)
    }

    /// Get a session by ID
    pub async fn get_session(&self, id: &str) -> Option<Arc<BrowserSession>> {
        self.sessions.read().await.get(id).cloned()
    }

    /// Get all sessions
    pub async fn get_all_sessions(&self) -> Vec<Arc<BrowserSession>> {
        self.sessions.read().await.values().cloned().collect()
    }

    /// Get session info for all sessions (for frontend)
    pub async fn get_all_session_info(&self) -> Vec<SessionInfo> {
        let sessions = self.sessions.read().await;
        let statuses = self.statuses.read().await;

        let mut infos = Vec::new();

        for (id, session) in sessions.iter() {
            let status = statuses.get(id).cloned().unwrap_or(SessionStatus::Running);
            let current_ip = session.current_ip().await;
            let previous_ip = session.previous_ip().await;

            infos.push(SessionInfo {
                id: id.clone(),
                alive: session.is_alive(),
                current_ip,
                previous_ip,
                ip_change_count: session.ip_change_count(),
                click_count: session.click_count(),
                error_count: session.error_count(),
                cycle_count: session.cycle_count(),
                captcha_count: session.captcha_count(),
                status,
            });
        }

        infos
    }

    /// Get session count
    pub async fn session_count(&self) -> usize {
        self.sessions.read().await.len()
    }

    /// Update session status
    pub async fn set_status(&self, id: &str, status: SessionStatus) {
        let mut statuses = self.statuses.write().await;
        statuses.insert(id.to_string(), status);
    }

    /// Close a specific session
    pub async fn close_session(&self, id: &str) -> Result<(), BrowserError> {
        let session = {
            let mut sessions = self.sessions.write().await;
            sessions.remove(id)
        };

        if let Some(session) = session {
            session.close().await?;
            let mut statuses = self.statuses.write().await;
            statuses.insert(id.to_string(), SessionStatus::Stopped);
        }

        Ok(())
    }

    /// Close all sessions
    pub async fn close_all(&self) -> Result<(), BrowserError> {
        let sessions: Vec<Arc<BrowserSession>> = {
            let mut sessions = self.sessions.write().await;
            sessions.drain().map(|(_, s)| s).collect()
        };

        for session in sessions {
            if let Err(e) = session.close().await {
                warn!("Error closing session {}: {}", session.id, e);
            }
        }

        // Clear statuses, used IPs, headless override, and reset bot counter
        self.statuses.write().await.clear();
        self.used_ips.write().await.clear();
        *self.headless_override.write().await = None;
        reset_bot_counter();

        info!("All browser sessions closed (used IPs cleared, bot counter reset)");
        Ok(())
    }

    /// Register an IP as used. Returns true if the IP is new (not seen before).
    pub async fn register_ip(&self, ip: &str) -> bool {
        let mut used = self.used_ips.write().await;
        let is_new = used.insert(ip.to_string());
        if !is_new {
            warn!("DUPLICATE IP detected: {} (already used by another session)", ip);
        }
        is_new
    }

    /// Check if an IP has been used before
    pub async fn is_ip_used(&self, ip: &str) -> bool {
        self.used_ips.read().await.contains(ip)
    }

    /// Get count of unique IPs used
    pub async fn used_ip_count(&self) -> usize {
        self.used_ips.read().await.len()
    }

    /// Detect IP for all sessions
    pub async fn detect_all_ips(&self) -> HashMap<String, Result<String, String>> {
        let sessions = self.get_all_sessions().await;
        let mut results = HashMap::new();

        for session in sessions {
            let id = session.id.clone();
            match session.detect_ip().await {
                Ok(ip) => {
                    info!("Session {} IP: {}", id, ip);
                    results.insert(id, Ok(ip));
                }
                Err(e) => {
                    warn!("Session {} IP detection failed: {}", id, e);
                    results.insert(id, Err(e.to_string()));
                }
            }
        }

        results
    }
}
