//! Oxylabs Proxy Generator
//!
//! Thread-safe proxy URL generator with unique session IDs per browser.
//! Each browser session gets a unique sessid to ensure a unique IP address.

use std::collections::HashSet;
use std::sync::Mutex;
use tracing::debug;
use urlencoding::encode;
use rand::Rng;

use super::ProxyConfig;

/// Global set of used session IDs to guarantee no reuse (thread-safe)
static USED_SESSIDS: std::sync::LazyLock<Mutex<HashSet<u64>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashSet::new()));

/// Oxylabs Proxy Generator
///
/// Generates unique proxy URLs with RANDOM session IDs.
/// Each call to `next()` returns a proxy URL with a unique random session ID,
/// ensuring each browser gets a different IP address.
/// Session IDs are never reused across the lifetime of the application.
#[derive(Debug)]
pub struct OxylabsProxyGenerator {
    config: ProxyConfig,
}

impl OxylabsProxyGenerator {
    /// Create a new proxy generator
    pub fn new(config: ProxyConfig) -> Self {
        debug!(
            "ProxyGenerator initialized: customer={}, country={} (random sessid mode)",
            config.customer, config.country
        );

        Self { config }
    }

    /// Allocate a unique RANDOM session ID (never reused)
    fn allocate_sessid(&self) -> u64 {
        let mut rng = rand::thread_rng();
        // Recover from poison (another thread panicked while holding lock)
        let mut used = USED_SESSIDS.lock().unwrap_or_else(|e| e.into_inner());

        loop {
            // Generate random sessid in a large range for maximum IP diversity
            let sessid: u64 = rng.gen_range(100_000_000..999_999_999);
            if used.insert(sessid) {
                debug!("Allocated random sessid: {}", sessid);
                return sessid;
            }
            // Collision (astronomically unlikely) - try again
        }
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
        assert!(url1.contains("://"));
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
