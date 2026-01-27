//! GrintaHub Clicker
//!
//! A Tauri desktop application for automated browsing on grintahub.com with
//! multi-session browser automation and proxy rotation.

pub mod proxy;
pub mod browser;
pub mod stats;
pub mod rate;
pub mod scheduler;
pub mod captcha;
pub mod auth;
pub mod supervisor;
pub mod bot;
pub mod web;

#[cfg(feature = "desktop")]
mod commands;

use std::sync::Arc;
use std::path::PathBuf;
use tokio::sync::RwLock;
use tracing::{info, warn, error};

use proxy::GlobalProxyManager;
use browser::BrowserPool;
use stats::GlobalStats;
use rate::RateLimiterConfig;
use scheduler::{Scheduler, ScheduleConfig};

/// Application configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    /// Proxy configuration
    pub proxy_customer: String,
    pub proxy_password: String,
    pub proxy_country: String,
    /// Session time in minutes (how long to keep the same IP)
    #[serde(default = "default_sesstime")]
    pub proxy_sesstime: u16,
    #[serde(default)]
    pub proxy_verified: bool,

    /// 2Captcha API key for CAPTCHA solving
    #[serde(default)]
    pub captcha_api_key: String,

    /// Session configuration
    pub concurrent_sessions: usize,
    pub headless: bool,

    /// Rate limiter configuration
    pub clicks_per_hour: u32,
    pub min_delay_ms: u64,
    pub max_delay_ms: u64,

    /// Max clicks per session before auto-close (0 = unlimited)
    #[serde(default)]
    pub max_clicks_per_session: u32,

    /// Keywords to search
    pub keywords: Vec<String>,

    /// Schedule configuration
    pub schedule: ScheduleConfig,

    /// Saved accounts for authenticated sessions
    #[serde(default)]
    pub accounts: Vec<auth::Account>,

    /// Auto-rotate IP after completing all keywords (restart session with new IP)
    #[serde(default)]
    pub auto_rotate_ip: bool,
}

/// Default session time in minutes
fn default_sesstime() -> u16 { 30 }

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            proxy_customer: String::new(),
            proxy_password: String::new(),
            proxy_country: "sa".to_string(),
            proxy_sesstime: default_sesstime(),
            proxy_verified: false,
            captcha_api_key: String::new(),
            concurrent_sessions: 5,
            headless: false,
            clicks_per_hour: 120,
            min_delay_ms: 3000,
            max_delay_ms: 10000,
            max_clicks_per_session: 0,  // 0 = unlimited (NEVER stop)
            keywords: vec![
                "تذاكر نادي الهلال".to_string(),
                "تذاكر الهلال والاهلي".to_string(),
                "تذاكر الهلال".to_string(),
                "منصة بيع تذاكر الهلال".to_string(),
                "حجز تذاكر مباراة الهلال والاهلي".to_string(),
            ],
            schedule: ScheduleConfig::default(),
            accounts: vec![],
            auto_rotate_ip: true,  // Rotate IP after completing all keywords
        }
    }
}

/// Get log directory path (shared across modules)
pub fn log_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|p| p.join("grintahub-clicker").join("logs"))
}

impl AppConfig {
    /// Get config file path
    fn config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|p| p.join("grintahub-clicker").join("config.json"))
    }

    /// Load config from file
    pub fn load() -> Self {
        if let Some(path) = Self::config_path() {
            if path.exists() {
                match std::fs::read_to_string(&path) {
                    Ok(content) => {
                        match serde_json::from_str(&content) {
                            Ok(config) => {
                                info!("Loaded config from {:?}", path);
                                return config;
                            }
                            Err(e) => {
                                warn!("Failed to parse config file: {}", e);
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Failed to read config file: {}", e);
                    }
                }
            }
        }
        Self::default()
    }

    /// Save config to file
    pub fn save(&self) {
        if let Some(path) = Self::config_path() {
            // Create parent directory if needed
            if let Some(parent) = path.parent() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    error!("Failed to create config directory: {}", e);
                    return;
                }
            }

            match serde_json::to_string_pretty(self) {
                Ok(content) => {
                    if let Err(e) = std::fs::write(&path, content) {
                        error!("Failed to save config: {}", e);
                    } else {
                        info!("Config saved to {:?}", path);
                    }
                }
                Err(e) => {
                    error!("Failed to serialize config: {}", e);
                }
            }
        }
    }
}

