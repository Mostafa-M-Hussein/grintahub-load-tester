//! CAPTCHA solving module
//!
//! Provides CAPTCHA solving via 2Captcha service:
//! - reCAPTCHA v2/v3
//! - Turnstile
//! - Race mode (parallel solves for speed)

mod solver;
mod types;

pub use solver::CaptchaSolver;
pub use types::*;
