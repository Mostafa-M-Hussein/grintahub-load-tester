//! Browser session management
//!
//! Handles launching and controlling individual Chrome browser instances.
//! Uses a local proxy forwarder to handle authenticated upstream proxies.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{info, debug};
use chromiumoxide::{Browser, BrowserConfig, Page};
use futures::StreamExt;
use uuid::Uuid;
use rand::Rng;

use super::BrowserError;
use crate::proxy::LocalProxyForwarder;

/// Rotating user-agents pool - realistic Chrome/Edge on Windows
const USER_AGENTS: &[&str] = &[
    // Chrome on Windows (most common)
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36",
    // Edge on Windows
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 Edg/131.0.0.0",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36 Edg/130.0.0.0",
    // Chrome on macOS
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36",
    // Chrome on Linux
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36",
];

/// Get a random user-agent
fn get_random_user_agent() -> &'static str {
    let idx = rand::thread_rng().gen_range(0..USER_AGENTS.len());
    USER_AGENTS[idx]
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
}

/// A browser session for automation
pub struct BrowserSession {
    /// Unique session ID
    pub id: String,
    /// The browser instance
    browser: Arc<RwLock<Option<Browser>>>,
    /// Current active page
    page: Arc<RwLock<Option<Page>>>,
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
        let session_id = Uuid::new_v4().to_string()[..8].to_string();

        info!("Launching browser session {} (headless: {})", session_id, config.headless);

        // Build browser config
        let mut builder = BrowserConfig::builder();

        // Set headless mode
        if !config.headless {
            builder = builder.with_head();
        }

        // Set Chrome path if specified
        if let Some(ref path) = config.chrome_path {
            builder = builder.chrome_executable(path);
        }

        // Set user data directory
        if let Some(ref dir) = config.user_data_dir {
            // Create directory if it doesn't exist
            let _ = std::fs::create_dir_all(dir);
            builder = builder.user_data_dir(dir);
        }

        // =========== UNDETECTED-CHROMEDRIVER STYLE FLAGS ===========
        // Based on: https://github.com/ultrafunkamsterdam/undetected-chromedriver
        builder = builder
            // REAL BROWSER PROFILE (not incognito) - more realistic
            // Each session still gets unique user-data-dir via for_session()

            // UNDETECTED-CHROMEDRIVER: Core flags that bypass detection
            .arg("--disable-blink-features=AutomationControlled")
            .arg("--exclude-switches=enable-automation")
            .arg("--disable-automation")
            .arg("--disable-infobars")
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .arg("--no-sandbox")
            .arg("--disable-dev-shm-usage")

            // UNDETECTED-CHROMEDRIVER: Window size (reasonable size that fits most screens)
            .arg("--window-size=1366,768")
            .arg("--window-position=50,50")

            // UNDETECTED-CHROMEDRIVER: Disable automation flags
            .arg("--disable-blink-features=AutomationControlled")
            .arg("--disable-features=AutomationControlled")

            // Disable session restore (no "restore tabs" prompt)
            .arg("--disable-session-crashed-bubble")
            .arg("--disable-restore-session-state")

            // Start with blank page (prevents new tab page from loading)
            .arg("--homepage=about:blank")

            // REAL BROWSER: Keep some extensions for realism, disable others
            .arg("--disable-extensions-except=")
            .arg("--load-extension=")

            // Stealth settings
            .arg("--disable-features=IsolateOrigins,site-per-process")
            .arg("--disable-site-isolation-trials")
            .arg("--disable-features=TranslateUI")
            .arg("--disable-popup-blocking")
            .arg("--disable-notifications")
            .arg("--disable-save-password-bubble")
            .arg("--disable-translate")
            .arg("--disable-sync")
            .arg("--disable-background-networking")
            .arg("--disable-background-timer-throttling")
            .arg("--disable-backgrounding-occluded-windows")
            .arg("--disable-renderer-backgrounding")
            .arg("--disable-client-side-phishing-detection")
            .arg("--disable-default-apps")
            .arg("--disable-hang-monitor")
            .arg("--disable-prompt-on-repost")
            .arg("--disable-domain-reliability")
            .arg("--disable-component-update")
            .arg("--disable-features=AudioServiceOutOfProcess")

            // Performance
            .arg("--disable-ipc-flooding-protection")
            .arg("--enable-features=NetworkService,NetworkServiceInProcess");

        // ROTATING USER-AGENT: Pick random user-agent for each session
        let user_agent = get_random_user_agent();
        info!("Session {} using user-agent: {}", session_id, &user_agent[..50]);
        builder = builder.arg(format!("--user-agent={}", user_agent))

            // Language settings for Saudi Arabia
            .arg("--lang=ar-SA")
            .arg("--accept-lang=ar-SA,ar,en-US,en")

