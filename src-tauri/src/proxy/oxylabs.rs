//! Oxylabs Proxy Generator
//!
//! Thread-safe proxy URL generator with unique session IDs per browser.
//! Each browser session gets a unique sessid to ensure a unique IP address.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::debug;
use urlencoding::encode;

use super::ProxyConfig;

/// Global atomic counter for unique session IDs (thread-safe)
static SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Oxylabs Proxy Generator
///
/// Generates unique proxy URLs with rotating session IDs.
/// Each call to `next()` returns a proxy URL with a unique session ID,
/// ensuring each browser gets a different IP address.
#[derive(Debug)]
pub struct OxylabsProxyGenerator {
    config: ProxyConfig,
    /// Base seed for session ID generation
    base_seed: u64,
}

impl OxylabsProxyGenerator {
    /// Create a new proxy generator
    pub fn new(config: ProxyConfig) -> Self {
        // Generate base seed from timestamp and process ID for uniqueness
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let pid = std::process::id() as u64;
        let base_seed = (timestamp % 1_000_000) * 1_000_000 + (pid % 1_000_000);

        debug!(
            "ProxyGenerator initialized: customer={}, country={}, base_seed={}",
            config.customer, config.country, base_seed
        );

        Self { config, base_seed }
    }

    /// Allocate a unique session ID
    fn allocate_sessid(&self) -> u64 {
        let counter = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
        self.base_seed + counter
    }

    /// Build the Oxylabs username with parameters
    fn build_username(&self, sessid: u64) -> String {
        format!(
            "customer-{}-cc-{}-sessid-{}-sesstime-{}",
            self.config.customer, self.config.country, sessid, self.config.sesstime
        )
    }

    /// Generate the next unique proxy URL
    ///
    /// Format: `{scheme}://{username}:{password}@{host}:{port}`
    pub fn next(&self) -> String {
        let sessid = self.allocate_sessid();
        let username = self.build_username(sessid);
        let password_encoded = encode(&self.config.password);

        let proxy_url = format!(
            "{}://{}:{}@{}:{}",
            self.config.scheme, username, password_encoded, self.config.host, self.config.port
        );

        debug!("Generated proxy URL with sessid={}", sessid);
        proxy_url
    }

    /// Generate a batch of unique proxy URLs
    pub fn next_batch(&self, count: usize) -> Vec<String> {
        (0..count).map(|_| self.next()).collect()
    }

    /// Get detailed proxy info
    pub fn next_with_info(&self) -> ProxyInfo {
        let sessid = self.allocate_sessid();
        let username = self.build_username(sessid);
        let password_encoded = encode(&self.config.password);

        let proxy_url = format!(
            "{}://{}:{}@{}:{}",
            self.config.scheme, username, password_encoded, self.config.host, self.config.port
        );

        ProxyInfo {
            proxy_url,
            session_id: sessid,
            username,
            host: self.config.host.clone(),
            port: self.config.port,
            scheme: self.config.scheme.clone(),
            country: self.config.country.clone(),
            sesstime: self.config.sesstime,
        }
    }

    /// Check if the generator is configured
    pub fn is_configured(&self) -> bool {
        self.config.is_configured()
    }
}

/// Detailed proxy information
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyInfo {
    pub proxy_url: String,
    pub session_id: u64,
    pub username: String,
    pub host: String,
    pub port: u16,
    pub scheme: String,
    pub country: String,
    pub sesstime: u16,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proxy_url_generation() {
        let config = ProxyConfig::new("testcustomer", "testpassword123");
        let generator = OxylabsProxyGenerator::new(config);

        let url1 = generator.next();
        let url2 = generator.next();

        // URLs should be different (different session IDs)
        assert_ne!(url1, url2);

        // Both should contain the required parts
        assert!(url1.contains("socks5h://"));
        assert!(url1.contains("customer-testcustomer"));
        assert!(url1.contains("cc-sa"));
        assert!(url1.contains("sessid-"));
    }

    #[test]
    fn test_unique_session_ids() {
        let config = ProxyConfig::new("test", "pass");
        let generator = OxylabsProxyGenerator::new(config);

        let mut session_ids: Vec<u64> = Vec::new();
        for _ in 0..100 {
            let info = generator.next_with_info();
            session_ids.push(info.session_id);
        }

        // All session IDs should be unique
        let unique_count = session_ids.iter().collect::<std::collections::HashSet<_>>().len();
        assert_eq!(unique_count, 100);
    }
}
