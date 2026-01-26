//! Session supervisor module
//!
//! Monitors active session count and auto-recovers missing sessions.
//! Also handles zombie Chrome process cleanup.

mod monitor;
mod zombie;

pub use monitor::{SessionSupervisor, SupervisorConfig};
pub use zombie::cleanup_zombie_chromes;
