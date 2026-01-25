//! Rate limiter with exponential backoff and jitter
//!
//! Controls the rate of clicks per session to simulate human behavior.

use std::time::{Duration, Instant};
use rand::Rng;
use tokio::time::sleep;
use tracing::debug;

/// Rate limiter configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimiterConfig {
    /// Target clicks per hour per session
    pub clicks_per_hour: u32,
    /// Minimum delay between actions in milliseconds
    pub min_delay_ms: u64,
    /// Maximum delay between actions in milliseconds
    pub max_delay_ms: u64,
    /// Add jitter to delays (percentage, 0-100)
    pub jitter_percent: u8,
}

impl Default for RateLimiterConfig {
    fn default() -> Self {
        Self {
            clicks_per_hour: 60,  // 1 click per minute by default
            min_delay_ms: 5000,   // 5 seconds minimum
            max_delay_ms: 30000,  // 30 seconds maximum
            jitter_percent: 20,   // 20% jitter
        }
    }
}

impl RateLimiterConfig {
    /// Calculate the base delay to achieve target clicks per hour
    pub fn base_delay_ms(&self) -> u64 {
        if self.clicks_per_hour == 0 {
            return self.max_delay_ms;
        }
        // milliseconds per hour / clicks per hour
        let ms_per_click = 3_600_000 / self.clicks_per_hour as u64;
        ms_per_click.clamp(self.min_delay_ms, self.max_delay_ms)
    }
}

/// Rate limiter for controlling session click rate
pub struct RateLimiter {
    config: RateLimiterConfig,
    last_action: Option<Instant>,
    consecutive_errors: u32,
}

impl RateLimiter {
    /// Create a new rate limiter with the given config
    pub fn new(config: RateLimiterConfig) -> Self {
        Self {
            config,
            last_action: None,
            consecutive_errors: 0,
        }
    }

    /// Update configuration
    pub fn set_config(&mut self, config: RateLimiterConfig) {
        self.config = config;
    }

    /// Get current configuration
    pub fn config(&self) -> &RateLimiterConfig {
        &self.config
    }

    /// Calculate the next delay with jitter
    fn calculate_delay(&self) -> Duration {
        let base_delay = self.config.base_delay_ms();

        // Add jitter
        let jitter_range = (base_delay as f64 * self.config.jitter_percent as f64 / 100.0) as u64;
        let jitter = if jitter_range > 0 {
            rand::thread_rng().gen_range(0..jitter_range * 2) as i64 - jitter_range as i64
        } else {
            0
        };

        let delay_with_jitter = (base_delay as i64 + jitter).max(self.config.min_delay_ms as i64) as u64;

        // Apply exponential backoff if there are consecutive errors
        let backoff_multiplier = if self.consecutive_errors > 0 {
            2u64.pow(self.consecutive_errors.min(5))
        } else {
            1
        };

        let final_delay = (delay_with_jitter * backoff_multiplier).min(self.config.max_delay_ms * 10);

        debug!(
            "Calculated delay: {}ms (base: {}ms, jitter: {}ms, backoff: {}x)",
            final_delay, base_delay, jitter, backoff_multiplier
        );

        Duration::from_millis(final_delay)
    }

    /// Wait for the appropriate delay before the next action
    pub async fn wait(&mut self) {
        let delay = self.calculate_delay();

        // If we have a last action time, calculate remaining wait
        if let Some(last) = self.last_action {
            let elapsed = last.elapsed();
            if elapsed < delay {
                let remaining = delay - elapsed;
                debug!("Rate limiter waiting {}ms", remaining.as_millis());
                sleep(remaining).await;
            }
        } else {
            // First action, apply full delay to stagger starts
            let initial_delay = rand::thread_rng().gen_range(0..delay.as_millis() as u64);
            debug!("Rate limiter initial delay {}ms", initial_delay);
            sleep(Duration::from_millis(initial_delay)).await;
        }

        self.last_action = Some(Instant::now());
    }

    /// Record a successful action (resets consecutive error count)
    pub fn record_success(&mut self) {
        self.consecutive_errors = 0;
    }

    /// Record a failed action (increases backoff)
    pub fn record_error(&mut self) {
        self.consecutive_errors = self.consecutive_errors.saturating_add(1);
    }

    /// Get the estimated time until next action
    pub fn time_until_next(&self) -> Duration {
        if let Some(last) = self.last_action {
            let delay = self.calculate_delay();
            let elapsed = last.elapsed();
            if elapsed < delay {
                return delay - elapsed;
            }
        }
        Duration::ZERO
    }

    /// Check if we can perform an action now
    pub fn can_act(&self) -> bool {
        self.time_until_next() == Duration::ZERO
    }
}

/// Calculate delay with exponential backoff and jitter (standalone function)
pub fn calculate_backoff_with_jitter(attempt: u32, base_ms: u64, max_ms: u64) -> Duration {
    let base_delay = base_ms * 2u64.pow(attempt.saturating_sub(1).min(5));
    let capped_delay = base_delay.min(max_ms);

    // Add ±20% jitter
    let jitter_range = capped_delay / 5;
    let jitter = if jitter_range > 0 {
        rand::thread_rng().gen_range(0..jitter_range * 2) as i64 - jitter_range as i64
    } else {
        0
    };

    Duration::from_millis((capped_delay as i64 + jitter).max(0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base_delay_calculation() {
        let config = RateLimiterConfig {
            clicks_per_hour: 60,
            min_delay_ms: 5000,
            max_delay_ms: 120000,
            jitter_percent: 0,
        };

        // 60 clicks/hour = 1 click/minute = 60000ms delay
        assert_eq!(config.base_delay_ms(), 60000);
    }

    #[test]
    fn test_base_delay_clamping() {
        let config = RateLimiterConfig {
            clicks_per_hour: 3600, // 1 click/second = 1000ms
            min_delay_ms: 5000,    // But min is 5000ms
            max_delay_ms: 30000,
            jitter_percent: 0,
        };

        assert_eq!(config.base_delay_ms(), 5000); // Clamped to min
    }

    #[test]
    fn test_backoff_with_jitter() {
        let delay1 = calculate_backoff_with_jitter(1, 100, 10000);
        let delay2 = calculate_backoff_with_jitter(2, 100, 10000);
        let delay3 = calculate_backoff_with_jitter(3, 100, 10000);

        // Each subsequent delay should be roughly double (with jitter)
        assert!(delay2.as_millis() > delay1.as_millis() / 2);
        assert!(delay3.as_millis() > delay2.as_millis() / 2);
    }
}
