//! Statistics module
//!
//! Lock-free statistics tracking using atomic operations.

mod atomic;

pub use atomic::{SessionStats, GlobalStats, SessionStatsSnapshot, GlobalStatsSnapshot};
