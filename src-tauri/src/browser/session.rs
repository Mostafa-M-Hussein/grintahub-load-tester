//! Browser session management
//!
//! Handles launching and controlling individual Chrome browser instances.
//! Uses a local proxy forwarder to handle authenticated upstream proxies.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{info, warn, debug};
use chaser_oxide::{Browser, BrowserConfig, Page};
use chaser_oxide::chaser::ChaserPage;
use chaser_oxide::profiles::{ChaserProfile, Gpu};
use futures::StreamExt;
use rand::Rng;

use std::sync::atomic::AtomicU32;

use super::BrowserError;
use crate::proxy::LocalProxyForwarder;

/// Global counter for sequential bot naming (Bot-1, Bot-2, ...)
static BOT_COUNTER: AtomicU32 = AtomicU32::new(1);

/// Reset the bot counter back to 1 (call when all sessions are closed)
pub fn reset_bot_counter() {
    BOT_COUNTER.store(1, Ordering::Relaxed);
}

/// Detect the major Chrome version from the installed binary.
/// Returns (major_version, full_version_string) e.g. (142, "142.0.7444.175")
fn detect_chrome_version() -> Option<(u32, String)> {
    let chrome_path = find_chrome()?;
    let output = std::process::Command::new(&chrome_path)
        .arg("--version")
        .output()
        .ok()?;
    let version_str = String::from_utf8_lossy(&output.stdout);
    // Parse "Google Chrome 142.0.7444.175" or "Chromium 142.0.7444.175"
    let full_ver = version_str
        .split_whitespace()
        .find(|s| s.contains('.'))?
        .trim()
        .to_string();
    let major: u32 = full_ver.split('.').next()?.parse().ok()?;
    info!("Detected Chrome version: {} (major: {})", full_ver, major);
    Some((major, full_ver))
}

/// Find Chrome/Chromium executable on the system
fn find_chrome() -> Option<std::path::PathBuf> {
    let candidates: Vec<std::path::PathBuf> = if cfg!(target_os = "windows") {
        let mut paths = vec![
            std::path::PathBuf::from(r"C:\Program Files\Google\Chrome\Application\chrome.exe"),
            std::path::PathBuf::from(r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe"),
        ];
        // Also check %LOCALAPPDATA%
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            paths.push(std::path::PathBuf::from(format!(r"{}\Google\Chrome\Application\chrome.exe", local)));
        }
        paths
    } else if cfg!(target_os = "macos") {
        vec![
            std::path::PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
        ]
    } else {
        // Chromium MUST come first: Google Chrome blocks --load-extension
        // (chrome/browser/extensions/extension_service.cc: "not allowed in Google Chrome")
        // Chromium allows loading unpacked extensions, which we need for 2Captcha solver
        vec![
            std::path::PathBuf::from("/usr/bin/chromium"),
            std::path::PathBuf::from("/usr/bin/chromium-browser"),
            std::path::PathBuf::from("/usr/bin/google-chrome"),
            std::path::PathBuf::from("/usr/bin/google-chrome-stable"),
        ]
    };

    candidates.into_iter().find(|p| p.exists())
}

/// Create a ChaserProfile configured for Saudi Arabia
/// Uses Linux profile to match the actual host OS — avoids platform mismatch detection.
/// ChaserProfile::windows() was causing Google to detect inconsistency between
/// the claimed Windows platform and the actual Linux TLS/font/header signatures.
/// Chrome version is auto-detected from the installed binary for consistency.
fn create_saudi_profile(chrome_major: u32) -> ChaserProfile {
    ChaserProfile::linux()
        .chrome_version(chrome_major)
        .gpu(Gpu::NvidiaGTX1660) // Single GPU — no rotation (reduces fingerprint entropy)
        .memory_gb(8)
        .cpu_cores(8)
        .locale("ar-SA")
        .timezone("Asia/Riyadh")
        .screen(1920, 1080)
        .build()
}

/// Configuration for a browser session
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserSessionConfig {
    /// Path to Chrome/Chromium executable
    pub chrome_path: Option<String>,
    /// Run in headless mode
    pub headless: bool,
    /// User data directory
    pub user_data_dir: Option<String>,
    /// Proxy URL
    pub proxy: Option<String>,
    /// Request timeout in seconds
    pub timeout_secs: u64,
    /// Window width
    pub window_width: u32,
    /// Window height
    pub window_height: u32,
    /// Path to the 2Captcha solver extension directory (unpacked)
    pub captcha_extension_dir: Option<String>,
}

impl Default for BrowserSessionConfig {
    fn default() -> Self {
        Self {
            chrome_path: None,
            headless: false,
            user_data_dir: None,
            proxy: None,
            timeout_secs: 60, // Increased from 30 to 60 seconds for slow proxy connections
            window_width: 1920,
            window_height: 1080,
            captcha_extension_dir: None,
        }
    }
}

impl BrowserSessionConfig {
    /// Create config for a specific session with data directory
    pub fn for_session(session_id: &str) -> Self {
        let base = std::env::temp_dir()
            .join("grintahub-clicker")
            .join("browser_data");

        let user_data_dir = base.join(session_id).to_string_lossy().to_string();

        Self {
            user_data_dir: Some(user_data_dir),
            ..Default::default()
        }
    }

    /// Set headless mode
    pub fn headless(mut self, headless: bool) -> Self {
        self.headless = headless;
        self
    }

    /// Set proxy
    pub fn proxy(mut self, proxy: Option<String>) -> Self {
        self.proxy = proxy;
        self
    }

    /// Set Chrome path
    pub fn chrome_path(mut self, path: Option<String>) -> Self {
        self.chrome_path = path;
        self
    }

    /// Set timeout
    pub fn timeout(mut self, timeout_secs: u64) -> Self {
        self.timeout_secs = timeout_secs;
        self
    }

    /// Set 2Captcha extension directory
    pub fn captcha_extension(mut self, dir: Option<String>) -> Self {
        self.captcha_extension_dir = dir;
        self
    }

    /// Find the 2Captcha solver extension directory.
    /// Searches in order: next to executable, current working directory, src-tauri (dev).
    pub fn find_captcha_extension() -> Option<String> {
        let candidates = vec![
            // Next to executable
            std::env::current_exe().ok().and_then(|p| {
                p.parent().map(|d| d.join("extensions").join("2captcha-solver"))
            }),
            // Current working directory
            Some(std::path::PathBuf::from("extensions/2captcha-solver")),
            // Dev mode (src-tauri relative)
            Some(std::path::PathBuf::from("src-tauri/extensions/2captcha-solver")),
        ];

        for candidate in candidates.into_iter().flatten() {
            let config_path = candidate.join("common").join("config.js");
            if config_path.exists() {
                if let Some(path_str) = candidate.to_str() {
                    info!("Found 2Captcha extension at: {}", path_str);
                    return Some(path_str.to_string());
                }
            }
        }

        warn!("2Captcha browser extension not found in any search path");
        None
    }

    /// Configure the 2Captcha extension with the given API key.
    /// Writes the API key into the extension's config.js defaults.
    /// Only writes if the key actually changed (avoids triggering Tauri dev rebuild).
    pub fn configure_captcha_extension(extension_dir: &str, api_key: &str) -> Result<(), String> {
        let config_path = std::path::Path::new(extension_dir).join("common").join("config.js");

        if !config_path.exists() {
            return Err(format!("Extension config.js not found at: {}", config_path.display()));
        }

        let content = std::fs::read_to_string(&config_path)
            .map_err(|e| format!("Failed to read config.js: {}", e))?;

        let expected_line = format!("apiKey: \"{}\",", api_key);

        // Check if the key is already set correctly — skip write to avoid triggering
        // Tauri dev watcher rebuild loop
        if content.contains(&expected_line) {
            info!("2Captcha extension already configured with correct API key ({}...)", crate::safe_truncate(api_key, 8));
            return Ok(());
        }

        // Replace the apiKey value (either null, empty string, or existing key)
        let updated = content
            .replace(
                &Self::find_api_key_line(&content).unwrap_or_else(|| "apiKey: \"\",".to_string()),
                &expected_line,
            );

        std::fs::write(&config_path, &updated)
            .map_err(|e| format!("Failed to write config.js: {}", e))?;

        info!("2Captcha extension configured with API key ({}...)", crate::safe_truncate(api_key, 8));
        Ok(())
    }

