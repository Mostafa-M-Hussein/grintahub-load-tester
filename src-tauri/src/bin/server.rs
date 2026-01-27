//! GrintaHub Clicker - Standalone Web Server
//!
//! Runs the bot with a web dashboard accessible via browser.
//! Build: `cargo build --release --no-default-features --bin server`
//!
//! Environment variables:
//! - `GRINTAHUB_WEB_PORT` - Server port (default: 8080)
//! - `GRINTAHUB_WEB_USER` - Basic auth username (default: "admin")
//! - `GRINTAHUB_WEB_PASS` - Basic auth password (auth disabled if not set)

use std::sync::Arc;
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging (same as desktop mode)
    let _guard = app_lib::init_logging();

    info!("Starting GrintaHub Clicker (server mode)");

    if let Some(dir) = app_lib::log_dir() {
        info!("Log files saved to: {}", dir.display());
    }

    // Read port from environment
    let port: u16 = std::env::var("GRINTAHUB_WEB_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);

    // Log auth status
    if std::env::var("GRINTAHUB_WEB_PASS").map(|p| !p.is_empty()).unwrap_or(false) {
        let user = std::env::var("GRINTAHUB_WEB_USER").unwrap_or_else(|_| "admin".to_string());
        info!("Basic auth enabled (user: {})", user);
    } else {
        info!("Basic auth disabled (set GRINTAHUB_WEB_PASS to enable)");
    }

    // Initialize application state
    let state = Arc::new(app_lib::AppState::new());

    // Server mode: check for display (Xvfb or real)
    // If DISPLAY is set, we can use GUI mode (better anti-detection)
    // If no display, force headless
    {
        let mut config = state.config.write().await;
        let has_display = std::env::var("DISPLAY").map(|d| !d.is_empty()).unwrap_or(false);

        if has_display {
            // Xvfb or real display available - use GUI mode for better anti-detection
            if config.headless {
                info!("Server mode: DISPLAY={} detected (Xvfb) - using GUI mode for anti-detection",
                      std::env::var("DISPLAY").unwrap_or_default());
                config.headless = false;
                config.save();
            } else {
                info!("Server mode: using GUI mode (DISPLAY={})", std::env::var("DISPLAY").unwrap_or_default());
            }
        } else if !config.headless {
            // No display available - must use headless
            info!("Server mode: no DISPLAY - forcing headless=true");
            config.headless = true;
            config.save();
        }
    }

    info!("Application state initialized");
    info!("Dashboard: http://0.0.0.0:{}", port);

    // Start the web server (blocks until shutdown)
    app_lib::web::start_server(state, port).await?;

    Ok(())
}
