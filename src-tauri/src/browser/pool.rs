//! Browser session pool
//!
//! Manages multiple browser instances running in parallel with unique proxies.

use std::sync::Arc;
use std::collections::HashMap;
use tokio::sync::RwLock;
use tracing::{info, warn, error};
use uuid::Uuid;

use super::{BrowserSession, BrowserSessionConfig, BrowserError};
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
}

impl BrowserPool {
    /// Create a new browser pool
    pub fn new(proxy_manager: Arc<GlobalProxyManager>) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            proxy_manager,
            default_config: BrowserSessionConfig::default(),
            statuses: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Set default configuration for new sessions
    pub fn with_config(mut self, config: BrowserSessionConfig) -> Self {
        self.default_config = config;
        self
    }

    /// Spawn multiple browser sessions in parallel (uses default headless setting)
    pub async fn spawn_sessions(&self, count: usize) -> Result<Vec<String>, BrowserError> {
        self.spawn_sessions_with_options(count, None).await
    }

    /// Spawn multiple browser sessions with staggered starts
    ///
    /// Each session gets a unique proxy URL to ensure different IPs.
    /// Sessions are spawned with a delay between them to avoid resource conflicts.
    /// Optionally override headless mode.
    pub async fn spawn_sessions_with_options(&self, count: usize, headless: Option<bool>) -> Result<Vec<String>, BrowserError> {
        use std::time::Duration;

        let headless_mode = headless.unwrap_or(self.default_config.headless);
        info!("=== SPAWNING {} BROWSER SESSIONS (headless: {}) ===", count, headless_mode);

        // Get unique proxies for each session
        let proxies = if self.proxy_manager.is_enabled() {
            info!("Proxy enabled, getting {} unique proxy URLs", count);
            self.proxy_manager.next_batch(count)
        } else {
            info!("Proxy disabled, sessions will use direct connection");
            None
        };

        let mut session_ids = Vec::with_capacity(count);

        // Spawn sessions sequentially with staggered delays to avoid Chrome conflicts
        // This is more reliable than parallel spawning which can cause port/resource conflicts
        for i in 0..count {
            info!(">>> Starting session {}/{}", i + 1, count);

            let proxy = proxies.as_ref().map(|p| p[i].clone());
            if let Some(ref p) = proxy {
                info!("  Session {} will use proxy: {}", i + 1, p.split('@').last().unwrap_or("unknown"));
            }

            // Use unique random ID for each session to ensure fresh browser profile
            let unique_id = format!("{}_{}", Uuid::new_v4().to_string()[..8].to_string(), i);
            let config = BrowserSessionConfig::for_session(&unique_id)
                .headless(headless_mode)
                .proxy(proxy)
                .chrome_path(self.default_config.chrome_path.clone())
                .timeout(self.default_config.timeout_secs);

            // Set status to starting
            {
                let mut s = self.statuses.write().await;
                s.insert(format!("pending_{}", i), SessionStatus::Starting);
            }

            // Launch browser with timeout
            let launch_result = tokio::time::timeout(
                Duration::from_secs(60), // 60 second timeout for browser launch
                BrowserSession::new(config)
            ).await;

            match launch_result {
                Ok(Ok(session)) => {
                    let session_id = session.id.clone();
                    info!("<<< Session {}/{} created: {} (IP detection pending)", i + 1, count, session_id);
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
                Ok(Err(e)) => {
                    error!("!!! Session {}/{} FAILED: {}", i + 1, count, e);
                    {
                        let mut s = self.statuses.write().await;
                        s.remove(&format!("pending_{}", i));
                        s.insert(format!("failed_{}", i), SessionStatus::Error(e.to_string()));
                    }
                }
                Err(_) => {
                    error!("!!! Session {}/{} TIMED OUT (60s)", i + 1, count);
                    {
                        let mut s = self.statuses.write().await;
                        s.remove(&format!("pending_{}", i));
                        s.insert(format!("failed_{}", i), SessionStatus::Error("Browser launch timed out".to_string()));
                    }
                }
            }

            // Add delay between session spawns (except for last one)
            // This prevents Chrome instances from conflicting with each other
            if i < count - 1 {
                info!("  Waiting 2s before spawning next session...");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }

        info!("=== SUCCESSFULLY SPAWNED {}/{} SESSIONS ===", session_ids.len(), count);

        if session_ids.is_empty() && count > 0 {
            return Err(BrowserError::LaunchFailed("All session launches failed".to_string()));
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

        // Clear statuses
        self.statuses.write().await.clear();

        info!("All browser sessions closed");
        Ok(())
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
