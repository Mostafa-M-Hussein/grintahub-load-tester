//! Schedule runner
//!
//! Manages scheduled start/stop of bot operations.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use chrono::{Local, NaiveTime, Weekday, Datelike};
use tokio::sync::RwLock;
use tracing::{info, debug};

/// Schedule configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleConfig {
    /// Enable scheduling
    pub enabled: bool,
    /// Start time (HH:MM format)
    pub start_time: String,
    /// End time (HH:MM format)
    pub end_time: String,
    /// Days of the week to run (0 = Monday, 6 = Sunday)
    pub days: Vec<u8>,
    /// Optional cron expression (overrides start_time/end_time if set)
    pub cron_expression: Option<String>,
}

impl Default for ScheduleConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            start_time: "09:00".to_string(),
            end_time: "18:00".to_string(),
            days: vec![0, 1, 2, 3, 4], // Monday to Friday
            cron_expression: None,
        }
    }
}

impl ScheduleConfig {
    /// Check if current time is within the scheduled window
    pub fn is_within_schedule(&self) -> bool {
        if !self.enabled {
            return true; // If scheduling disabled, always allow
        }

        let now = Local::now();

        // Check day of week
        let today = match now.weekday() {
            Weekday::Mon => 0,
            Weekday::Tue => 1,
            Weekday::Wed => 2,
            Weekday::Thu => 3,
            Weekday::Fri => 4,
            Weekday::Sat => 5,
            Weekday::Sun => 6,
        };

        if !self.days.contains(&today) {
            debug!("Today ({}) not in scheduled days: {:?}", today, self.days);
            return false;
        }

        // Parse start and end times
        let start = match NaiveTime::parse_from_str(&self.start_time, "%H:%M") {
            Ok(t) => t,
            Err(_) => {
                debug!("Invalid start time format: {}", self.start_time);
                return true;
            }
        };

        let end = match NaiveTime::parse_from_str(&self.end_time, "%H:%M") {
            Ok(t) => t,
            Err(_) => {
                debug!("Invalid end time format: {}", self.end_time);
                return true;
            }
        };

        let current_time = now.time();

        // Handle overnight schedules (e.g., 22:00 - 06:00)
        if start > end {
            return current_time >= start || current_time <= end;
        }

        current_time >= start && current_time <= end
    }

    /// Get time until schedule starts (in seconds)
    pub fn time_until_start(&self) -> Option<i64> {
        if !self.enabled {
            return None;
        }

        let now = Local::now();
        let current_time = now.time();

        let start = NaiveTime::parse_from_str(&self.start_time, "%H:%M").ok()?;

        if current_time < start {
            let duration = start - current_time;
            Some(duration.num_seconds())
        } else {
            // Already past start time, return time until tomorrow's start
            let remaining_today = NaiveTime::from_hms_opt(23, 59, 59)? - current_time;
            let start_tomorrow = start - NaiveTime::from_hms_opt(0, 0, 0)?;
            Some(remaining_today.num_seconds() + start_tomorrow.num_seconds() + 1)
        }
    }

    /// Get time until schedule ends (in seconds)
    pub fn time_until_end(&self) -> Option<i64> {
        if !self.enabled {
            return None;
        }

        let now = Local::now();
        let current_time = now.time();

        let end = NaiveTime::parse_from_str(&self.end_time, "%H:%M").ok()?;

        if current_time < end {
            let duration = end - current_time;
            Some(duration.num_seconds())
        } else {
            None // Already past end time
        }
    }
}

/// Schedule status
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ScheduleStatus {
    Disabled,
    WaitingForStart { seconds_until: i64 },
    Running { seconds_until_end: Option<i64> },
    OutsideSchedule,
}

/// Scheduler for managing bot operation times
pub struct Scheduler {
    config: Arc<RwLock<ScheduleConfig>>,
    running: Arc<AtomicBool>,
}

impl Scheduler {
    /// Create a new scheduler
    pub fn new() -> Self {
        Self {
            config: Arc::new(RwLock::new(ScheduleConfig::default())),
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Create with initial config
    pub fn with_config(config: ScheduleConfig) -> Self {
        Self {
            config: Arc::new(RwLock::new(config)),
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Update configuration
    pub async fn set_config(&self, config: ScheduleConfig) {
        *self.config.write().await = config;
    }

    /// Get current configuration
    pub async fn get_config(&self) -> ScheduleConfig {
        self.config.read().await.clone()
    }

    /// Check if bot should be running according to schedule
    pub async fn should_run(&self) -> bool {
        let config = self.config.read().await;
        config.is_within_schedule()
    }

    /// Get current schedule status
    pub async fn status(&self) -> ScheduleStatus {
        let config = self.config.read().await;

        if !config.enabled {
            return ScheduleStatus::Disabled;
        }

        if config.is_within_schedule() {
            ScheduleStatus::Running {
                seconds_until_end: config.time_until_end(),
            }
        } else {
            if let Some(seconds) = config.time_until_start() {
                ScheduleStatus::WaitingForStart {
                    seconds_until: seconds,
                }
            } else {
                ScheduleStatus::OutsideSchedule
            }
        }
    }

    /// Start the scheduler monitoring loop
    pub async fn start_monitor<F, Fut>(&self, on_start: F, on_stop: F)
    where
        F: Fn() -> Fut + Send + Sync + Clone + 'static,
        Fut: std::future::Future<Output = ()> + Send,
    {
        info!("Starting schedule monitor");
        self.running.store(true, Ordering::Relaxed);

        let config = self.config.clone();
        let running = self.running.clone();
        let on_start = on_start.clone();
        let on_stop = on_stop.clone();

        tokio::spawn(async move {
            let mut was_in_schedule = false;

            while running.load(Ordering::Relaxed) {
                let is_in_schedule = {
                    let cfg = config.read().await;
                    cfg.is_within_schedule()
                };

                // Detect transitions
                if is_in_schedule && !was_in_schedule {
                    info!("Schedule started - triggering start callback");
                    on_start().await;
                } else if !is_in_schedule && was_in_schedule {
                    info!("Schedule ended - triggering stop callback");
                    on_stop().await;
                }

                was_in_schedule = is_in_schedule;

                // Check every minute
                tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
            }

            info!("Schedule monitor stopped");
        });
    }

    /// Stop the scheduler monitor
    pub fn stop_monitor(&self) {
        self.running.store(false, Ordering::Relaxed);
    }

    /// Check if monitor is running
    pub fn is_monitoring(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schedule_config_default() {
        let config = ScheduleConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.start_time, "09:00");
        assert_eq!(config.end_time, "18:00");
        assert_eq!(config.days, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn test_disabled_schedule_always_allows() {
        let config = ScheduleConfig {
            enabled: false,
            ..Default::default()
        };
        assert!(config.is_within_schedule());
    }
}