    /// Find the apiKey line in config.js content
    fn find_api_key_line(content: &str) -> Option<String> {
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("apiKey:") {
                return Some(trimmed.to_string());
            }
        }
        None
    }
}

/// A browser session for automation
pub struct BrowserSession {
    /// Unique session ID (display name, e.g. "Bot-1")
    pub id: String,
    /// The data directory session ID (UUID-based, used in --user-data-dir path)
    /// This is what appears in Chrome's command line and is used by the zombie cleaner.
    pub data_dir_id: String,
    /// The browser instance
    browser: Arc<RwLock<Option<Browser>>>,
    /// Current active page (ChaserPage wraps Page with stealth execution)
    page: Arc<RwLock<Option<ChaserPage>>>,
    /// Stealth profile (Saudi Arabia fingerprint)
    profile: ChaserProfile,
    /// Session configuration
    config: BrowserSessionConfig,
    /// Whether session is alive
    alive: Arc<AtomicBool>,
    /// Current proxy IP (detected)
    current_ip: Arc<RwLock<Option<String>>>,
    /// Previous IP (for change detection)
    previous_ip: Arc<RwLock<Option<String>>>,
    /// Number of times IP has changed
    ip_change_count: Arc<std::sync::atomic::AtomicU32>,
    /// Click count
    click_count: Arc<AtomicU64>,
    /// Error count
    error_count: Arc<AtomicU64>,
    /// Current cycle count
    cycle_count: Arc<AtomicU64>,
    /// Number of CAPTCHAs encountered
    captcha_count: Arc<std::sync::atomic::AtomicU32>,
    /// Local proxy forwarder (handles auth to upstream proxy)
    proxy_forwarder: Arc<RwLock<Option<LocalProxyForwarder>>>,
}

