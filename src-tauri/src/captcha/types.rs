//! CAPTCHA types and 2Captcha API models
//!
//! Based on ticketforge implementation for 2Captcha integration.

use serde::{Deserialize, Serialize};

/// Supported CAPTCHA types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptchaType {
    RecaptchaV2,
    RecaptchaV3,
    Turnstile,
    HCaptcha,
}

impl CaptchaType {
    /// Get 2Captcha method name
    pub fn as_2captcha_method(&self) -> &'static str {
        match self {
            Self::RecaptchaV2 => "RecaptchaV2TaskProxyless",
            Self::RecaptchaV3 => "RecaptchaV3TaskProxyless",
            Self::Turnstile => "TurnstileTaskProxyless",
            Self::HCaptcha => "HCaptchaTaskProxyless",
        }
    }

    /// Token time-to-live in seconds
    pub fn token_ttl_secs(&self) -> u64 {
        match self {
            Self::RecaptchaV2 => 120,
            Self::RecaptchaV3 => 120,
            Self::Turnstile => 300,
            Self::HCaptcha => 120,
        }
    }

    /// Safe TTL with margin for network delays
    pub fn safe_token_ttl_secs(&self) -> u64 {
        const SAFETY_MARGIN: u64 = 15;
        self.token_ttl_secs().saturating_sub(SAFETY_MARGIN)
    }
}

/// CAPTCHA solve request
#[derive(Debug, Clone)]
pub struct CaptchaRequest {
    pub captcha_type: CaptchaType,
    pub sitekey: String,
    pub page_url: String,
    pub action: Option<String>,
    pub min_score: Option<f64>,
    pub enterprise: bool,
}

impl CaptchaRequest {
    /// Create reCAPTCHA v2 request
    pub fn recaptcha_v2(sitekey: &str, page_url: &str) -> Self {
        Self {
            captcha_type: CaptchaType::RecaptchaV2,
            sitekey: sitekey.to_string(),
            page_url: page_url.to_string(),
            action: None,
            min_score: None,
            enterprise: false,
        }
    }

    /// Create reCAPTCHA v2 Enterprise request (Google /sorry/ pages use this)
    pub fn recaptcha_v2_enterprise(sitekey: &str, page_url: &str) -> Self {
        Self {
            captcha_type: CaptchaType::RecaptchaV2,
            sitekey: sitekey.to_string(),
            page_url: page_url.to_string(),
            action: None,
            min_score: None,
            enterprise: true,
        }
    }

    /// Create reCAPTCHA v3 request
    pub fn recaptcha_v3(sitekey: &str, page_url: &str, action: &str, min_score: f64) -> Self {
        Self {
            captcha_type: CaptchaType::RecaptchaV3,
            sitekey: sitekey.to_string(),
            page_url: page_url.to_string(),
            action: Some(action.to_string()),
            min_score: Some(min_score),
            enterprise: false,
        }
    }

    /// Create Turnstile request
    pub fn turnstile(sitekey: &str, page_url: &str) -> Self {
        Self {
            captcha_type: CaptchaType::Turnstile,
            sitekey: sitekey.to_string(),
            page_url: page_url.to_string(),
            action: None,
            min_score: None,
            enterprise: false,
        }
    }

    // ========== Pre-configured requests for GrintaHub ==========

    /// GrintaHub registration reCAPTCHA v3
    /// Sitekey: 6LfRMGAqAAAAADmJB37u6YTAMGcZ5OMzZg53wPCN
    pub fn grintahub_registration() -> Self {
        Self::recaptcha_v3(
            "6LfRMGAqAAAAADmJB37u6YTAMGcZ5OMzZg53wPCN",
            "https://grintahub.com/register",
            "generic",
            0.5,
        )
    }

    /// GrintaHub login reCAPTCHA v3
    pub fn grintahub_login() -> Self {
        Self::recaptcha_v3(
            "6LfRMGAqAAAAADmJB37u6YTAMGcZ5OMzZg53wPCN",
            "https://grintahub.com/login",
            "generic",
            0.5,
        )
    }
}

/// CAPTCHA solve result
#[derive(Debug, Clone)]
pub struct CaptchaResult {
    pub token: String,
    pub solve_time_ms: u64,
}

