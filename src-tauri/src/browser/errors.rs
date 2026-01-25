//! Browser error types

use thiserror::Error;

/// Browser-related errors
#[derive(Error, Debug)]
pub enum BrowserError {
    #[error("Failed to launch browser: {0}")]
    LaunchFailed(String),

    #[error("Navigation failed: {0}")]
    NavigationFailed(String),

    #[error("JavaScript error: {0}")]
    JavaScriptError(String),

    #[error("Connection lost: {0}")]
    ConnectionLost(String),

    #[error("Timeout: {0}")]
    Timeout(String),

    #[error("Element not found: {0}")]
    ElementNotFound(String),

    #[error("Session not found: {0}")]
    SessionNotFound(String),

    #[error("Pool error: {0}")]
    PoolError(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Google CAPTCHA detected: {0}")]
    CaptchaDetected(String),
}

impl From<BrowserError> for String {
    fn from(err: BrowserError) -> String {
        err.to_string()
    }
}