            // STEALTH: Disable WebRTC to prevent IP leak
            .arg("--disable-webrtc")
            .arg("--disable-webrtc-hw-encoding")
            .arg("--disable-webrtc-hw-decoding")
            .arg("--disable-webrtc-encryption")
            .arg("--disable-webrtc-hw-vp8-encoding")
            .arg("--disable-webrtc-multiple-routes")
            .arg("--disable-webrtc-hw-vp9-encoding")
            .arg("--enforce-webrtc-ip-permission-check")
            .arg("--force-webrtc-ip-handling-policy=disable_non_proxied_udp")

            // STEALTH: Timezone for Saudi Arabia (AST = UTC+3)
            .arg("--timezone=Asia/Riyadh")

            // STEALTH: Geolocation spoof (Riyadh)
            .arg("--disable-geolocation")

            // UNDETECTED-CHROMEDRIVER: DON'T disable canvas/WebGL (makes fingerprint unique but consistent)
            // Real browsers have these enabled
            // .arg("--disable-reading-from-canvas")  // REMOVED
            // .arg("--disable-3d-apis")              // REMOVED
            // .arg("--disable-accelerated-2d-canvas") // REMOVED

            // STEALTH: More anti-detection flags
            .arg("--disable-features=IsolateOrigins,site-per-process,TranslateUI")
            .arg("--disable-blink-features=AutomationControlled")
            .arg("--password-store=basic")
            .arg("--use-mock-keychain")
            // STEALTH: Fake screen resolution (common desktop)
            .arg("--window-position=0,0");

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
                builder = builder.arg(format!("--proxy-server={}", local_proxy));

