//! Lock-free statistics using atomic operations
//!
//! Provides high-performance statistics tracking without mutex contention.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Per-session statistics
#[derive(Debug, Default)]
pub struct SessionStats {
    pub clicks: AtomicU64,
    pub success: AtomicU64,
    pub errors: AtomicU64,
    pub total_latency_ms: AtomicU64,
    pub start_time: AtomicU64,
}

impl SessionStats {
    /// Create new session stats
    pub fn new() -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Self {
            clicks: AtomicU64::new(0),
            success: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            total_latency_ms: AtomicU64::new(0),
            start_time: AtomicU64::new(now),
        }
    }

    /// Record a successful click
    pub fn record_click(&self, latency_ms: u64) {
        self.clicks.fetch_add(1, Ordering::Relaxed);
        self.success.fetch_add(1, Ordering::Relaxed);
        self.total_latency_ms.fetch_add(latency_ms, Ordering::Relaxed);
    }

    /// Record an error
    pub fn record_error(&self) {
        self.clicks.fetch_add(1, Ordering::Relaxed);
        self.errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Get click count
    pub fn click_count(&self) -> u64 {
        self.clicks.load(Ordering::Relaxed)
    }

    /// Get success count
    pub fn success_count(&self) -> u64 {
        self.success.load(Ordering::Relaxed)
    }

    /// Get error count
    pub fn error_count(&self) -> u64 {
        self.errors.load(Ordering::Relaxed)
    }

    /// Get average latency in milliseconds
    pub fn average_latency_ms(&self) -> f64 {
        let success = self.success.load(Ordering::Relaxed);
        if success == 0 {
            return 0.0;
        }
        self.total_latency_ms.load(Ordering::Relaxed) as f64 / success as f64
    }

    /// Get success rate (0.0 - 1.0)
    pub fn success_rate(&self) -> f64 {
        let total = self.clicks.load(Ordering::Relaxed);
        if total == 0 {
            return 1.0;
        }
        self.success.load(Ordering::Relaxed) as f64 / total as f64
    }

    /// Get clicks per hour
    pub fn clicks_per_hour(&self) -> f64 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let start = self.start_time.load(Ordering::Relaxed);
        let elapsed_hours = (now - start) as f64 / 3600.0;

        if elapsed_hours < 0.001 {
            return 0.0;
        }

        self.clicks.load(Ordering::Relaxed) as f64 / elapsed_hours
    }

    /// Reset statistics
    pub fn reset(&self) {
        self.clicks.store(0, Ordering::Relaxed);
        self.success.store(0, Ordering::Relaxed);
        self.errors.store(0, Ordering::Relaxed);
        self.total_latency_ms.store(0, Ordering::Relaxed);

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.start_time.store(now, Ordering::Relaxed);
    }

    /// Get snapshot for serialization
    pub fn snapshot(&self) -> SessionStatsSnapshot {
        SessionStatsSnapshot {
            clicks: self.clicks.load(Ordering::Relaxed),
            success: self.success.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
            average_latency_ms: self.average_latency_ms(),
            success_rate: self.success_rate(),
            clicks_per_hour: self.clicks_per_hour(),
        }
    }
}

/// Serializable snapshot of session stats
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStatsSnapshot {
    pub clicks: u64,
    pub success: u64,
    pub errors: u64,
    pub average_latency_ms: f64,
    pub success_rate: f64,
    pub clicks_per_hour: f64,
}

/// Global statistics aggregated across all sessions
#[derive(Debug, Default)]
pub struct GlobalStats {
    pub total_clicks: AtomicU64,
    pub total_success: AtomicU64,
    pub total_errors: AtomicU64,
    pub total_latency_ms: AtomicU64,
    pub active_sessions: AtomicU64,
    pub start_time: AtomicU64,
}

impl GlobalStats {
    /// Create new global stats
    pub fn new() -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Self {
            total_clicks: AtomicU64::new(0),
            total_success: AtomicU64::new(0),
            total_errors: AtomicU64::new(0),
            total_latency_ms: AtomicU64::new(0),
            active_sessions: AtomicU64::new(0),
            start_time: AtomicU64::new(now),
        }
    }

    /// Record a successful click
    pub fn record_click(&self, latency_ms: u64) {
        self.total_clicks.fetch_add(1, Ordering::Relaxed);
        self.total_success.fetch_add(1, Ordering::Relaxed);
        self.total_latency_ms.fetch_add(latency_ms, Ordering::Relaxed);
    }

    /// Record an error
    pub fn record_error(&self) {
        self.total_clicks.fetch_add(1, Ordering::Relaxed);
        self.total_errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment active sessions
    pub fn add_session(&self) {
        self.active_sessions.fetch_add(1, Ordering::Relaxed);
    }

    /// Decrement active sessions
    pub fn remove_session(&self) {
        self.active_sessions.fetch_sub(1, Ordering::Relaxed);
    }

    /// Get total clicks
    pub fn total_clicks(&self) -> u64 {
        self.total_clicks.load(Ordering::Relaxed)
    }

    /// Get active session count
    pub fn active_sessions(&self) -> u64 {
        self.active_sessions.load(Ordering::Relaxed)
    }

    /// Get average latency
    pub fn average_latency_ms(&self) -> f64 {
        let success = self.total_success.load(Ordering::Relaxed);
        if success == 0 {
            return 0.0;
        }
        self.total_latency_ms.load(Ordering::Relaxed) as f64 / success as f64
    }

    /// Get total clicks per hour
    pub fn clicks_per_hour(&self) -> f64 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let start = self.start_time.load(Ordering::Relaxed);
        let elapsed_hours = (now - start) as f64 / 3600.0;

        if elapsed_hours < 0.001 {
            return 0.0;
        }

        self.total_clicks.load(Ordering::Relaxed) as f64 / elapsed_hours
    }

    /// Get snapshot for serialization
    pub fn snapshot(&self) -> GlobalStatsSnapshot {
        GlobalStatsSnapshot {
            total_clicks: self.total_clicks.load(Ordering::Relaxed),
            total_success: self.total_success.load(Ordering::Relaxed),
            total_errors: self.total_errors.load(Ordering::Relaxed),
            average_latency_ms: self.average_latency_ms(),
            clicks_per_hour: self.clicks_per_hour(),
            active_sessions: self.active_sessions.load(Ordering::Relaxed),
        }
    }

    /// Reset all stats
    pub fn reset(&self) {
        self.total_clicks.store(0, Ordering::Relaxed);
        self.total_success.store(0, Ordering::Relaxed);
        self.total_errors.store(0, Ordering::Relaxed);
        self.total_latency_ms.store(0, Ordering::Relaxed);

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.start_time.store(now, Ordering::Relaxed);
    }
}

/// Serializable snapshot of global stats
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalStatsSnapshot {
    pub total_clicks: u64,
    pub total_success: u64,
    pub total_errors: u64,
    pub average_latency_ms: f64,
    pub clicks_per_hour: f64,
    pub active_sessions: u64,
}
