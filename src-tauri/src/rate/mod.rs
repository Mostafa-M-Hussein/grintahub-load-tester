//! Rate control module
//!
//! Provides rate limiting and delay control for sessions.

mod limiter;

pub use limiter::{RateLimiter, RateLimiterConfig};