impl BrowserSession {
    /// Create a new browser session with the given config
    pub async fn new(config: BrowserSessionConfig) -> Result<Self, BrowserError> {
        let session_id = format!("Bot-{}", BOT_COUNTER.fetch_add(1, Ordering::Relaxed));

        // Extract the data_dir_id from user_data_dir path (the last component of the path)
        // This is the UUID-based ID used in --user-data-dir and matched by the zombie cleaner.
        let data_dir_id = config.user_data_dir.as_ref()
            .and_then(|dir| {
                std::path::Path::new(dir)
                    .file_name()
                    .map(|f| f.to_string_lossy().to_string())
            })
            .unwrap_or_default();

        info!("Launching browser session {} (headless: {})", session_id, config.headless);

        // Check if Chrome is available before attempting launch
        if config.chrome_path.is_none() && find_chrome().is_none() {
            return Err(BrowserError::LaunchFailed(
                "Google Chrome not found. Please install Chrome from https://www.google.com/chrome/ and restart the app.".to_string()
            ));
        }

        // Build browser config
        let mut builder = BrowserConfig::builder();

        // Set headless mode
        if config.headless {
            // Modern Chrome requires --headless=new for proper headless
            builder = builder.arg(("headless", "new"));
        } else {
            builder = builder.with_head();
        }

        // Set Chrome path if specified (or use auto-detected path)
        if let Some(ref path) = config.chrome_path {
            builder = builder.chrome_executable(path);
        } else if let Some(chrome_path) = find_chrome() {
            info!("Auto-detected Chrome at: {}", chrome_path.display());
            builder = builder.chrome_executable(chrome_path);
        }

        // Set user data directory
        if let Some(ref dir) = config.user_data_dir {
            // Create directory if it doesn't exist
            let _ = std::fs::create_dir_all(dir);
            builder = builder.user_data_dir(dir);
        }

        // =========== STEALTH FLAGS (chaser-oxide Arg API) ===========
        // IMPORTANT: Keys must NOT include "--" prefix — ArgsBuilder adds it automatically.
        // Use "flag" for boolean flags, ("key", "value") for key=value pairs.
        // Many flags (no-first-run, disable-sync, disable-dev-shm-usage, etc.)
        // are already in chaser-oxide's DEFAULT_ARGS and don't need repeating.

        builder = builder
            // Anti-detection (undetected-chromedriver style)
            .arg(("disable-blink-features", "AutomationControlled"))
            .arg(("exclude-switches", "enable-automation"))
            .arg("disable-automation")
            .arg("disable-infobars")
            .arg("no-default-browser-check")

            // Window position (size is set via builder.window_size())
            .arg(("window-position", "50,50"))

            // Disable features (merged into ONE call — HashMap overwrites duplicate keys)
            // DEFAULT_ARGS has TranslateUI, we must include it plus our extras
            .arg(("disable-features", "TranslateUI,AutomationControlled,IsolateOrigins,site-per-process,AudioServiceOutOfProcess"))

            // Disable session restore (no "restore tabs" prompt)
            .arg("disable-session-crashed-bubble")
            .arg("disable-restore-session-state")

            // Start with blank page
            .arg(("homepage", "about:blank"))

            // Site isolation
            .arg("disable-site-isolation-trials")

            // UI suppression
            .arg("disable-notifications")
            .arg("disable-save-password-bubble")
            .arg("disable-translate")

            // Other
            .arg("disable-domain-reliability")
            .arg("disable-component-update")
            // Required when running as root (e.g., in Docker or on VPS)
            .arg("no-sandbox");

        // Load 2Captcha extension via chaser-oxide's builder method
        // IMPORTANT: Must use .extension() not .arg(("load-extension", ...))
        // because chaser-oxide adds --disable-extensions when its internal
        // extensions vec is empty, which overrides any manual --load-extension args.
        if let Some(ref ext_dir) = config.captcha_extension_dir {
            info!("Session {} loading 2Captcha extension from: {}", session_id, ext_dir);
            builder = builder.extension(ext_dir);
        }

        // Auto-detect Chrome version for consistent fingerprint
        let (chrome_major, chrome_full_ver) = detect_chrome_version()
            .unwrap_or_else(|| {
                warn!("Could not detect Chrome version, defaulting to 131");
                (131, "131.0.6778.139".to_string())
            });

        // ChaserProfile handles: user-agent, lang, timezone, platform
        // Chrome version must match the installed binary for consistency
        let profile = create_saudi_profile(chrome_major);
        info!("Session {} using profile: {} (full ver: {})", session_id, profile, chrome_full_ver);

        builder = builder
            // WebRTC IP leak prevention
            .arg("disable-webrtc")
            .arg("disable-webrtc-hw-encoding")
            .arg("disable-webrtc-hw-decoding")
            .arg("disable-webrtc-encryption")
            .arg("disable-webrtc-hw-vp8-encoding")
            .arg("disable-webrtc-multiple-routes")
            .arg("disable-webrtc-hw-vp9-encoding")
            .arg("enforce-webrtc-ip-permission-check")
            .arg(("force-webrtc-ip-handling-policy", "disable_non_proxied_udp"))

            // Geolocation: disable default, we override via CDP with Riyadh coordinates
            .arg("disable-geolocation");

        // Set up local proxy forwarder if proxy is configured
        // This handles proxy authentication transparently for Chrome
        let mut proxy_forwarder: Option<LocalProxyForwarder> = None;

        if let Some(ref proxy_url) = config.proxy {
            // Parse proxy URL to get upstream details
            if let Some((upstream_host, upstream_port, username, password)) = Self::parse_proxy_for_forwarder(proxy_url) {
                info!("Session {} setting up local proxy forwarder to {}:{}", session_id, upstream_host, upstream_port);

                // Create and start local proxy forwarder
                let mut forwarder = LocalProxyForwarder::with_auto_port(
                    &upstream_host,
                    upstream_port,
                    &username,
                    &password,
                );

                forwarder.start().await
                    .map_err(|e| BrowserError::LaunchFailed(format!("Failed to start proxy forwarder: {}", e)))?;

                // Use local proxy URL for Chrome (no auth needed!)
                let local_proxy = forwarder.local_url();
                info!("Session {} using local proxy: {}", session_id, local_proxy);
                builder = builder.arg(("proxy-server", local_proxy.as_str()));

                proxy_forwarder = Some(forwarder);
            } else {
                // Fallback: use proxy URL directly (for proxies without auth)
                let (chrome_proxy, _) = Self::parse_proxy_url(proxy_url);
                info!("Session {} using direct proxy: {}", session_id, chrome_proxy);
                builder = builder.arg(("proxy-server", chrome_proxy.as_str()));
            }

            // Bypass proxy for non-essential Google services that Oxylabs blocks/throttles
            // These cause 403 (restricted target) and 522 (timeout) errors
            // They are background Chrome services not needed for search/ad clicking
            builder = builder.arg(("proxy-bypass-list",
                "mtalk.google.com;\
                 alt1-mtalk.google.com;\
                 alt2-mtalk.google.com;\
                 alt3-mtalk.google.com;\
                 alt4-mtalk.google.com;\
                 alt5-mtalk.google.com;\
                 alt6-mtalk.google.com;\
                 alt7-mtalk.google.com;\
                 alt8-mtalk.google.com;\
                 optimizationguide-pa.googleapis.com;\
                 content-autofill.googleapis.com;\
                 clientservices.googleapis.com;\
                 update.googleapis.com;\
                 safebrowsing.googleapis.com;\
                 accounts.google.com;\
                 clients1.google.com;\
                 clients2.google.com;\
                 clients3.google.com;\
                 clients4.google.com;\
                 clients5.google.com;\
                 clients6.google.com"
            ));
        }

        // Set window size
        builder = builder.window_size(config.window_width, config.window_height);

        let browser_config = builder.build()
            .map_err(|e| BrowserError::LaunchFailed(e.to_string()))?;

        // Launch browser
        let (browser, mut handler) = Browser::launch(browser_config)
            .await
            .map_err(|e| BrowserError::LaunchFailed(e.to_string()))?;

        // Spawn handler in background — when handler ends, Chrome has disconnected
        let session_id_clone = session_id.clone();
        let alive_flag = Arc::new(AtomicBool::new(true));
        let alive_for_handler = alive_flag.clone();
        tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                debug!("Session {} browser event: {:?}", session_id_clone, event);
            }
            // Handler ended = Chrome disconnected or crashed
            warn!("Session {} Chrome disconnected (event handler ended)", session_id_clone);
            alive_for_handler.store(false, Ordering::Relaxed);
        });

        // Get existing page or create new one (Chrome opens with a blank tab)
        // Close any extra tabs to avoid blank tab issue
        let page = {
            let mut pages = browser.pages().await
                .map_err(|e| BrowserError::LaunchFailed(e.to_string()))?;

            // Take the first page as our main page
            let main_page = if !pages.is_empty() {
                pages.remove(0)
            } else {
                // Fallback: create a new page if none exists
                browser.new_page("about:blank")
                    .await
                    .map_err(|e| BrowserError::LaunchFailed(e.to_string()))?
            };

            // Close any extra blank tabs
            for extra_page in pages {
                debug!("Closing extra blank tab");
                let _ = extra_page.close().await;
            }

            main_page
        };

        // Wrap page in ChaserPage for protocol-level stealth
        let chaser = ChaserPage::new(page);

        // === ZERO JavaScript overrides approach ===
        // ChaserProfile's apply_profile() injects a bootstrap_script that modifies
        // JavaScript prototypes (navigator.platform, webdriver, userAgentData, etc.)
        // These JS-level modifications are DETECTABLE by Google's bot detection.
        // Instead, we use ONLY CDP-level overrides which work at the browser engine
        // level and are invisible to JavaScript inspection.
        //
        // DISABLED: chaser.apply_profile(&profile) — no JS bootstrap
        // DISABLED: inject_extra_evasions() — no JS prototype modifications
        //
        // navigator.webdriver is handled by Chrome flag: --disable-blink-features=AutomationControlled
        // This sets it to false at the C++ level, not JavaScript level.

        // 1) CDP User-Agent + Metadata (sets UA string, Sec-CH-UA headers, Accept-Language)
        Self::set_cdp_headers(chaser.raw_page(), &profile, &chrome_full_ver).await?;

        // 2) CDP Timezone Override (sets Date.getTimezoneOffset and Intl.DateTimeFormat natively)
        Self::set_timezone_override(chaser.raw_page()).await?;

        // Override geolocation to Riyadh (24.7136°N, 46.6753°E) via CDP
        Self::set_geolocation_riyadh(chaser.raw_page()).await?;

        // 4) Pre-set Google cookies to look like a returning user (not a fresh bot)
        Self::pre_set_google_cookies(chaser.raw_page()).await?;

        // Block unnecessary resources to reduce proxy bandwidth consumption
        Self::block_unnecessary_resources(chaser.raw_page()).await?;

        info!("Browser session {} created (CDP-only, zero JS overrides, Chrome {})", session_id, chrome_full_ver);

        Ok(Self {
            id: session_id,
            data_dir_id,
            browser: Arc::new(RwLock::new(Some(browser))),
            page: Arc::new(RwLock::new(Some(chaser))),
            profile,
            config,
            alive: alive_flag,
            current_ip: Arc::new(RwLock::new(None)),
            previous_ip: Arc::new(RwLock::new(None)),
            ip_change_count: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            click_count: Arc::new(AtomicU64::new(0)),
            error_count: Arc::new(AtomicU64::new(0)),
            cycle_count: Arc::new(AtomicU64::new(0)),
            captcha_count: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            proxy_forwarder: Arc::new(RwLock::new(proxy_forwarder)),
        })
    }

    /// Get session ID
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Check if the session is alive
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    /// Get click count
    pub fn click_count(&self) -> u64 {
        self.click_count.load(Ordering::Relaxed)
    }

    /// Get error count
    pub fn error_count(&self) -> u64 {
        self.error_count.load(Ordering::Relaxed)
    }

    /// Increment click count
    pub fn increment_clicks(&self) {
        self.click_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment error count
    pub fn increment_errors(&self) {
        self.error_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Get cycle count
    pub fn cycle_count(&self) -> u64 {
        self.cycle_count.load(Ordering::Relaxed)
    }

    /// Increment cycle count
    pub fn increment_cycles(&self) {
        self.cycle_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Get CAPTCHA count
    pub fn captcha_count(&self) -> u32 {
        self.captcha_count.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Increment CAPTCHA count
    pub fn increment_captchas(&self) {
        self.captcha_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// Get current IP (if detected)
    pub async fn current_ip(&self) -> Option<String> {
        self.current_ip.read().await.clone()
    }

    /// Get previous IP (before last change)
    pub async fn previous_ip(&self) -> Option<String> {
        self.previous_ip.read().await.clone()
    }

    /// Get IP change count
    pub fn ip_change_count(&self) -> u32 {
        self.ip_change_count.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Set current IP and track changes
    pub async fn set_current_ip(&self, ip: String) {
        let mut current = self.current_ip.write().await;

        // Check if IP changed
        if let Some(ref old_ip) = *current {
            if old_ip != &ip {
                // IP changed! Update previous and increment counter
                *self.previous_ip.write().await = Some(old_ip.clone());
                self.ip_change_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                info!("Session {} IP changed: {} -> {} (change #{})",
                    self.id, old_ip, ip, self.ip_change_count());
            }
        }

        *current = Some(ip);
    }

    /// Navigate to a URL
    pub async fn navigate(&self, url: &str) -> Result<(), BrowserError> {
        let chaser = self.page.read().await;
        let chaser = chaser.as_ref().ok_or(BrowserError::ConnectionLost("No active page".into()))?;

        debug!("Session {} navigating to: {}", self.id, url);
        chaser.goto(url)
            .await
            .map_err(|e| BrowserError::NavigationFailed(e.to_string()))?;

        Ok(())
    }

    /// Wait for navigation to complete
    pub async fn wait_for_navigation(&self, timeout_secs: u64) -> Result<(), BrowserError> {
        let chaser = self.page.read().await;
        let chaser = chaser.as_ref().ok_or(BrowserError::ConnectionLost("No active page".into()))?;

        tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            chaser.raw_page().wait_for_navigation()
        )
        .await
        .map_err(|_| BrowserError::Timeout("Navigation timeout".into()))?
        .map_err(|e| BrowserError::NavigationFailed(e.to_string()))?;

        Ok(())
    }

    /// Execute JavaScript on the page with default 60 second timeout
    /// Uses stealth evaluation (isolated world) to avoid detection
    pub async fn execute_js(&self, script: &str) -> Result<serde_json::Value, BrowserError> {
        self.execute_js_with_timeout(script, 60).await
    }

    /// Execute JavaScript on the page with custom timeout (in seconds)
    /// Uses stealth evaluation via ChaserPage (isolated world, no Runtime.enable)
    pub async fn execute_js_with_timeout(&self, script: &str, timeout_secs: u64) -> Result<serde_json::Value, BrowserError> {
        let chaser = self.page.read().await;
        let chaser = chaser.as_ref().ok_or(BrowserError::ConnectionLost("No active page".into()))?;

        let result = tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            chaser.evaluate_stealth(script)
        )
        .await
        .map_err(|_| BrowserError::Timeout(format!("JavaScript execution timed out after {}s", timeout_secs)))?
        .map_err(|e| BrowserError::JavaScriptError(e.to_string()))?;

        // evaluate_stealth returns Option<Value>
        Ok(result.unwrap_or(serde_json::Value::Null))
    }

    /// Get current URL
    pub async fn get_current_url(&self) -> Result<String, BrowserError> {
        let chaser = self.page.read().await;
        let chaser = chaser.as_ref().ok_or(BrowserError::ConnectionLost("No active page".into()))?;

        chaser.url()
            .await
            .map_err(|e| BrowserError::ConnectionLost(e.to_string()))?
            .ok_or_else(|| BrowserError::ConnectionLost("No URL".into()))
    }

    /// Click on an element by selector
    pub async fn click(&self, selector: &str) -> Result<(), BrowserError> {
        let chaser = self.page.read().await;
        let chaser = chaser.as_ref().ok_or(BrowserError::ConnectionLost("No active page".into()))?;

        let element = chaser.raw_page().find_element(selector)
            .await
            .map_err(|e| BrowserError::ElementNotFound(format!("{}: {}", selector, e)))?;

        element.click()
            .await
            .map_err(|e| BrowserError::JavaScriptError(e.to_string()))?;

        self.increment_clicks();
        Ok(())
    }

    /// Type text into an element using human-like typing via ChaserPage
    pub async fn type_text(&self, selector: &str, text: &str) -> Result<(), BrowserError> {
        let chaser = self.page.read().await;
        let chaser = chaser.as_ref().ok_or(BrowserError::ConnectionLost("No active page".into()))?;

        // Click the element first
        let element = chaser.raw_page().find_element(selector)
            .await
            .map_err(|e| BrowserError::ElementNotFound(format!("{}: {}", selector, e)))?;
        element.click().await.ok();

        // Type with human-like delays using ChaserPage
        chaser.type_text(text)
            .await
            .map_err(|e| BrowserError::JavaScriptError(e.to_string()))?;

        Ok(())
    }

    /// Type text into currently focused element using raw CDP keyboard events (Send-safe)
    /// Uses Input.dispatchKeyEvent directly, bypassing chaser-oxide's !Send methods
    pub async fn type_text_cdp(&self, text: &str) -> Result<(), BrowserError> {
        use chaser_oxide::cdp::browser_protocol::input::{DispatchKeyEventParams, DispatchKeyEventType};
        use rand::SeedableRng;

        let chaser = self.page.read().await;
        let chaser = chaser.as_ref().ok_or(BrowserError::ConnectionLost("No active page".into()))?;
        let page = chaser.raw_page();

        let mut rng = rand::rngs::StdRng::from_entropy();

        for c in text.chars() {
            // Send keyDown with the character text
            let key_down = DispatchKeyEventParams::builder()
                .r#type(DispatchKeyEventType::KeyDown)
                .text(c.to_string())
                .build()
                .unwrap();
            page.execute(key_down)
                .await
                .map_err(|e| BrowserError::JavaScriptError(format!("CDP keyDown failed: {}", e)))?;

            // Send keyUp
            let key_up = DispatchKeyEventParams::builder()
                .r#type(DispatchKeyEventType::KeyUp)
                .build()
                .unwrap();
            page.execute(key_up)
                .await
                .map_err(|e| BrowserError::JavaScriptError(format!("CDP keyUp failed: {}", e)))?;

            // Human-like delay between keystrokes (50-150ms)
            let delay = rng.gen_range(50..150);
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }

        Ok(())
    }

    /// Press Enter key via raw CDP (Send-safe)
    pub async fn press_enter(&self) -> Result<(), BrowserError> {
        use chaser_oxide::cdp::browser_protocol::input::{DispatchKeyEventParams, DispatchKeyEventType};
        use rand::SeedableRng;

        let chaser = self.page.read().await;
        let chaser = chaser.as_ref().ok_or(BrowserError::ConnectionLost("No active page".into()))?;
        let page = chaser.raw_page();

        // Small random delay before pressing (100-300ms)
        let mut rng = rand::rngs::StdRng::from_entropy();
        let delay = rng.gen_range(100..300);
        tokio::time::sleep(Duration::from_millis(delay)).await;

        // rawKeyDown Enter (with full key properties for proper form submission)
        let key_down = DispatchKeyEventParams::builder()
            .r#type(DispatchKeyEventType::RawKeyDown)
            .key("Enter")
            .code("Enter")
            .windows_virtual_key_code(13)
            .native_virtual_key_code(13)
            .build()
            .unwrap();
        page.execute(key_down)
            .await
            .map_err(|e| BrowserError::JavaScriptError(format!("CDP Enter keyDown failed: {}", e)))?;

        // char event with \r (triggers form submission in most browsers)
        let char_event = DispatchKeyEventParams::builder()
            .r#type(DispatchKeyEventType::Char)
            .text("\r")
            .build()
            .unwrap();
        page.execute(char_event)
            .await
            .map_err(|e| BrowserError::JavaScriptError(format!("CDP Enter char failed: {}", e)))?;

        // keyUp Enter
        let key_up = DispatchKeyEventParams::builder()
            .r#type(DispatchKeyEventType::KeyUp)
            .key("Enter")
            .code("Enter")
            .windows_virtual_key_code(13)
            .native_virtual_key_code(13)
            .build()
            .unwrap();
        page.execute(key_up)
            .await
            .map_err(|e| BrowserError::JavaScriptError(format!("CDP Enter keyUp failed: {}", e)))?;

        Ok(())
    }

    /// Scroll the page using CDP mouse wheel events (Send-safe)
    pub async fn scroll_human(&self, delta_y: i32) -> Result<(), BrowserError> {
        use chaser_oxide::cdp::browser_protocol::input::{
            DispatchMouseEventParams, DispatchMouseEventType, MouseButton,
        };
        use rand::SeedableRng;

        let chaser = self.page.read().await;
        let chaser = chaser.as_ref().ok_or(BrowserError::ConnectionLost("No active page".into()))?;
        let page = chaser.raw_page();

        let mut rng = rand::rngs::StdRng::from_entropy();
        let steps = 3 + rng.gen_range(0..3);
        let per_step = delta_y / steps;

        for _ in 0..steps {
            let jitter = rng.gen_range(-20..20);
            let scroll = DispatchMouseEventParams::builder()
                .r#type(DispatchMouseEventType::MouseWheel)
                .x(400.0)
                .y(300.0)
                .button(MouseButton::None)
                .delta_x(0.0)
                .delta_y((per_step + jitter) as f64)
                .build()
                .unwrap();
            page.execute(scroll)
                .await
                .map_err(|e| BrowserError::JavaScriptError(format!("CDP scroll failed: {}", e)))?;

            let delay = rng.gen_range(80..200);
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }

        Ok(())
    }

    /// Move mouse with physics-based bezier curve (Send-safe)
    /// Simulates natural mouse movement with overshoot and easing
    pub async fn move_mouse_human(&self, target_x: f64, target_y: f64) -> Result<(), BrowserError> {
        use chaser_oxide::cdp::browser_protocol::input::{
            DispatchMouseEventParams, DispatchMouseEventType, MouseButton,
        };
        use rand::SeedableRng;

        let chaser = self.page.read().await;
        let chaser = chaser.as_ref().ok_or(BrowserError::ConnectionLost("No active page".into()))?;
        let page = chaser.raw_page();

        let mut rng = rand::rngs::StdRng::from_entropy();

        // Start from a random position (simulates existing cursor)
        let start_x: f64 = rng.gen_range(100.0..800.0);
        let start_y: f64 = rng.gen_range(100.0..500.0);

        // Generate bezier control points with slight overshoot
        let overshoot = rng.gen_range(0.0..15.0);
        let cp1_x = start_x + (target_x - start_x) * 0.25 + rng.gen_range(-50.0..50.0);
        let cp1_y = start_y + (target_y - start_y) * 0.25 + rng.gen_range(-40.0..40.0);
        let cp2_x = target_x + overshoot * rng.gen_range(-1.0..1.0);
        let cp2_y = target_y + overshoot * rng.gen_range(-1.0..1.0);

        // Number of steps based on distance (more steps = smoother)
        let distance = ((target_x - start_x).powi(2) + (target_y - start_y).powi(2)).sqrt();
        let steps = (15.0 + distance / 30.0).min(40.0) as i32;

        for i in 0..=steps {
            let t = i as f64 / steps as f64;
            let mt = 1.0 - t;

            // Cubic bezier
            let x = mt.powi(3) * start_x
                + 3.0 * mt.powi(2) * t * cp1_x
                + 3.0 * mt * t.powi(2) * cp2_x
                + t.powi(3) * target_x;
            let y = mt.powi(3) * start_y
                + 3.0 * mt.powi(2) * t * cp1_y
                + 3.0 * mt * t.powi(2) * cp2_y
                + t.powi(3) * target_y;

            let move_event = DispatchMouseEventParams::builder()
                .r#type(DispatchMouseEventType::MouseMoved)
                .x(x)
                .y(y)
                .button(MouseButton::None)
                .build()
                .unwrap();
            page.execute(move_event).await.ok();

            // Variable delay: faster in middle, slower at start/end (ease in/out)
            let speed_factor = 1.0 - (2.0 * t - 1.0).abs(); // peak at t=0.5
            let delay = (8.0 + 12.0 * (1.0 - speed_factor) + rng.gen_range(0.0..5.0)) as u64;
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }

        Ok(())
    }

    /// Click at coordinates with human-like mouse movement first (Send-safe)
    pub async fn click_human_at(&self, x: f64, y: f64) -> Result<(), BrowserError> {
        use chaser_oxide::cdp::browser_protocol::input::{
            DispatchMouseEventParams, DispatchMouseEventType, MouseButton,
        };
        use rand::SeedableRng;

        // Move mouse to target first
        self.move_mouse_human(x, y).await?;

        let chaser = self.page.read().await;
        let chaser = chaser.as_ref().ok_or(BrowserError::ConnectionLost("No active page".into()))?;
        let page = chaser.raw_page();

        let mut rng = rand::rngs::StdRng::from_entropy();

        // Small jitter on click position (humans don't click pixel-perfect)
        let click_x = x + rng.gen_range(-2.0..2.0);
        let click_y = y + rng.gen_range(-2.0..2.0);

        // Brief pause before clicking (50-150ms)
        let pre_click = rng.gen_range(50..150);
        tokio::time::sleep(Duration::from_millis(pre_click)).await;

        // Mouse down
        let mouse_down = DispatchMouseEventParams::builder()
            .r#type(DispatchMouseEventType::MousePressed)
            .x(click_x)
            .y(click_y)
            .button(MouseButton::Left)
            .click_count(1)
            .build()
            .unwrap();
        page.execute(mouse_down).await
            .map_err(|e| BrowserError::JavaScriptError(format!("CDP mouseDown failed: {}", e)))?;

        // Hold duration (40-120ms like real clicks)
        let hold = rng.gen_range(40..120);
        tokio::time::sleep(Duration::from_millis(hold)).await;

        // Mouse up
        let mouse_up = DispatchMouseEventParams::builder()
            .r#type(DispatchMouseEventType::MouseReleased)
            .x(click_x)
            .y(click_y)
            .button(MouseButton::Left)
            .click_count(1)
            .build()
            .unwrap();
        page.execute(mouse_up).await
            .map_err(|e| BrowserError::JavaScriptError(format!("CDP mouseUp failed: {}", e)))?;

        self.increment_clicks();
        Ok(())
    }

    /// Type text with occasional typos and corrections (Send-safe)
    /// Simulates realistic typing with variable speed, pauses, and typo corrections
    pub async fn type_text_with_typos_cdp(&self, text: &str) -> Result<(), BrowserError> {
        use chaser_oxide::cdp::browser_protocol::input::{DispatchKeyEventParams, DispatchKeyEventType};
        use rand::SeedableRng;

        let chaser = self.page.read().await;
        let chaser = chaser.as_ref().ok_or(BrowserError::ConnectionLost("No active page".into()))?;
        let page = chaser.raw_page();

        let mut rng = rand::rngs::StdRng::from_entropy();
        let chars: Vec<char> = text.chars().collect();

        for (i, &c) in chars.iter().enumerate() {
            // 3% chance of typo (type wrong char, pause, backspace, retype)
            if rng.gen_bool(0.03) && i > 0 {
                // Type a wrong character (nearby key or random)
                let typo_char = if c.is_ascii_alphabetic() {
                    // Pick a random nearby letter
                    let offset: i32 = if rng.gen_bool(0.5) { 1 } else { -1 };
                    let typo = ((c as i32) + offset) as u8 as char;
                    if typo.is_ascii_alphabetic() { typo } else { c }
                } else {
                    c // Don't typo non-alpha chars
                };

                if typo_char != c {
                    // Type wrong char
                    let wrong = DispatchKeyEventParams::builder()
                        .r#type(DispatchKeyEventType::KeyDown)
                        .text(typo_char.to_string())
                        .build().unwrap();
                    page.execute(wrong).await.ok();
                    let up = DispatchKeyEventParams::builder()
                        .r#type(DispatchKeyEventType::KeyUp)
                        .build().unwrap();
                    page.execute(up).await.ok();

                    // Pause (noticing the mistake: 200-500ms)
                    tokio::time::sleep(Duration::from_millis(rng.gen_range(200..500))).await;

                    // Backspace
                    let bs_down = DispatchKeyEventParams::builder()
                        .r#type(DispatchKeyEventType::RawKeyDown)
                        .key("Backspace").code("Backspace")
                        .windows_virtual_key_code(8)
                        .build().unwrap();
                    page.execute(bs_down).await.ok();
                    let bs_up = DispatchKeyEventParams::builder()
                        .r#type(DispatchKeyEventType::KeyUp)
                        .key("Backspace").code("Backspace")
                        .build().unwrap();
                    page.execute(bs_up).await.ok();

                    // Brief pause after correction (100-250ms)
                    tokio::time::sleep(Duration::from_millis(rng.gen_range(100..250))).await;
                }
            }

            // Type the correct character
            let key_down = DispatchKeyEventParams::builder()
                .r#type(DispatchKeyEventType::KeyDown)
                .text(c.to_string())
                .build().unwrap();
            page.execute(key_down).await
                .map_err(|e| BrowserError::JavaScriptError(format!("CDP keyDown failed: {}", e)))?;

            let key_up = DispatchKeyEventParams::builder()
                .r#type(DispatchKeyEventType::KeyUp)
                .build().unwrap();
            page.execute(key_up).await
                .map_err(|e| BrowserError::JavaScriptError(format!("CDP keyUp failed: {}", e)))?;

            // Variable delay between keystrokes
            let base_delay = rng.gen_range(60..180); // slower than before (was 50-150)
            // 8% chance of a longer "thinking" pause between words
            let delay = if c == ' ' || rng.gen_bool(0.08) {
                rng.gen_range(200..500)
            } else {
                base_delay
            };
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }

        Ok(())
    }

    /// Detect current IP address via external service
    pub async fn detect_ip(&self) -> Result<String, BrowserError> {
        self.navigate("https://api.ipify.org?format=json").await?;

        // Wait a bit for page to load
        tokio::time::sleep(Duration::from_millis(500)).await;

        let result = self.execute_js("document.body.innerText").await?;

        if let Some(text) = result.as_str() {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(text) {
                if let Some(ip) = json.get("ip").and_then(|v| v.as_str()) {
                    self.set_current_ip(ip.to_string()).await;
                    return Ok(ip.to_string());
                }
            }
        }

        Err(BrowserError::JavaScriptError("Could not detect IP".into()))
    }

    /// Close the browser session
    pub async fn close(&self) -> Result<(), BrowserError> {
        // Mark as not alive first to prevent new operations
        self.alive.store(false, Ordering::Relaxed);

        // 1. Close page first (stops navigation/JS execution)
        {
            let mut chaser = self.page.write().await;
            if let Some(c) = chaser.take() {
                let _ = c.raw_page().clone().close().await;
            }
        }

        // 2. Close browser - try graceful close, give it a moment, then force kill
        {
            let mut browser = self.browser.write().await;
            if let Some(mut b) = browser.take() {
                // Try graceful close first (sends Browser.close CDP command)
                let _ = b.close().await;
                // Brief grace period for Chrome child processes to exit
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                // Force kill to ensure all Chrome processes are terminated (fixes Windows zombie issue)
                let _ = b.kill().await;
            }
        }

        // 3. Stop local proxy forwarder after browser is dead
        {
            let mut forwarder = self.proxy_forwarder.write().await;
            if let Some(mut f) = forwarder.take() {
                f.stop().await;
            }
        }

        info!("Browser session {} closed", self.id);
        Ok(())
    }

    /// Parse proxy URL to extract Chrome-compatible format and auth credentials
    ///
    /// Chrome's --proxy-server format (no inline auth supported in modern Chrome):
    /// - HTTP proxies: http://host:port or just host:port
    /// - SOCKS proxies: socks5://host:port
    ///
    /// Returns: (chrome_proxy_url, Option<(username, password)>)
    fn parse_proxy_url(proxy_url: &str) -> (String, Option<(String, String)>) {
        // Try to parse as URL
        if let Ok(url) = url::Url::parse(proxy_url) {
            let scheme = match url.scheme() {
                "socks5h" | "socks5" => "socks5",
                "http" | "https" => "http",
                other => other,
            };

            let host = url.host_str().unwrap_or("localhost");
            let port = url.port().unwrap_or(match scheme {
                "socks5" => 1080,
                "http" => 80,
                "https" => 443,
                _ => 1080,
            });

            // Extract auth if present
            let auth = if !url.username().is_empty() {
                let username = urlencoding::decode(url.username())
                    .unwrap_or_else(|_| url.username().into())
                    .to_string();
                let password = url.password()
                    .map(|p| urlencoding::decode(p).unwrap_or_else(|_| p.into()).to_string())
                    .unwrap_or_default();
                Some((username, password))
            } else {
                None
            };

            // Chrome proxy format - NO inline auth (not supported in modern Chrome)
            // Use http://host:port or socks5://host:port
            let chrome_proxy = format!("{}://{}:{}", scheme, host, port);

            (chrome_proxy, auth)
        } else {
            // Fallback: return as-is with no auth
            (proxy_url.to_string(), None)
        }
    }

    /// Parse proxy URL to extract components for local proxy forwarder
    ///
    /// Returns: Option<(host, port, username, password)>
    /// Returns None if the proxy URL doesn't have authentication credentials
    fn parse_proxy_for_forwarder(proxy_url: &str) -> Option<(String, u16, String, String)> {
        if let Ok(url) = url::Url::parse(proxy_url) {
            // Only use forwarder if credentials are present
            if url.username().is_empty() {
                return None;
            }

            let host = url.host_str()?.to_string();
            let port = url.port().unwrap_or(match url.scheme() {
                "socks5h" | "socks5" => 7777,
                "http" | "https" => 60000,
                _ => 60000,
            });

            let username = urlencoding::decode(url.username())
                .unwrap_or_else(|_| url.username().into())
                .to_string();

            let password = url.password()
                .map(|p| urlencoding::decode(p).unwrap_or_else(|_| p.into()).to_string())
                .unwrap_or_default();

            info!("Parsed proxy credentials - host: {}, port: {}, user: {}..., pass_len: {}",
                   host, port, crate::safe_truncate(&username, 30), password.len());

            Some((host, port, username, password))
        } else {
            None
        }
    }

    /// Set full CDP headers to ensure Sec-CH-UA-Platform and Accept-Language match the profile.
    /// ChaserPage::apply_profile() only sets user_agent string — it does NOT set metadata.
    /// Without this, Chrome sends the REAL OS in Sec-CH-UA-Platform headers.
    ///
    /// IMPORTANT: Brand string must use "Not=A?Brand" to match ChaserProfile's bootstrap_script
    /// (profiles.rs line 292). Any mismatch between JS-level userAgentData.brands and
    /// HTTP-level Sec-CH-UA headers is a detection vector.
    ///
    /// NOTE: We do NOT set Sec-CH-UA via SetExtraHttpHeaders because
    /// SetUserAgentOverrideParams with user_agent_metadata already handles all
    /// Sec-CH-UA-* headers. Double-setting them causes conflicts.
    async fn set_cdp_headers(page: &Page, profile: &ChaserProfile, full_version: &str) -> Result<(), BrowserError> {
        use chaser_oxide::cdp::browser_protocol::emulation::{
            SetUserAgentOverrideParams, UserAgentMetadata, UserAgentBrandVersion,
        };
        use chaser_oxide::cdp::browser_protocol::network::{
            SetExtraHttpHeadersParams, Headers,
        };

        let major = profile.chrome_version().to_string();

        // Build the REAL User-Agent with full version (not .0.0.0 which ChaserProfile uses)
        // Real Chrome: "Chrome/142.0.7444.175" not "Chrome/142.0.0.0"
        let real_ua = format!(
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{} Safari/537.36",
            full_version
        );

        // 1) SetUserAgentOverride with FULL metadata (platform, brands, accept-language)
        // Brand string "Not=A?Brand" matches ChaserProfile bootstrap_script exactly
        let ua_params = SetUserAgentOverrideParams {
            user_agent: real_ua,
            accept_language: Some("ar-SA,ar;q=0.9,en-US;q=0.8,en;q=0.7".to_string()),
            platform: Some("Linux x86_64".to_string()),
            user_agent_metadata: Some(UserAgentMetadata {
                brands: Some(vec![
                    UserAgentBrandVersion::new("Google Chrome", &major),
                    UserAgentBrandVersion::new("Chromium", &major),
                    UserAgentBrandVersion::new("Not=A?Brand", "24"),
                ]),
                full_version_list: Some(vec![
                    UserAgentBrandVersion::new("Google Chrome", full_version),
                    UserAgentBrandVersion::new("Chromium", full_version),
                    UserAgentBrandVersion::new("Not=A?Brand", "24.0.0.0"),
                ]),
                platform: "Linux".to_string(),
                platform_version: "6.1.0".to_string(),
                architecture: "x86".to_string(),
                model: String::new(),
                mobile: false,
                bitness: Some("64".to_string()),
                wow64: Some(false),
                form_factors: None,
            }),
        };

        page.execute(ua_params)
            .await
            .map_err(|e| BrowserError::LaunchFailed(format!("Failed to set UA override: {}", e)))?;

        // 2) Only set Accept-Language via SetExtraHttpHeaders.
        // Sec-CH-UA headers are handled by user_agent_metadata above.
        // Setting them in BOTH places causes double-header conflicts.
        let headers_json = serde_json::json!({
            "Accept-Language": "ar-SA,ar;q=0.9,en-US;q=0.8,en;q=0.7"
        });

        let extra_headers = SetExtraHttpHeadersParams::new(Headers::new(headers_json));
        page.execute(extra_headers)
            .await
            .map_err(|e| BrowserError::LaunchFailed(format!("Failed to set extra headers: {}", e)))?;

        debug!("CDP headers set: Chrome/{}, Platform=Linux, Accept-Language=ar-SA", full_version);
        Ok(())
    }

    /// Set timezone to Asia/Riyadh via CDP Emulation (no JavaScript modification).
    /// This makes Date.getTimezoneOffset() return -180 and
    /// Intl.DateTimeFormat().resolvedOptions().timeZone return "Asia/Riyadh"
    /// at the browser engine level — completely invisible to detection scripts.
    async fn set_timezone_override(page: &Page) -> Result<(), BrowserError> {
        use chaser_oxide::cdp::browser_protocol::emulation::SetTimezoneOverrideParams;

        let tz_params = SetTimezoneOverrideParams::new("Asia/Riyadh");
        page.execute(tz_params)
            .await
            .map_err(|e| BrowserError::LaunchFailed(format!("Failed to set timezone override: {}", e)))?;

        debug!("CDP timezone override set: Asia/Riyadh (UTC+3, offset -180)");
        Ok(())
    }

    /// Inject additional anti-detection evasions via AddScriptToEvaluateOnNewDocument.
    /// This persists across navigations and does NOT trigger Runtime.enable.
    /// ChaserProfile bootstrap covers ~12 evasions; we add ~25 more here.
    ///
    /// Also injects a dynamic script to patch navigator.userAgentData.getHighEntropyValues(),
    /// which ChaserProfile's bootstrap destroys by replacing userAgentData with a plain object.
    /// Google calls getHighEntropyValues() to get full version + platform details.
    async fn inject_extra_evasions(page: &Page, chrome_major: u32, full_version: &str) -> Result<(), BrowserError> {
        use chaser_oxide::cdp::browser_protocol::page::AddScriptToEvaluateOnNewDocumentParams;

        // 1) Static evasions (28 sections covering fingerprint surfaces)
        let evasion_script = include_str!("../../evasions/extra_evasions.js");

        let params = AddScriptToEvaluateOnNewDocumentParams {
            source: evasion_script.to_string(),
            world_name: None,
            include_command_line_api: None,
            run_immediately: None,
        };

        page.execute(params)
            .await
            .map_err(|e| BrowserError::LaunchFailed(format!("Failed to inject extra evasions: {}", e)))?;

        // 2) Dynamic script: Fix navigator.userAgentData after ChaserProfile's bootstrap
        //
        // ChaserProfile's bootstrap replaces navigator.userAgentData with a plain object
        // that only has {brands, mobile, platform}. Real Chrome's NavigatorUAData also has
        // getHighEntropyValues() and toJSON(). Google uses getHighEntropyValues() to get
        // the full version, architecture, bitness, etc.
        //
        // We need runtime values (full_version) so this can't be in the static JS file.
        let uad_patch = format!(
            r#"(function() {{
                'use strict';
                try {{
                    const uad = navigator.userAgentData;
                    if (uad && !uad.getHighEntropyValues) {{
                        const fullVer = "{full_ver}";
                        const major = "{major}";
                        uad.getHighEntropyValues = function(hints) {{
                            const result = {{
                                brands: uad.brands,
                                mobile: uad.mobile,
                                platform: uad.platform,
                            }};
                            if (hints.includes('fullVersionList')) {{
                                result.fullVersionList = [
                                    {{ brand: "Google Chrome", version: fullVer }},
                                    {{ brand: "Chromium", version: fullVer }},
                                    {{ brand: "Not=A?Brand", version: "24.0.0.0" }}
                                ];
                            }}
                            if (hints.includes('platformVersion')) result.platformVersion = "6.1.0";
                            if (hints.includes('architecture')) result.architecture = "x86";
                            if (hints.includes('bitness')) result.bitness = "64";
                            if (hints.includes('model')) result.model = "";
                            if (hints.includes('uaFullVersion')) result.uaFullVersion = fullVer;
                            if (hints.includes('wow64')) result.wow64 = false;
                            if (hints.includes('formFactors')) result.formFactors = [];
                            return Promise.resolve(result);
                        }};
                        uad.toJSON = function() {{
                            return {{ brands: uad.brands, mobile: uad.mobile, platform: uad.platform }};
                        }};
                    }}
                }} catch(e) {{}}
            }})();"#,
            full_ver = full_version,
            major = chrome_major,
        );

        let uad_params = AddScriptToEvaluateOnNewDocumentParams {
            source: uad_patch,
            world_name: None,
            include_command_line_api: None,
            run_immediately: None,
        };

        page.execute(uad_params)
            .await
            .map_err(|e| BrowserError::LaunchFailed(format!("Failed to inject UAD patch: {}", e)))?;

        debug!("Extra anti-detection evasions injected (25+ evasions + UAD patch, Chrome {})", full_version);
        Ok(())
    }

    /// Override geolocation to Riyadh, Saudi Arabia via CDP
    async fn set_geolocation_riyadh(page: &Page) -> Result<(), BrowserError> {
        use chaser_oxide::cdp::browser_protocol::emulation::SetGeolocationOverrideParams;

        let params = SetGeolocationOverrideParams::builder()
            .latitude(24.7136)
            .longitude(46.6753)
            .accuracy(100.0)
            .build();

        page.execute(params)
            .await
            .map_err(|e| BrowserError::JavaScriptError(format!("Failed to set geolocation: {}", e)))?;

        info!("Geolocation overridden to Riyadh (24.7136, 46.6753)");
        Ok(())
    }

    /// Pre-set Google cookies via CDP to look like a returning user.
    /// A fresh browser with zero cookies is a strong bot signal to Google.
    /// Sets CONSENT (accepted cookies) and PREF (language/country) before any navigation.
    async fn pre_set_google_cookies(page: &Page) -> Result<(), BrowserError> {
        use chaser_oxide::cdp::browser_protocol::network::{SetCookiesParams, CookieParam};

        let cookies = vec![
            // CONSENT cookie — tells Google consent dialog was already accepted
            CookieParam::builder()
                .name("CONSENT")
                .value("PENDING+987")
                .domain(".google.com.sa")
                .path("/")
                .secure(true)
                .build()
                .map_err(|e| BrowserError::LaunchFailed(format!("Cookie build error: {}", e)))?,
            // Also set for .google.com (some redirects go through google.com)
            CookieParam::builder()
                .name("CONSENT")
                .value("PENDING+987")
                .domain(".google.com")
                .path("/")
                .secure(true)
                .build()
                .map_err(|e| BrowserError::LaunchFailed(format!("Cookie build error: {}", e)))?,
            // PREF cookie — sets language to Arabic and country to Saudi Arabia
            CookieParam::builder()
                .name("PREF")
                .value("hl=ar&gl=SA")
                .domain(".google.com.sa")
                .path("/")
                .secure(true)
                .build()
                .map_err(|e| BrowserError::LaunchFailed(format!("Cookie build error: {}", e)))?,
            CookieParam::builder()
                .name("PREF")
                .value("hl=ar&gl=SA")
                .domain(".google.com")
                .path("/")
                .secure(true)
                .build()
                .map_err(|e| BrowserError::LaunchFailed(format!("Cookie build error: {}", e)))?,
        ];

        page.execute(SetCookiesParams::new(cookies))
            .await
            .map_err(|e| BrowserError::LaunchFailed(format!("Failed to set Google cookies: {}", e)))?;

        info!("Pre-set Google cookies (CONSENT, PREF) for .google.com.sa and .google.com");
        Ok(())
    }

    // Anti-detection approach: ZERO JavaScript modifications
    //
    // Google detects JS prototype modifications (navigator overrides, toString patches, etc.)
    // Before chaser-oxide: CAPTCHAs were rare. After: CAPTCHAs on every search.
    // Root cause: bootstrap_script + extra_evasions.js modified ~37 JS prototypes.
    //
    // New approach — CDP-level only (invisible to JavaScript):
    // 1. Auto-detect Chrome version from installed binary (MUST match real browser)
    // 2. SetUserAgentOverrideParams — UA string, Accept-Language, platform, userAgentMetadata
    //    (handles navigator.userAgent, navigator.language, navigator.languages, Sec-CH-UA-*)
    // 3. SetTimezoneOverrideParams — Asia/Riyadh timezone at browser engine level
    //    (handles Date.getTimezoneOffset, Intl.DateTimeFormat natively)
    // 4. Emulation.setGeolocationOverride — Riyadh coordinates
    // 5. Chrome flag: --disable-blink-features=AutomationControlled
    //    (handles navigator.webdriver=false at C++ level)
    //
    // NO JavaScript overrides. NO prototype modifications. NO toString patches.
    // The browser runs as genuine Chrome/Chromium with its real fingerprint.

    /// Block unnecessary resources via CDP to reduce proxy bandwidth consumption
    ///
    /// This significantly reduces proxy data usage by blocking:
    /// - Analytics/tracking scripts (not needed for ad clicking)
    /// - Third-party tracking pixels
    /// - Font downloads (system fonts used instead)
    /// - Video embeds
    ///
    /// NOTE: Google Ads services are NOT blocked (googleadservices.com, googlesyndication.com)
    async fn block_unnecessary_resources(page: &Page) -> Result<(), BrowserError> {
        use chaser_oxide::cdp::browser_protocol::network::SetBlockedUrLsParams;

        let blocked_urls = vec![
            // Analytics & tracking (NOT related to Google Ads)
            "*.google-analytics.com/*".to_string(),
            "*collect?v=*".to_string(),                    // GA collect endpoint pattern
            "*.facebook.net/*".to_string(),
            "*.hotjar.com/*".to_string(),
            "*.hotjar.io/*".to_string(),
            "*.segment.io/*".to_string(),
            "*.segment.com/*".to_string(),
            "*.mixpanel.com/*".to_string(),
            "*.amplitude.com/*".to_string(),
            "*.clarity.ms/*".to_string(),
            "*.newrelic.com/*".to_string(),
            "*.sentry.io/*".to_string(),
            "*.mouseflow.com/*".to_string(),
            "*.crazyegg.com/*".to_string(),
            "*.fullstory.com/*".to_string(),
            "*.heap-analytics.com/*".to_string(),
            // Social media widgets & tracking
            "platform.twitter.com/*".to_string(),
            "connect.facebook.net/*".to_string(),
            "*.linkedin.com/li.lms-analytics/*".to_string(),
            // Ad networks (NOT Google Ads — those are needed for click tracking)
            // These cause proxy 522 timeouts during warm-up browsing
            "*.taboola.com/*".to_string(),
            "*.taboolasyndication.com/*".to_string(),
            "*.pubmatic.com/*".to_string(),
            "*.outbrain.com/*".to_string(),
            "*.criteo.com/*".to_string(),
            "*.criteo.net/*".to_string(),
            "*.adsrvr.org/*".to_string(),
            "*.moatads.com/*".to_string(),
            "*.rubiconproject.com/*".to_string(),
            "*.openx.net/*".to_string(),
            "*.bidswitch.net/*".to_string(),
            "*.casalemedia.com/*".to_string(),
            "*.amazon-adsystem.com/*".to_string(),
            "*.adnxs.com/*".to_string(),
            // Chrome background services (cause proxy 403/522 errors on Oxylabs)
            "*mtalk.google.com*".to_string(),
            "*optimizationguide-pa.googleapis.com*".to_string(),
            "*content-autofill.googleapis.com*".to_string(),
            "*clientservices.googleapis.com*".to_string(),
            "*update.googleapis.com*".to_string(),
            "*safebrowsing.googleapis.com*".to_string(),
            "*clients1.google.com*".to_string(),
            "*clients2.google.com*".to_string(),
            "*clients3.google.com*".to_string(),
            "*clients4.google.com*".to_string(),
            "*clients5.google.com*".to_string(),
            "*clients6.google.com*".to_string(),
            // Video embeds (heavy bandwidth)
            "*.youtube.com/embed/*".to_string(),
            "*.vimeo.com/*".to_string(),
            // Font services (saves ~200-500KB per page load)
            "fonts.googleapis.com/*".to_string(),
            "fonts.gstatic.com/*".to_string(),
            "use.typekit.net/*".to_string(),
            "*.fontawesome.com/*".to_string(),
        ];

        let params = SetBlockedUrLsParams::new(blocked_urls);
        page.execute(params)
            .await
            .map_err(|e| BrowserError::JavaScriptError(format!("Failed to block URLs: {}", e)))?;

        info!("Resource blocking enabled - analytics, tracking, fonts, video blocked (saves proxy bandwidth)");
        Ok(())
    }
}

impl Drop for BrowserSession {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Relaxed);
    }
}