                proxy_forwarder = Some(forwarder);
            } else {
                // Fallback: use proxy URL directly (for proxies without auth)
                let (chrome_proxy, _) = Self::parse_proxy_url(proxy_url);
                info!("Session {} using direct proxy: {}", session_id, chrome_proxy);
                builder = builder.arg(format!("--proxy-server={}", chrome_proxy));
            }
        }

        // Set window size
        builder = builder.window_size(config.window_width, config.window_height);

        let browser_config = builder.build()
            .map_err(|e| BrowserError::LaunchFailed(e.to_string()))?;

        // Launch browser
        let (browser, mut handler) = Browser::launch(browser_config)
            .await
            .map_err(|e| BrowserError::LaunchFailed(e.to_string()))?;

        // Spawn handler in background
        let session_id_clone = session_id.clone();
        tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                debug!("Session {} browser event: {:?}", session_id_clone, event);
            }
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

        // Inject anti-detection JavaScript
        Self::inject_anti_detection(&page).await?;

        info!("Browser session {} created successfully", session_id);

        Ok(Self {
            id: session_id,
            browser: Arc::new(RwLock::new(Some(browser))),
            page: Arc::new(RwLock::new(Some(page))),
            config,
            alive: Arc::new(AtomicBool::new(true)),
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
        let page = self.page.read().await;
        let page = page.as_ref().ok_or(BrowserError::ConnectionLost("No active page".into()))?;

        debug!("Session {} navigating to: {}", self.id, url);
        page.goto(url)
            .await
            .map_err(|e| BrowserError::NavigationFailed(e.to_string()))?;

        Ok(())
    }

    /// Wait for navigation to complete
    pub async fn wait_for_navigation(&self, timeout_secs: u64) -> Result<(), BrowserError> {
        let page = self.page.read().await;
        let page = page.as_ref().ok_or(BrowserError::ConnectionLost("No active page".into()))?;

        tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            page.wait_for_navigation()
        )
        .await
        .map_err(|_| BrowserError::Timeout("Navigation timeout".into()))?
        .map_err(|e| BrowserError::NavigationFailed(e.to_string()))?;

        Ok(())
    }

    /// Execute JavaScript on the page with default 60 second timeout
    pub async fn execute_js(&self, script: &str) -> Result<serde_json::Value, BrowserError> {
        self.execute_js_with_timeout(script, 60).await
    }

    /// Execute JavaScript on the page with custom timeout (in seconds)
    pub async fn execute_js_with_timeout(&self, script: &str, timeout_secs: u64) -> Result<serde_json::Value, BrowserError> {
        let page = self.page.read().await;
        let page = page.as_ref().ok_or(BrowserError::ConnectionLost("No active page".into()))?;

        let result = tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            page.evaluate(script)
        )
        .await
        .map_err(|_| BrowserError::Timeout(format!("JavaScript execution timed out after {}s", timeout_secs)))?
        .map_err(|e| BrowserError::JavaScriptError(e.to_string()))?;

        Ok(result.value().cloned().unwrap_or(serde_json::Value::Null))
    }

    /// Get current URL
    pub async fn get_current_url(&self) -> Result<String, BrowserError> {
        let page = self.page.read().await;
        let page = page.as_ref().ok_or(BrowserError::ConnectionLost("No active page".into()))?;

        page.url()
            .await
            .map_err(|e| BrowserError::ConnectionLost(e.to_string()))?
            .ok_or_else(|| BrowserError::ConnectionLost("No URL".into()))
    }

    /// Click on an element by selector
    pub async fn click(&self, selector: &str) -> Result<(), BrowserError> {
        let page = self.page.read().await;
        let page = page.as_ref().ok_or(BrowserError::ConnectionLost("No active page".into()))?;

        let element = page.find_element(selector)
            .await
            .map_err(|e| BrowserError::ElementNotFound(format!("{}: {}", selector, e)))?;

        element.click()
            .await
            .map_err(|e| BrowserError::JavaScriptError(e.to_string()))?;

        self.increment_clicks();
        Ok(())
    }

    /// Type text into an element
    pub async fn type_text(&self, selector: &str, text: &str) -> Result<(), BrowserError> {
        let page = self.page.read().await;
        let page = page.as_ref().ok_or(BrowserError::ConnectionLost("No active page".into()))?;

        let element = page.find_element(selector)
            .await
            .map_err(|e| BrowserError::ElementNotFound(format!("{}: {}", selector, e)))?;

        element.click().await.ok();
        element.type_str(text)
            .await
            .map_err(|e| BrowserError::JavaScriptError(e.to_string()))?;

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
        self.alive.store(false, Ordering::Relaxed);

        // Close page
        {
            let mut page = self.page.write().await;
            if let Some(p) = page.take() {
                let _ = p.close().await;
            }
        }

        // Close browser
        {
            let mut browser = self.browser.write().await;
            if let Some(mut b) = browser.take() {
                let _ = b.close().await;
            }
        }

        // Stop local proxy forwarder
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
                   host, port, &username[..username.len().min(30)], password.len());

            Some((host, port, username, password))
        } else {
            None
        }
    }

    /// Inject comprehensive anti-detection JavaScript (puppeteer-stealth equivalent)
    async fn inject_anti_detection(page: &Page) -> Result<(), BrowserError> {
        // Comprehensive stealth script based on puppeteer-extra-plugin-stealth
        page.evaluate(r#"
            // =========== STEALTH EVASIONS ===========

            // 1. Remove webdriver property
            Object.defineProperty(navigator, 'webdriver', {
                get: () => undefined,
                configurable: true
            });
            delete Object.getPrototypeOf(navigator).webdriver;

            // 2. Mock plugins array (realistic Chrome plugins)
            Object.defineProperty(navigator, 'plugins', {
                get: () => {
                    const plugins = [
                        {
                            name: 'Chrome PDF Plugin',
                            filename: 'internal-pdf-viewer',
                            description: 'Portable Document Format',
                            length: 1,
                            0: { type: 'application/x-google-chrome-pdf', suffixes: 'pdf', description: 'Portable Document Format' }
                        },
                        {
                            name: 'Chrome PDF Viewer',
                            filename: 'mhjfbmdgcfjbbpaeojofohoefgiehjai',
                            description: '',
                            length: 1,
                            0: { type: 'application/pdf', suffixes: 'pdf', description: '' }
                        },
                        {
                            name: 'Native Client',
                            filename: 'internal-nacl-plugin',
                            description: '',
                            length: 2,
                            0: { type: 'application/x-nacl', suffixes: '', description: 'Native Client Executable' },
                            1: { type: 'application/x-pnacl', suffixes: '', description: 'Portable Native Client Executable' }
                        }
                    ];
                    plugins.item = (i) => plugins[i] || null;
                    plugins.namedItem = (name) => plugins.find(p => p.name === name) || null;
                    plugins.refresh = () => {};
                    Object.setPrototypeOf(plugins, PluginArray.prototype);
                    return plugins;
                }
            });

            // 3. Mock mimeTypes
            Object.defineProperty(navigator, 'mimeTypes', {
                get: () => {
                    const mimes = [
                        { type: 'application/pdf', suffixes: 'pdf', description: 'Portable Document Format', enabledPlugin: navigator.plugins[0] },
                        { type: 'application/x-google-chrome-pdf', suffixes: 'pdf', description: 'Portable Document Format', enabledPlugin: navigator.plugins[0] },
                        { type: 'application/x-nacl', suffixes: '', description: 'Native Client Executable', enabledPlugin: navigator.plugins[2] },
                        { type: 'application/x-pnacl', suffixes: '', description: 'Portable Native Client Executable', enabledPlugin: navigator.plugins[2] }
                    ];
                    mimes.item = (i) => mimes[i] || null;
                    mimes.namedItem = (name) => mimes.find(m => m.type === name) || null;
                    Object.setPrototypeOf(mimes, MimeTypeArray.prototype);
                    return mimes;
                }
            });

            // 4. Mock languages (Arabic + English for Saudi region)
            Object.defineProperty(navigator, 'languages', { get: () => Object.freeze(['ar-SA', 'ar', 'en-US', 'en']) });
            Object.defineProperty(navigator, 'language', { get: () => 'ar-SA' });

            // 5. Mock platform and userAgent properties
            Object.defineProperty(navigator, 'platform', { get: () => 'Win32' });
            Object.defineProperty(navigator, 'vendor', { get: () => 'Google Inc.' });
            Object.defineProperty(navigator, 'productSub', { get: () => '20030107' });

            // 6. Mock hardware properties
            Object.defineProperty(navigator, 'hardwareConcurrency', { get: () => 8 });
            Object.defineProperty(navigator, 'deviceMemory', { get: () => 8 });
            Object.defineProperty(navigator, 'maxTouchPoints', { get: () => 0 });

            // 7. Mock permissions API
            const originalQuery = navigator.permissions?.query?.bind(navigator.permissions);
            if (originalQuery) {
                navigator.permissions.query = (params) => {
                    if (params.name === 'notifications') {
                        return Promise.resolve({ state: Notification.permission, onchange: null });
                    }
                    return originalQuery(params);
                };
            }

            // 8. Mock chrome runtime object
            window.chrome = window.chrome || {};
            window.chrome.runtime = {
                connect: () => {},
                sendMessage: () => {},
                onConnect: { addListener: () => {} },
                onMessage: { addListener: () => {} }
            };
            window.chrome.loadTimes = () => ({
                requestTime: Date.now() / 1000 - 5,
                startLoadTime: Date.now() / 1000 - 4,
                commitLoadTime: Date.now() / 1000 - 3,
                finishDocumentLoadTime: Date.now() / 1000 - 2,
                finishLoadTime: Date.now() / 1000 - 1,
                firstPaintTime: Date.now() / 1000 - 0.5,
                firstPaintAfterLoadTime: 0,
                navigationType: 'Other',
                wasFetchedViaSpdy: false,
                wasNpnNegotiated: true,
                npnNegotiatedProtocol: 'h2',
                wasAlternateProtocolAvailable: false,
                connectionInfo: 'h2'
            });
            window.chrome.csi = () => ({
                startE: Date.now() - 5000,
                onloadT: Date.now() - 1000,
                pageT: Date.now(),
                tran: 15
            });
            window.chrome.app = { isInstalled: false, InstallState: { DISABLED: 'disabled', INSTALLED: 'installed', NOT_INSTALLED: 'not_installed' }, RunningState: { CANNOT_RUN: 'cannot_run', READY_TO_RUN: 'ready_to_run', RUNNING: 'running' } };

            // 9. WebGL fingerprint evasion
            const getParameterProxy = new Proxy(WebGLRenderingContext.prototype.getParameter, {
                apply: function(target, thisArg, args) {
                    if (args[0] === 37445) return 'Intel Inc.'; // UNMASKED_VENDOR_WEBGL
                    if (args[0] === 37446) return 'Intel(R) Iris(R) Xe Graphics'; // UNMASKED_RENDERER_WEBGL
                    return Reflect.apply(target, thisArg, args);
                }
            });
            WebGLRenderingContext.prototype.getParameter = getParameterProxy;

            try {
                const getParameter2Proxy = new Proxy(WebGL2RenderingContext.prototype.getParameter, {
                    apply: function(target, thisArg, args) {
                        if (args[0] === 37445) return 'Intel Inc.';
                        if (args[0] === 37446) return 'Intel(R) Iris(R) Xe Graphics';
                        return Reflect.apply(target, thisArg, args);
                    }
                });
                WebGL2RenderingContext.prototype.getParameter = getParameter2Proxy;
            } catch(e) {}

            // 10. Visibility state (always visible)
            Object.defineProperty(document, 'hidden', { get: () => false });
            Object.defineProperty(document, 'visibilityState', { get: () => 'visible' });

            // 11. Fix toString() for native functions
            const nativeToStringFunctionString = Error.toString().replace(/Error/g, 'toString');
            const oldCall = Function.prototype.call;
            function call() { return oldCall.apply(this, arguments); }
            Function.prototype.call = call;
            const nativeToStringFunction = Function.prototype.toString;
            const proxiedToString = new Proxy(nativeToStringFunction, {
                apply: function(target, thisArg, args) {
                    if (thisArg === navigator.permissions.query) return 'function query() { [native code] }';
                    if (thisArg === document.hasFocus) return 'function hasFocus() { [native code] }';
                    return Reflect.apply(target, thisArg, args);
                }
            });
            Function.prototype.toString = proxiedToString;

            // 12. Mock connection API
            Object.defineProperty(navigator, 'connection', {
                get: () => ({
                    effectiveType: '4g',
                    rtt: 50 + Math.floor(Math.random() * 50),
                    downlink: 10 + Math.random() * 5,
                    saveData: false,
                    onchange: null,
                    addEventListener: () => {},
                    removeEventListener: () => {}
                })
            });

            // 13. Mock battery API
            navigator.getBattery = () => Promise.resolve({
                charging: true,
                chargingTime: 0,
                dischargingTime: Infinity,
                level: 0.95 + Math.random() * 0.05,
                addEventListener: () => {},
                removeEventListener: () => {},
                onchargingchange: null,
                onchargingtimechange: null,
                ondischargingtimechange: null,
                onlevelchange: null
            });

            // 14. Canvas fingerprint protection (add subtle noise)
            const originalGetImageData = CanvasRenderingContext2D.prototype.getImageData;
            CanvasRenderingContext2D.prototype.getImageData = function(...args) {
                const imageData = originalGetImageData.apply(this, args);
                // Add very subtle noise to prevent exact fingerprinting
                for (let i = 0; i < imageData.data.length; i += 4) {
                    if (Math.random() < 0.01) {
                        imageData.data[i] = Math.max(0, Math.min(255, imageData.data[i] + (Math.random() > 0.5 ? 1 : -1)));
                    }
                }
                return imageData;
            };

            // 15. Prevent automation detection via CDP
            delete window.cdc_adoQpoasnfa76pfcZLmcfl_Array;
            delete window.cdc_adoQpoasnfa76pfcZLmcfl_Promise;
            delete window.cdc_adoQpoasnfa76pfcZLmcfl_Symbol;

            // 16. Mock getClientRects (prevent empty iframe detection)
            const originalGetClientRects = Element.prototype.getClientRects;
            Element.prototype.getClientRects = function() {
                const rects = originalGetClientRects.apply(this, arguments);
                if (rects.length === 0 && this.tagName === 'IFRAME') {
                    return [{ top: 0, right: 0, bottom: 0, left: 0, width: 0, height: 0 }];
                }
                return rects;
            };

            // 17. Prevent WebDriver detection via error stack traces
            const originalError = Error;
            Error = function(...args) {
                const err = new originalError(...args);
                const stack = err.stack;
                if (stack && stack.includes('webdriver')) {
                    err.stack = stack.replace(/webdriver/gi, 'driver');
                }
                return err;
            };
            Error.prototype = originalError.prototype;

            // 18. Mock performance.memory (Chromium only)
            if (window.performance && !window.performance.memory) {
                Object.defineProperty(window.performance, 'memory', {
                    get: () => ({
                        jsHeapSizeLimit: 4294705152,
                        totalJSHeapSize: 35839098,
                        usedJSHeapSize: 28678374
                    })
                });
            }

            // 19. WebRTC IP leak protection
            if (navigator.mediaDevices && navigator.mediaDevices.getUserMedia) {
                navigator.mediaDevices.getUserMedia = () => Promise.reject(new Error('Permission denied'));
            }
            if (window.RTCPeerConnection) {
                window.RTCPeerConnection = class extends window.RTCPeerConnection {
                    constructor(config) {
                        if (config && config.iceServers) {
                            config.iceServers = [];
                        }
                        super(config);
                    }
                };
            }
            if (window.RTCDataChannel) {
                const origCreateDataChannel = RTCPeerConnection.prototype.createDataChannel;
                RTCPeerConnection.prototype.createDataChannel = function() {
                    return origCreateDataChannel.apply(this, arguments);
                };
            }

            // 20. Timezone spoof (Saudi Arabia - UTC+3)
            const targetTimezone = 'Asia/Riyadh';
            const targetOffset = -180; // UTC+3 in minutes (negative because getTimezoneOffset returns opposite)

            const OriginalDate = Date;
            Date = class extends OriginalDate {
                constructor(...args) {
                    super(...args);
                }
                getTimezoneOffset() {
                    return targetOffset;
                }
            };
            Date.now = OriginalDate.now;
            Date.parse = OriginalDate.parse;
            Date.UTC = OriginalDate.UTC;

            // Also spoof Intl.DateTimeFormat
            const origDateTimeFormat = Intl.DateTimeFormat;
            Intl.DateTimeFormat = function(locales, options) {
                options = options || {};
                options.timeZone = options.timeZone || targetTimezone;
                return new origDateTimeFormat(locales, options);
            };
            Intl.DateTimeFormat.prototype = origDateTimeFormat.prototype;
            Intl.DateTimeFormat.supportedLocalesOf = origDateTimeFormat.supportedLocalesOf;

            // 21. Screen resolution (common desktop)
            Object.defineProperty(screen, 'width', { get: () => 1920 });
            Object.defineProperty(screen, 'height', { get: () => 1080 });
            Object.defineProperty(screen, 'availWidth', { get: () => 1920 });
            Object.defineProperty(screen, 'availHeight', { get: () => 1040 });
            Object.defineProperty(screen, 'colorDepth', { get: () => 24 });
            Object.defineProperty(screen, 'pixelDepth', { get: () => 24 });

            // 22. Prevent font fingerprinting
            const originalOffsetWidth = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'offsetWidth');
            const originalOffsetHeight = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'offsetHeight');

            // 23. Navigator properties for Saudi Arabia
            Object.defineProperty(navigator, 'doNotTrack', { get: () => '1' });

            // 24. Disable Notification API fingerprinting
            if (window.Notification) {
                window.Notification.requestPermission = () => Promise.resolve('denied');
            }

            // 25. AudioContext fingerprint protection
            if (window.AudioContext || window.webkitAudioContext) {
                const AudioCtx = window.AudioContext || window.webkitAudioContext;
                const origCreateAnalyser = AudioCtx.prototype.createAnalyser;
                AudioCtx.prototype.createAnalyser = function() {
                    const analyser = origCreateAnalyser.call(this);
                    const origGetFloatFrequencyData = analyser.getFloatFrequencyData;
                    analyser.getFloatFrequencyData = function(array) {
                        origGetFloatFrequencyData.call(this, array);
                        // Add slight noise
                        for (let i = 0; i < array.length; i++) {
                            array[i] += (Math.random() - 0.5) * 0.1;
                        }
                    };
                    return analyser;
                };
            }

            // 26. Speech synthesis fingerprint protection
            if (window.speechSynthesis) {
                const origGetVoices = window.speechSynthesis.getVoices;
                window.speechSynthesis.getVoices = function() {
                    return []; // Return empty to prevent fingerprinting
                };
            }

            // 27. Media codecs evasion (puppeteer-stealth)
            const originalCanPlayType = HTMLMediaElement.prototype.canPlayType;
            HTMLMediaElement.prototype.canPlayType = function(type) {
                // Return realistic responses for common codecs
                if (type.includes('video/mp4')) return 'probably';
                if (type.includes('video/webm')) return 'probably';
                if (type.includes('video/ogg')) return 'maybe';
                if (type.includes('audio/mpeg')) return 'probably';
                if (type.includes('audio/mp4')) return 'probably';
                if (type.includes('audio/ogg')) return 'probably';
                if (type.includes('audio/wav')) return 'probably';
                if (type.includes('audio/webm')) return 'probably';
                return originalCanPlayType.call(this, type);
            };

            // 28. iframe.contentWindow evasion (puppeteer-stealth)
            try {
                const originalContentWindow = Object.getOwnPropertyDescriptor(HTMLIFrameElement.prototype, 'contentWindow');
                if (originalContentWindow && originalContentWindow.get) {
                    Object.defineProperty(HTMLIFrameElement.prototype, 'contentWindow', {
                        get: function() {
                            const win = originalContentWindow.get.call(this);
                            if (win) {
                                try {
                                    // Ensure chrome object is consistent across iframes
                                    if (!win.chrome && window.chrome) {
                                        Object.defineProperty(win, 'chrome', {
                                            get: () => window.chrome,
                                            configurable: true
                                        });
                                    }
                                } catch (e) {
                                    // Cross-origin iframe, ignore
                                }
                            }
                            return win;
                        }
                    });
                }
            } catch (e) {
                // Fallback if property descriptor fails
            }

            // 29. Source URL evasion (hide automation traces in stack traces)
            try {
                const originalPrepareStackTrace = Error.prepareStackTrace;
                Error.prepareStackTrace = function(error, stack) {
                    let result;
                    if (originalPrepareStackTrace) {
                        result = originalPrepareStackTrace(error, stack);
                    } else {
                        result = stack.map(frame => frame.toString()).join('\n');
                    }
                    // Hide automation-related strings in stack traces
                    if (typeof result === 'string') {
                        result = result.replace(/puppeteer|chromium|headless|webdriver|selenium/gi, 'chrome');
                    }
                    return result;
                };
            } catch (e) {
                // Fallback for environments where prepareStackTrace isn't writable
            }

            // 30. Additional detection bypass for CDP (Chrome DevTools Protocol)
            try {
                // Hide Runtime.enable detection
                delete window.__proto__.Runtime;
                // Hide debugger detection
                const originalSetTimeout = window.setTimeout;
                window.setTimeout = function(fn, delay, ...args) {
                    if (typeof fn === 'string' && fn.includes('debugger')) {
                        return null;
                    }
                    return originalSetTimeout.call(this, fn, delay, ...args);
                };
            } catch (e) {}

            // =========== UNDETECTED-CHROMEDRIVER PATCHES ===========

            // Remove all cdc_ variables (Chrome DevTools Protocol markers)
            // These are injected by chromedriver and detected by anti-bot systems
            const cdcProps = Object.getOwnPropertyNames(window).filter(p => p.startsWith('cdc_') || p.startsWith('$cdc_'));
            for (const prop of cdcProps) {
                try { delete window[prop]; } catch(e) {}
            }

            // Remove $chrome_asyncScriptInfo (another chromedriver marker)
            try { delete window.$chrome_asyncScriptInfo; } catch(e) {}

            // Remove webdriver from navigator prototype chain
            try {
                const proto = Object.getPrototypeOf(navigator);
                if (proto.hasOwnProperty('webdriver')) {
                    delete proto.webdriver;
                }
            } catch(e) {}

            // Patch navigator.webdriver at multiple levels
            Object.defineProperty(Navigator.prototype, 'webdriver', {
                get: () => undefined,
                configurable: true
            });

            // Remove automation-related properties from window
            const autoProps = ['domAutomation', 'domAutomationController', '_phantom', '_selenium', 'callPhantom', 'callSelenium', '__nightmare', 'emit', 'spawn'];
            for (const prop of autoProps) {
                try { delete window[prop]; } catch(e) {}
            }

            // Patch document.$cdc_asdjflasutopfhvcZLmcfl_ (common chromedriver marker)
            const docCdcProps = Object.getOwnPropertyNames(document).filter(p => p.includes('cdc') || p.includes('driver'));
            for (const prop of docCdcProps) {
                try { delete document[prop]; } catch(e) {}
            }

            // =========== ADVANCED STEALTH (2025 techniques) ===========

            // 31. User-Agent Client Hints API (modern detection method)
            if (navigator.userAgentData) {
                Object.defineProperty(navigator, 'userAgentData', {
                    get: () => ({
                        brands: [
                            { brand: 'Google Chrome', version: '131' },
                            { brand: 'Chromium', version: '131' },
                            { brand: 'Not_A Brand', version: '24' }
                        ],
                        mobile: false,
                        platform: 'Windows',
                        getHighEntropyValues: (hints) => Promise.resolve({
                            architecture: 'x86',
                            bitness: '64',
                            brands: [
                                { brand: 'Google Chrome', version: '131' },
                                { brand: 'Chromium', version: '131' },
                                { brand: 'Not_A Brand', version: '24' }
                            ],
                            fullVersionList: [
                                { brand: 'Google Chrome', version: '131.0.6778.85' },
                                { brand: 'Chromium', version: '131.0.6778.85' },
                                { brand: 'Not_A Brand', version: '24.0.0.0' }
                            ],
                            mobile: false,
                            model: '',
                            platform: 'Windows',
                            platformVersion: '15.0.0',
                            uaFullVersion: '131.0.6778.85'
                        }),
                        toJSON: () => ({
                            brands: [
                                { brand: 'Google Chrome', version: '131' },
                                { brand: 'Chromium', version: '131' },
                                { brand: 'Not_A Brand', version: '24' }
                            ],
                            mobile: false,
                            platform: 'Windows'
                        })
                    })
                });
            }

            // 32. Window dimensions consistency (headless detection)
            Object.defineProperty(window, 'outerWidth', { get: () => window.innerWidth + 16 });
            Object.defineProperty(window, 'outerHeight', { get: () => window.innerHeight + 88 });
            Object.defineProperty(window, 'screenX', { get: () => 0 });
            Object.defineProperty(window, 'screenY', { get: () => 0 });
            Object.defineProperty(window, 'screenLeft', { get: () => 0 });
            Object.defineProperty(window, 'screenTop', { get: () => 0 });

            // 33. DevTools detection bypass (console timing attack)
            const originalConsoleLog = console.log;
            console.log = function(...args) {
                // Don't log anything that could trigger devtools detection
                if (args.some(arg => typeof arg === 'object' && arg !== null)) {
                    return originalConsoleLog.apply(this, args.map(a => typeof a === 'object' ? '[Object]' : a));
                }
                return originalConsoleLog.apply(this, args);
            };

            // 34. Document.hasFocus() - always return true
            document.hasFocus = () => true;

            // 35. Performance.now() noise (timing attack protection)
            const originalPerformanceNow = performance.now.bind(performance);
            performance.now = function() {
                return originalPerformanceNow() + Math.random() * 0.1;
            };

            // 36. requestAnimationFrame timing normalization
            const originalRAF = window.requestAnimationFrame;
            window.requestAnimationFrame = function(callback) {
                return originalRAF.call(window, function(timestamp) {
                    callback(timestamp + Math.random() * 0.1);
                });
            };

            // 37. Per-session unique canvas fingerprint
            const sessionNoise = Math.random() * 0.1;
            const origToDataURL = HTMLCanvasElement.prototype.toDataURL;
            HTMLCanvasElement.prototype.toDataURL = function(type) {
                const ctx = this.getContext('2d');
                if (ctx && this.width > 0 && this.height > 0) {
                    try {
                        const imageData = ctx.getImageData(0, 0, Math.min(this.width, 10), Math.min(this.height, 10));
                        for (let i = 0; i < imageData.data.length; i += 4) {
                            imageData.data[i] = Math.max(0, Math.min(255, imageData.data[i] + (sessionNoise > 0.05 ? 1 : -1)));
                        }
                        ctx.putImageData(imageData, 0, 0);
                    } catch(e) {}
                }
                return origToDataURL.apply(this, arguments);
            };

            // 38. WebGL renderer randomization per session
            const gpuRenderers = [
                'Intel(R) Iris(R) Xe Graphics',
                'Intel(R) UHD Graphics 620',
                'Intel(R) HD Graphics 630',
                'NVIDIA GeForce GTX 1650',
                'AMD Radeon RX 580'
            ];
            const sessionRenderer = gpuRenderers[Math.floor(Math.random() * gpuRenderers.length)];

            // Re-apply with session-specific renderer
            const getParamProxyAdvanced = new Proxy(WebGLRenderingContext.prototype.getParameter, {
                apply: function(target, thisArg, args) {
                    if (args[0] === 37445) return 'Intel Inc.';
                    if (args[0] === 37446) return sessionRenderer;
                    return Reflect.apply(target, thisArg, args);
                }
            });
            WebGLRenderingContext.prototype.getParameter = getParamProxyAdvanced;

            // 39. Keyboard event timing humanization
            const originalAddEventListener = EventTarget.prototype.addEventListener;
            EventTarget.prototype.addEventListener = function(type, listener, options) {
                if (type === 'keydown' || type === 'keyup' || type === 'keypress') {
                    const wrappedListener = function(event) {
                        // Add micro-delay to simulate human reaction time variance
                        setTimeout(() => listener.call(this, event), Math.random() * 5);
                    };
                    return originalAddEventListener.call(this, type, wrappedListener, options);
                }
                return originalAddEventListener.call(this, type, listener, options);
            };

            // 40. Storage quota fingerprint protection
            if (navigator.storage && navigator.storage.estimate) {
                const origEstimate = navigator.storage.estimate.bind(navigator.storage);
                navigator.storage.estimate = async function() {
                    const result = await origEstimate();
                    // Return slightly randomized values
                    return {
                        quota: result.quota || 1073741824,
                        usage: Math.floor(Math.random() * 1000000),
                        usageDetails: {}
                    };
                };
            }

            // 41. Screen orientation lock (mobile detection)
            if (screen.orientation) {
                Object.defineProperty(screen.orientation, 'type', { get: () => 'landscape-primary' });
                Object.defineProperty(screen.orientation, 'angle', { get: () => 0 });
            }

            // 42. Bluetooth API protection
            if (navigator.bluetooth) {
                navigator.bluetooth.getAvailability = () => Promise.resolve(false);
                navigator.bluetooth.requestDevice = () => Promise.reject(new Error('User cancelled'));
            }

            // 43. USB API protection
            if (navigator.usb) {
                navigator.usb.getDevices = () => Promise.resolve([]);
                navigator.usb.requestDevice = () => Promise.reject(new Error('No device selected'));
            }

            // 44. Serial API protection
            if (navigator.serial) {
                navigator.serial.getPorts = () => Promise.resolve([]);
                navigator.serial.requestPort = () => Promise.reject(new Error('No port selected'));
            }

            // 45. HID API protection
            if (navigator.hid) {
                navigator.hid.getDevices = () => Promise.resolve([]);
                navigator.hid.requestDevice = () => Promise.reject(new Error('No device selected'));
            }

            // 46. Gamepad API protection
            navigator.getGamepads = () => [null, null, null, null];

            // 47. Credential API protection
            if (navigator.credentials) {
                navigator.credentials.get = () => Promise.resolve(null);
                navigator.credentials.store = () => Promise.resolve();
            }

            // 48. Payment API protection
            if (window.PaymentRequest) {
                window.PaymentRequest = class {
                    constructor() { throw new Error('Not supported'); }
                };
            }

            // 49. Network Information randomization
            if (navigator.connection) {
                const connectionTypes = ['wifi', '4g', 'ethernet'];
                const sessionConnectionType = connectionTypes[Math.floor(Math.random() * connectionTypes.length)];
                Object.defineProperty(navigator.connection, 'type', { get: () => sessionConnectionType });
            }

            // 50. Beacon API tracking (allow but log nothing)
            const origSendBeacon = navigator.sendBeacon;
            navigator.sendBeacon = function(url, data) {
                // Allow beacon but could be used for fingerprinting
                return origSendBeacon.call(this, url, data);
            };

            console.log('[Stealth] Ultimate anti-detection initialized (50 evasions active) - Session: ' + Math.random().toString(36).substr(2, 8));
        "#)
        .await
        .map_err(|e| BrowserError::JavaScriptError(e.to_string()))?;

        debug!("Stealth anti-detection scripts injected");
        Ok(())
    }
}

impl Drop for BrowserSession {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Relaxed);
    }
}
