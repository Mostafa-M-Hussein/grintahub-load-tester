//! Basic authentication middleware for the web server.
//!
//! Reads credentials from environment variables:
//! - `GRINTAHUB_WEB_USER` (default: "admin")
//! - `GRINTAHUB_WEB_PASS` (required for auth to be enabled)

use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::Response,
};
use base64::Engine;
use tracing::warn;

/// Basic auth middleware.
///
/// If `GRINTAHUB_WEB_PASS` is not set, authentication is disabled (open access).
/// When enabled, all requests must include a valid `Authorization: Basic ...` header.
pub async fn basic_auth_middleware(
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let expected_pass = match std::env::var("GRINTAHUB_WEB_PASS") {
        Ok(p) if !p.is_empty() => p,
        _ => {
            // No password configured — skip auth
            return Ok(next.run(request).await);
        }
    };

    let expected_user = std::env::var("GRINTAHUB_WEB_USER")
        .unwrap_or_else(|_| "admin".to_string());

    // Extract Authorization header
    let auth_header = request
        .headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok());

    let auth_header = match auth_header {
        Some(h) => h,
        None => {
            warn!("[Auth] Missing Authorization header");
            return Err(StatusCode::UNAUTHORIZED);
        }
    };

    // Parse "Basic <base64>" format
    if !auth_header.starts_with("Basic ") {
        warn!("[Auth] Invalid auth scheme (expected Basic)");
        return Err(StatusCode::UNAUTHORIZED);
    }

    let encoded = &auth_header[6..];
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| {
            warn!("[Auth] Invalid base64 in Authorization header");
            StatusCode::UNAUTHORIZED
        })?;

    let credentials = String::from_utf8(decoded).map_err(|_| {
        warn!("[Auth] Invalid UTF-8 in credentials");
        StatusCode::UNAUTHORIZED
    })?;

    // Split "username:password"
    let mut parts = credentials.splitn(2, ':');
    let username = parts.next().unwrap_or("");
    let password = parts.next().unwrap_or("");

    if username == expected_user && password == expected_pass {
        Ok(next.run(request).await)
    } else {
        warn!("[Auth] Invalid credentials for user: {}", username);
        Err(StatusCode::UNAUTHORIZED)
    }
}
