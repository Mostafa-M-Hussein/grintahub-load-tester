//! Authentication module for GrintaHub
//!
//! Provides:
//! - Account registration with CAPTCHA solving
//! - Login with CAPTCHA solving
//! - Batch account creation
//! - Fake data generation

mod client;
mod types;

pub use client::AuthClient;
pub use types::*;