/// Application state shared across the app
pub struct AppState {
    /// Proxy manager
    pub proxy_manager: Arc<GlobalProxyManager>,
    /// Browser pool
    pub browser_pool: Arc<BrowserPool>,
    /// Global statistics
    pub global_stats: Arc<GlobalStats>,
    /// Rate limiter configuration
    pub rate_config: Arc<RwLock<RateLimiterConfig>>,
    /// Scheduler
    pub scheduler: Arc<Scheduler>,
    /// Application configuration
    pub config: Arc<RwLock<AppConfig>>,
    /// Bot running state
    pub is_running: Arc<std::sync::atomic::AtomicBool>,
    /// Supervisor task handle (for stopping it on bot stop)
    pub supervisor_handle: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl AppState {
    /// Create new application state with loaded config
    pub fn new() -> Self {
        // Load saved config
        let saved_config = AppConfig::load();

        // Initialize proxy manager with saved credentials
        let proxy_manager = if !saved_config.proxy_customer.is_empty() && !saved_config.proxy_password.is_empty() {
            let pm = GlobalProxyManager::disabled();
            pm.configure(
                &saved_config.proxy_customer,
                &saved_config.proxy_password,
                Some(&saved_config.proxy_country),
                Some(saved_config.proxy_sesstime),
            );
            // Restore verified status
            if saved_config.proxy_verified {
                pm.set_verified(true);
            }
            Arc::new(pm)
        } else {
            Arc::new(GlobalProxyManager::disabled())
        };

        let browser_pool = Arc::new(BrowserPool::new(proxy_manager.clone()));

        // Initialize rate config from saved config
        let rate_config = RateLimiterConfig {
            clicks_per_hour: saved_config.clicks_per_hour,
            min_delay_ms: saved_config.min_delay_ms,
            max_delay_ms: saved_config.max_delay_ms,
            ..RateLimiterConfig::default()
        };

        Self {
            proxy_manager,
            browser_pool,
            global_stats: Arc::new(GlobalStats::new()),
            rate_config: Arc::new(RwLock::new(rate_config)),
            scheduler: Arc::new(Scheduler::new()),
            config: Arc::new(RwLock::new(saved_config)),
            is_running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            supervisor_handle: tokio::sync::Mutex::new(None),
        }
    }

    /// Configure the application with new settings
    pub async fn configure(&self, config: AppConfig) {
        // Update proxy manager
        if !config.proxy_customer.is_empty() && !config.proxy_password.is_empty() {
            self.proxy_manager.configure(
                &config.proxy_customer,
                &config.proxy_password,
                Some(&config.proxy_country),
                Some(config.proxy_sesstime),
            );
        }

        // Update rate config
        {
            let mut rate_config = self.rate_config.write().await;
            rate_config.clicks_per_hour = config.clicks_per_hour;
            rate_config.min_delay_ms = config.min_delay_ms;
            rate_config.max_delay_ms = config.max_delay_ms;
        }

        // Update scheduler
        self.scheduler.set_config(config.schedule.clone()).await;

        // Save config to file
        config.save();

        // Store config in memory
        *self.config.write().await = config;

        info!("Application configured");
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

/// Initialize logging (shared between desktop and server modes)
pub fn init_logging() -> Option<tracing_appender::non_blocking::WorkerGuard> {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let env_filter = tracing_subscriber::EnvFilter::from_default_env()
        .add_directive(tracing::Level::INFO.into());

    let console_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_thread_ids(false);

    if let Some(log_dir) = log_dir() {
        let _ = std::fs::create_dir_all(&log_dir);
        let file_appender = tracing_appender::rolling::daily(&log_dir, "grintahub-clicker.log");
        let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

        let file_layer = tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_target(true)
            .with_thread_ids(true)
            .with_writer(non_blocking);

        tracing_subscriber::registry()
            .with(env_filter)
            .with(console_layer)
            .with(file_layer)
            .init();

        Some(guard)
    } else {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(console_layer)
            .init();

        None
    }
}

/// Desktop (Tauri) entry point
#[cfg(feature = "desktop")]
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    use tauri::Manager;

    let _guard = init_logging();

    info!("Starting GrintaHub Clicker (desktop mode)");
    if let Some(dir) = log_dir() {
        info!("Log files saved to: {}", dir.display());
    }

    tauri::Builder::default()
        .setup(|app| {
            // Initialize app state
            let state = AppState::new();
            app.manage(state);

            info!("Application setup complete");
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::configure,
            commands::get_config,
            commands::start_sessions,
            commands::stop_sessions,
            commands::get_session_info,
            commands::get_global_stats,
            commands::start_bot,
            commands::stop_bot,
            commands::get_bot_status,
            commands::detect_ips,
            commands::set_schedule,
            commands::get_schedule_status,
            commands::test_proxy,
            commands::is_proxy_verified,
            commands::get_log_dir,
            // Manual test browser
            commands::open_test_browser,
            commands::close_test_browser,
            // Session management
            commands::close_session,
            // CAPTCHA commands
            commands::get_captcha_balance,
            commands::test_captcha,
            // Auth commands
            commands::register_account,
            commands::login_account,
            commands::batch_register_accounts,
            commands::get_saved_accounts,
            commands::delete_account,
            // Oxylabs usage
            commands::get_oxylabs_usage,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
