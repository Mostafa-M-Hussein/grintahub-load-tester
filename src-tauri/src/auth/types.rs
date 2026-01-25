//! Authentication types for GrintaHub
//!
//! Models for registration, login, and account management.

use serde::{Deserialize, Serialize};

/// GrintaHub account representation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: Option<i64>,
    pub name: String,
    pub email: String,
    pub phone: Option<String>,
    pub token: Option<String>,
    pub cookies: Option<String>,
}

/// Registration request for GrintaHub
#[derive(Debug, Serialize)]
pub struct RegisterRequest {
    pub name: String,
    pub email: String,
    pub email_confirmation: String,
    pub phone_number: String,
    pub password: String,
    pub password_confirmation: String,
    #[serde(rename = "g-recaptcha-response")]
    pub captcha_token: String,
}

impl RegisterRequest {
    pub fn new(name: &str, email: &str, phone: &str, password: &str, captcha_token: &str) -> Self {
        Self {
            name: name.to_string(),
            email: email.to_string(),
            email_confirmation: email.to_string(),
            phone_number: phone.to_string(),
            password: password.to_string(),
            password_confirmation: password.to_string(),
            captcha_token: captcha_token.to_string(),
        }
    }
}

/// Login request for GrintaHub
#[derive(Debug, Serialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
    #[serde(rename = "g-recaptcha-response")]
    pub captcha_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remember: Option<bool>,
}

impl LoginRequest {
    pub fn new(email: &str, password: &str, captcha_token: &str) -> Self {
        Self {
            email: email.to_string(),
            password: password.to_string(),
            captcha_token: captcha_token.to_string(),
            remember: Some(true),
        }
    }
}

/// Authentication response from GrintaHub
#[derive(Debug, Deserialize)]
pub struct AuthResponse {
    pub success: Option<bool>,
    pub message: Option<String>,
    pub redirect: Option<String>,
    pub errors: Option<serde_json::Value>,
    pub user: Option<UserData>,
}

/// User data in auth response
#[derive(Debug, Deserialize)]
pub struct UserData {
    pub id: Option<i64>,
    pub name: Option<String>,
    pub email: Option<String>,
}

/// Authentication error types
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("Registration failed: {0}")]
    RegistrationFailed(String),

    #[error("Login failed: {0}")]
    LoginFailed(String),

    #[error("Invalid credentials")]
    InvalidCredentials,

    #[error("CAPTCHA required")]
    CaptchaRequired,

    #[error("Rate limited")]
    RateLimited,

    #[error("Network error: {0}")]
    NetworkError(String),

    #[error("Invalid response: {0}")]
    InvalidResponse(String),
}

/// Fake data generator for registration
pub struct FakeData;

impl FakeData {
    /// Generate a random name (English)
    pub fn random_name() -> String {
        use rand::Rng;
        let first_names = [
            "Ahmed", "Mohammed", "Ali", "Omar", "Youssef", "Ibrahim", "Hassan",
            "Khaled", "Faisal", "Abdullah", "Nasser", "Saad", "Hamad", "Turki",
            "Fahad", "Saleh", "Saud", "Bandar", "Majed", "Waleed",
        ];
        let last_names = [
            "Al Saud", "Al Rashid", "Al Fahad", "Al Thani", "Al Maktoum",
            "Al Nahyan", "Al Sabah", "Al Khalifa", "Al Qasimi", "Al Nuaimi",
            "Abdullah", "Mohammed", "Ahmed", "Hassan", "Ibrahim",
        ];

        let mut rng = rand::thread_rng();
        format!(
            "{} {}",
            first_names[rng.gen_range(0..first_names.len())],
            last_names[rng.gen_range(0..last_names.len())]
        )
    }

    /// Generate a random email
    pub fn random_email() -> String {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        let domains = ["gmail.com", "outlook.com", "yahoo.com", "hotmail.com"];
        let random_str: String = (0..8)
            .map(|_| rng.sample(rand::distributions::Alphanumeric) as char)
            .collect();

        format!(
            "user{}{}@{}",
            random_str.to_lowercase(),
            rng.gen_range(100..999),
            domains[rng.gen_range(0..domains.len())]
        )
    }

    /// Generate a random Saudi phone number (local format without country code)
    pub fn random_saudi_phone() -> String {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        let prefixes = ["50", "53", "54", "55", "56", "57", "58", "59"];
        let prefix = prefixes[rng.gen_range(0..prefixes.len())];
        let number: u32 = rng.gen_range(1000000..9999999);
        // Format: 5XXXXXXXX (9 digits, no leading 0, no country code)
        format!("{}{}", prefix, number)
    }

    /// Generate a secure random password
    pub fn random_password() -> String {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        let chars: Vec<char> = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789!@#$%"
            .chars()
            .collect();

        let password: String = (0..12)
            .map(|_| chars[rng.gen_range(0..chars.len())])
            .collect();

        // Ensure it has at least one uppercase, lowercase, digit, and special char
        format!("{}Aa1!", password)
    }
}
