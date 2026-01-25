//! Proxy module for Oxylabs proxy rotation
//!
//! Provides thread-safe proxy URL generation with unique session IDs per browser.
//! Includes a local proxy forwarder to handle Chrome's proxy authentication limitations.

mod oxylabs;
mod config;
mod forwarder;

pub use oxylabs::{OxylabsProxyGenerator, ProxyInfo};
pub use config::ProxyConfig;
pub use forwarder::{LocalProxyForwarder, allocate_port};

use parking_lot::RwLock;
use tracing::info;

/// Global Proxy Manager - Centralized proxy rotation for all browser sessions
///
/// This manager wraps `OxylabsProxyGenerator` and provides a unified interface
/// for obtaining fresh proxy URLs across all browser sessions.
pub struct GlobalProxyManager {
    inner: RwLock<ProxyManagerInner>,
}

struct ProxyManagerInner {
    generator: OxylabsProxyGenerator,
    enabled: bool,
    verified: bool,
}

impl std::fmt::Debug for GlobalProxyManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inner = self.inner.read();
        f.debug_struct("GlobalProxyManager")
            .field("enabled", &inner.enabled)
            .finish()
    }
}

impl GlobalProxyManager {
    /// Create a new GlobalProxyManager with the given configuration
    pub fn new(config: ProxyConfig) -> Self {
        let enabled = config.is_configured();
        let generator = OxylabsProxyGenerator::new(config);

        info!("GlobalProxyManager initialized (enabled: {})", enabled);

        Self {
            inner: RwLock::new(ProxyManagerInner { generator, enabled, verified: false }),
        }
    }

    /// Create a disabled proxy manager
    pub fn disabled() -> Self {
        let config = ProxyConfig::default();
        Self {
            inner: RwLock::new(ProxyManagerInner {
                generator: OxylabsProxyGenerator::new(config),
                enabled: false,
                verified: false,
            }),
        }
    }

    /// Create from individual credentials
    pub fn from_credentials(customer: &str, password: &str, country: Option<&str>, sesstime: Option<u16>) -> Self {
        let mut config = ProxyConfig::new(customer, password);
        if let Some(cc) = country {
            config = config.with_country(cc);
        }
        if let Some(st) = sesstime {
            config = config.with_sesstime(st);
        }
        Self::new(config)
    }

    /// Configure the proxy at runtime
    pub fn configure(&self, customer: &str, password: &str, country: Option<&str>, sesstime: Option<u16>) {
        let mut config = ProxyConfig::new(customer, password);
        if let Some(cc) = country {
            config = config.with_country(cc);
        }
        if let Some(st) = sesstime {
            config = config.with_sesstime(st);
        }
        let enabled = config.is_configured();
        let generator = OxylabsProxyGenerator::new(config);

        let mut inner = self.inner.write();
        inner.generator = generator;
        inner.enabled = enabled;
        inner.verified = false; // Reset verified when reconfigured

        info!("GlobalProxyManager reconfigured (enabled: {})", enabled);
    }

    /// Disable the proxy at runtime
    pub fn disable(&self) {
        let mut inner = self.inner.write();
        inner.enabled = false;
        info!("GlobalProxyManager disabled");
    }

    /// Get the next unique proxy URL
    pub fn next_proxy(&self) -> Option<String> {
        let inner = self.inner.read();
        if !inner.enabled {
            return None;
        }
        Some(inner.generator.next())
    }

    /// Get a batch of unique proxy URLs (one per browser session)
    pub fn next_batch(&self, count: usize) -> Option<Vec<String>> {
        let inner = self.inner.read();
        if !inner.enabled {
            return None;
        }
        Some(inner.generator.next_batch(count))
    }

    /// Check if proxy rotation is enabled
    pub fn is_enabled(&self) -> bool {
        self.inner.read().enabled
    }

    /// Check if proxy is properly configured
    pub fn is_configured(&self) -> bool {
        self.inner.read().generator.is_configured()
    }

    /// Get detailed proxy info for debugging
    pub fn next_proxy_info(&self) -> Option<ProxyInfo> {
        let inner = self.inner.read();
        if !inner.enabled {
            return None;
        }
        Some(inner.generator.next_with_info())
    }

    /// Check if proxy has been verified (tested successfully)
    pub fn is_verified(&self) -> bool {
        self.inner.read().verified
    }

    /// Set verified status
    pub fn set_verified(&self, verified: bool) {
        self.inner.write().verified = verified;
        info!("Proxy verified status: {}", verified);
    }
}
