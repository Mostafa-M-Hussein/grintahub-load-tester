//! Lock-free statistics using atomic operations
//!
//! Provides high-performance statistics tracking without mutex contention.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// We use SeqCst for writes and Acquire for reads to ensure snapshot consistency.
/// This prevents transient states where success > clicks in a snapshot.
const WRITE_ORDER: Ordering = Ordering::SeqCst;
const READ_ORDER: Ordering = Ordering::SeqCst;

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
        self.clicks.fetch_add(1, WRITE_ORDER);
        self.success.fetch_add(1, WRITE_ORDER);
        self.total_latency_ms.fetch_add(latency_ms, WRITE_ORDER);
    }

    /// Record an error
    pub fn record_error(&self) {
        self.clicks.fetch_add(1, WRITE_ORDER);
        self.errors.fetch_add(1, WRITE_ORDER);
    }

    /// Get click count
    pub fn click_count(&self) -> u64 {
        self.clicks.load(READ_ORDER)
    }

    /// Get success count
    pub fn success_count(&self) -> u64 {
        self.success.load(READ_ORDER)
    }

    /// Get error count
    pub fn error_count(&self) -> u64 {
        self.errors.load(READ_ORDER)
    }

    /// Get average latency in milliseconds
    pub fn average_latency_ms(&self) -> f64 {
        let success = self.success.load(READ_ORDER);
        if success == 0 {
            return 0.0;
        }
        self.total_latency_ms.load(READ_ORDER) as f64 / success as f64
    }

    /// Get success rate (0.0 - 1.0)
    pub fn success_rate(&self) -> f64 {
        let total = self.clicks.load(READ_ORDER);
        if total == 0 {
            return 1.0;
        }
        let success = self.success.load(READ_ORDER);
        // Guard: success can never exceed total
        if success > total {
            return 1.0;
        }
        success as f64 / total as f64
    }

    /// Get successful clicks per hour
    pub fn clicks_per_hour(&self) -> f64 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let start = self.start_time.load(READ_ORDER);
        let elapsed_secs = now.saturating_sub(start);
        let elapsed_hours = elapsed_secs as f64 / 3600.0;

        if elapsed_hours < 0.001 {
            return 0.0;
        }

        self.success.load(READ_ORDER) as f64 / elapsed_hours
    }

    /// Reset statistics
    pub fn reset(&self) {
        self.clicks.store(0, WRITE_ORDER);
        self.success.store(0, WRITE_ORDER);
        self.errors.store(0, WRITE_ORDER);
        self.total_latency_ms.store(0, WRITE_ORDER);

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.start_time.store(now, WRITE_ORDER);
    }

    /// Get snapshot for serialization (consistent read)
    pub fn snapshot(&self) -> SessionStatsSnapshot {
        // Read all values with SeqCst to get a consistent view
        let clicks = self.clicks.load(READ_ORDER);
        let success = self.success.load(READ_ORDER);
        let errors = self.errors.load(READ_ORDER);
        let latency = self.total_latency_ms.load(READ_ORDER);
        let start = self.start_time.load(READ_ORDER);

        // Guard: ensure success + errors <= clicks
        let success = success.min(clicks);
        let errors = errors.min(clicks.saturating_sub(success));

        let avg_latency = if success == 0 { 0.0 } else { latency as f64 / success as f64 };
        let success_rate = if clicks == 0 { 1.0 } else { success as f64 / clicks as f64 };

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let elapsed_hours = now.saturating_sub(start) as f64 / 3600.0;
        let clicks_per_hour = if elapsed_hours < 0.001 { 0.0 } else { success as f64 / elapsed_hours };

        SessionStatsSnapshot {
            clicks,
            success,
            errors,
            average_latency_ms: avg_latency,
            success_rate,
            clicks_per_hour,
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
    pub total_ip_changes: AtomicU64,
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
            total_ip_changes: AtomicU64::new(0),
            start_time: AtomicU64::new(now),
        }
    }

    /// Set active session count directly (used by bot status)
    pub fn set_active_sessions(&self, count: u64) {
        self.active_sessions.store(count, WRITE_ORDER);
    }

    /// Record a successful click
    pub fn record_click(&self, latency_ms: u64) {
        self.total_clicks.fetch_add(1, WRITE_ORDER);
        self.total_success.fetch_add(1, WRITE_ORDER);
        self.total_latency_ms.fetch_add(latency_ms, WRITE_ORDER);
    }

    /// Record an error
    pub fn record_error(&self) {
        self.total_clicks.fetch_add(1, WRITE_ORDER);
        self.total_errors.fetch_add(1, WRITE_ORDER);
    }

    /// Record an IP change (session respawn)
    pub fn record_ip_change(&self) {
        self.total_ip_changes.fetch_add(1, WRITE_ORDER);
    }

    /// Get total IP changes
    pub fn total_ip_changes(&self) -> u64 {
        self.total_ip_changes.load(READ_ORDER)
    }

    /// Increment active sessions
    pub fn add_session(&self) {
        self.active_sessions.fetch_add(1, WRITE_ORDER);
    }

    /// Decrement active sessions (saturating — never underflows below 0)
    pub fn remove_session(&self) {
        // Use compare-exchange loop for saturating subtraction
        loop {
            let current = self.active_sessions.load(READ_ORDER);
            if current == 0 {
                return; // Already at 0, don't underflow
            }
            match self.active_sessions.compare_exchange_weak(
                current,
                current - 1,
                WRITE_ORDER,
                READ_ORDER,
            ) {
                Ok(_) => return,
                Err(_) => continue, // Retry on contention
            }
        }
    }

    /// Get total clicks
    pub fn total_clicks(&self) -> u64 {
        self.total_clicks.load(READ_ORDER)
    }

    /// Get total successful ad clicks
    pub fn total_success(&self) -> u64 {
        self.total_success.load(READ_ORDER)
    }

    /// Get active session count
    pub fn active_sessions(&self) -> u64 {
        self.active_sessions.load(READ_ORDER)
    }

    /// Get average latency
    pub fn average_latency_ms(&self) -> f64 {
        let success = self.total_success.load(READ_ORDER);
        if success == 0 {
            return 0.0;
        }
        self.total_latency_ms.load(READ_ORDER) as f64 / success as f64
    }

    /// Get successful ad clicks per hour
    pub fn clicks_per_hour(&self) -> f64 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let start = self.start_time.load(READ_ORDER);
        let elapsed_secs = now.saturating_sub(start);
        let elapsed_hours = elapsed_secs as f64 / 3600.0;

        if elapsed_hours < 0.001 {
            return 0.0;
        }

        self.total_success.load(READ_ORDER) as f64 / elapsed_hours
    }

    /// Get snapshot for serialization (consistent read)
    pub fn snapshot(&self) -> GlobalStatsSnapshot {
        // Read all values with SeqCst for consistency
        let total_clicks = self.total_clicks.load(READ_ORDER);
        let total_success = self.total_success.load(READ_ORDER);
        let total_errors = self.total_errors.load(READ_ORDER);
        let total_latency = self.total_latency_ms.load(READ_ORDER);
        let active_sessions = self.active_sessions.load(READ_ORDER);
        let start = self.start_time.load(READ_ORDER);

        // Guard: ensure success + errors <= total_clicks
        let total_success = total_success.min(total_clicks);
        let total_errors = total_errors.min(total_clicks.saturating_sub(total_success));

        let avg_latency = if total_success == 0 { 0.0 } else { total_latency as f64 / total_success as f64 };

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let elapsed_hours = now.saturating_sub(start) as f64 / 3600.0;
        let clicks_per_hour = if elapsed_hours < 0.001 { 0.0 } else { total_success as f64 / elapsed_hours };

        let total_ip_changes = self.total_ip_changes.load(READ_ORDER);

        GlobalStatsSnapshot {
            total_clicks,
            total_success,
            total_errors,
            average_latency_ms: avg_latency,
            clicks_per_hour,
            active_sessions,
            total_ip_changes,
        }
    }

    /// Reset all stats (call when bot starts fresh)
    pub fn reset(&self) {
        self.total_clicks.store(0, WRITE_ORDER);
        self.total_success.store(0, WRITE_ORDER);
        self.total_errors.store(0, WRITE_ORDER);
        self.total_latency_ms.store(0, WRITE_ORDER);
        self.total_ip_changes.store(0, WRITE_ORDER);
        // Don't reset active_sessions — that's managed separately

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.start_time.store(now, WRITE_ORDER);
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
    pub total_ip_changes: u64,
}