// ========== 2Captcha API Models ==========

/// 2Captcha create task request
#[derive(Debug, Serialize)]
pub struct TwoCaptchaCreateTask {
    #[serde(rename = "clientKey")]
    pub client_key: String,
    pub task: TwoCaptchaTask,
}

/// 2Captcha task types
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum TwoCaptchaTask {
    #[serde(rename = "RecaptchaV2TaskProxyless")]
    RecaptchaV2Proxyless {
        #[serde(rename = "websiteURL")]
        website_url: String,
        #[serde(rename = "websiteKey")]
        website_key: String,
    },

    #[serde(rename = "RecaptchaV2EnterpriseTaskProxyless")]
    RecaptchaV2EnterpriseProxyless {
        #[serde(rename = "websiteURL")]
        website_url: String,
        #[serde(rename = "websiteKey")]
        website_key: String,
    },

    #[serde(rename = "RecaptchaV3TaskProxyless")]
    RecaptchaV3Proxyless {
        #[serde(rename = "websiteURL")]
        website_url: String,
        #[serde(rename = "websiteKey")]
        website_key: String,
        #[serde(rename = "pageAction")]
        page_action: String,
        #[serde(rename = "minScore")]
        min_score: f64,
    },

    #[serde(rename = "TurnstileTaskProxyless")]
    TurnstileProxyless {
        #[serde(rename = "websiteURL")]
        website_url: String,
        #[serde(rename = "websiteKey")]
        website_key: String,
    },

    #[serde(rename = "HCaptchaTaskProxyless")]
    HCaptchaProxyless {
        #[serde(rename = "websiteURL")]
        website_url: String,
        #[serde(rename = "websiteKey")]
        website_key: String,
    },
}

/// 2Captcha create task response
#[derive(Debug, Deserialize)]
pub struct TwoCaptchaCreateResponse {
    #[serde(rename = "errorId")]
    pub error_id: i32,
    #[serde(rename = "errorCode")]
    pub error_code: Option<String>,
    #[serde(rename = "errorDescription")]
    pub error_description: Option<String>,
    #[serde(rename = "taskId")]
    pub task_id: Option<i64>,
}

/// 2Captcha get result request
#[derive(Debug, Serialize)]
pub struct TwoCaptchaGetResult {
    #[serde(rename = "clientKey")]
    pub client_key: String,
    #[serde(rename = "taskId")]
    pub task_id: i64,
}

/// 2Captcha get result response
#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct TwoCaptchaResultResponse {
    #[serde(rename = "errorId")]
    pub error_id: i32,
    #[serde(rename = "errorCode")]
    pub error_code: Option<String>,
    #[serde(rename = "errorDescription")]
    pub error_description: Option<String>,
    pub status: Option<String>,
    pub solution: Option<TwoCaptchaSolution>,
}

impl TwoCaptchaResultResponse {
    pub fn is_processing(&self) -> bool {
        self.status.as_deref() == Some("processing")
    }

    pub fn is_ready(&self) -> bool {
        self.status.as_deref() == Some("ready")
    }

    pub fn get_token(&self) -> Option<&str> {
        self.solution.as_ref().and_then(|s| {
            s.g_recaptcha_response.as_deref()
                .or(s.token.as_deref())
                .or(s.text.as_deref())
        })
    }
}

/// 2Captcha solution
#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct TwoCaptchaSolution {
    #[serde(rename = "gRecaptchaResponse")]
    pub g_recaptcha_response: Option<String>,
    pub token: Option<String>,
    pub text: Option<String>,
}

/// CAPTCHA error types
#[derive(Debug, thiserror::Error)]
pub enum CaptchaError {
    #[error("API key not configured")]
    ApiKeyMissing,

    #[error("2Captcha API error: {0}")]
    ApiError(String),

    #[error("Task creation failed: {0}")]
    TaskCreationFailed(String),

    #[error("Solve timeout after {0}s")]
    Timeout(u64),

    #[error("Network error: {0}")]
    NetworkError(String),

    #[error("Invalid response: {0}")]
    InvalidResponse(String),
}
