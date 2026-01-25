//! Browser automation module
//!
//! Handles launching and controlling multiple Chrome/Chromium browser instances
//! for load testing with unique proxy per session.

mod session;
mod pool;
mod actions;
mod errors;

pub use session::{BrowserSession, BrowserSessionConfig};
pub use pool::{BrowserPool, SessionInfo};
pub use actions::{BrowserActions, GoogleAccount};
pub use errors::BrowserError;
