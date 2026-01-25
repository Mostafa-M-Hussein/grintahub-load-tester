//! Proxy configuration

/// Default Oxylabs proxy host
pub const DEFAULT_HOST: &str = "pr.oxylabs.io";
/// Default port for HTTP proxy (more reliable for browsers with auth)
pub const DEFAULT_PORT: u16 = 60000;
/// Default session time in minutes
pub const DEFAULT_SESSTIME: u16 = 10;

/// Proxy configuration for Oxylabs
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProxyConfig {
    /// Oxylabs customer ID
    pub customer: String,
    /// Oxylabs password
    pub password: String,
    /// Proxy host (default: pr.oxylabs.io)
    pub host: String,
    /// Proxy port (default: 7777)
    pub port: u16,
    /// Country code (default: sa for Saudi Arabia)
    pub country: String,
    /// Session time in minutes (default: 10)
    pub sesstime: u16,
    /// Proxy scheme (socks5h, socks5, http, https)
    pub scheme: String,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        // Use HTTP proxy by default (port 60000) - more reliable for browsers with authentication
        let scheme = std::env::var("PROXY_SCHEME").unwrap_or_else(|_| "http".to_string());

        Self {
            customer: String::new(),
            password: String::new(),
            host: DEFAULT_HOST.to_string(),
            port: DEFAULT_PORT,
            country: "sa".to_string(), // Saudi Arabia by default
            sesstime: DEFAULT_SESSTIME,
            scheme,
        }
    }
}

impl ProxyConfig {
    /// Create a new proxy configuration
    pub fn new(customer: &str, password: &str) -> Self {
        Self {
            customer: customer.to_string(),
            password: password.to_string(),
            ..Default::default()
        }
    }

    /// Set the country code
    pub fn with_country(mut self, country: &str) -> Self {
        self.country = country.to_lowercase();
        self
    }

    /// Set the session time in minutes
    pub fn with_sesstime(mut self, minutes: u16) -> Self {
        self.sesstime = minutes;
        self
    }

    /// Set the proxy scheme
    pub fn with_scheme(mut self, scheme: &str) -> Self {
        self.scheme = scheme.to_lowercase();
        self
    }

    /// Set to HTTP proxy mode (port 60000)
    pub fn with_http_mode(mut self) -> Self {
        self.scheme = "http".to_string();
        self.port = 60000;
        self
    }

    /// Check if proxy is configured
    pub fn is_configured(&self) -> bool {
        !self.customer.is_empty() && !self.password.is_empty()
    }
}
