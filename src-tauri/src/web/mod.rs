//! Web server module for headless server mode.
//!
//! Provides an axum-based HTTP server that serves the React frontend
//! and exposes REST API endpoints equivalent to the Tauri commands.

pub mod auth;
pub mod routes;

use std::sync::Arc;
use axum::Router;
use tower_http::cors::{CorsLayer, Any};
use tower_http::services::ServeDir;
use tracing::info;

use crate::AppState;

/// Build the complete axum router with API routes and static file serving.
pub fn build_router(state: Arc<AppState>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let api_routes = routes::api_router(state.clone());

    Router::new()
        .nest("/api", api_routes)
        // Serve static files from ./dist/ directory (React build output)
        .fallback_service(ServeDir::new("dist").append_index_html_on_directories(true))
        .layer(cors)
}

/// Start the web server on the given port.
pub async fn start_server(state: Arc<AppState>, port: u16) -> Result<(), Box<dyn std::error::Error>> {
    let app = build_router(state);

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    info!("Web server listening on http://0.0.0.0:{}", port);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
