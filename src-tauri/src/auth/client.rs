//! GrintaHub authentication client
//!
//! Handles registration and login with CAPTCHA solving.
//! Properly handles Laravel CSRF tokens.

use std::time::Duration;
use reqwest::{Client, cookie::Jar};
use std::sync::Arc;
use tracing::{info, debug, warn};

use super::types::*;
use crate::captcha::{CaptchaSolver, CaptchaRequest};

/// GrintaHub base URL
const GRINTAHUB_URL: &str = "https://grintahub.com";

/// Authentication client for GrintaHub
pub struct AuthClient {
    client: Client,
    #[allow(dead_code)]
    cookie_jar: Arc<Jar>,
    #[allow(dead_code)]
    timeout_secs: u64,
}

impl AuthClient {
    /// Create a new auth client
    pub fn new(timeout_secs: u64) -> Result<Self, AuthError> {
        let cookie_jar = Arc::new(Jar::default());

        let client = Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .cookie_provider(cookie_jar.clone())
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .map_err(|e| AuthError::NetworkError(e.to_string()))?;

        Ok(Self {
            client,
            cookie_jar,
            timeout_secs,
        })
    }

    /// Extract CSRF token from HTML page
    fn extract_csrf_token(html: &str) -> Option<String> {
        // Try meta tag first: <meta name="csrf-token" content="...">
        if let Some(start) = html.find("name=\"csrf-token\"") {
            if let Some(content_start) = html[start..].find("content=\"") {
                let token_start = start + content_start + 9;
                if let Some(token_end) = html[token_start..].find('"') {
                    return Some(html[token_start..token_start + token_end].to_string());
                }
            }
        }

        // Try hidden input: <input type="hidden" name="_token" value="...">
        if let Some(start) = html.find("name=\"_token\"") {
            if let Some(value_start) = html[start..].find("value=\"") {
                let token_start = start + value_start + 7;
                if let Some(token_end) = html[token_start..].find('"') {
                    return Some(html[token_start..token_start + token_end].to_string());
                }
            }
        }

        // Try reverse order for input
        if let Some(start) = html.find("value=\"") {
            // Look for _token nearby
            let search_range = &html[start.saturating_sub(100)..html.len().min(start + 200)];
            if search_range.contains("_token") {
                let token_start = start + 7;
                if let Some(token_end) = html[token_start..].find('"') {
                    return Some(html[token_start..token_start + token_end].to_string());
                }
            }
        }

        None
    }

    /// Register a new account with CAPTCHA solving
    pub async fn register(
        &self,
        name: &str,
        email: &str,
        phone: &str,
        password: &str,
        captcha_token: &str,
    ) -> Result<Account, AuthError> {
        info!("Registering account: {}", email);

        // First, visit the registration page to get cookies and CSRF token
        let page_response = self.client
            .get(format!("{}/register", GRINTAHUB_URL))
            .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
            .header("Accept-Language", "en-US,en;q=0.9,ar;q=0.8")
            .send()
            .await
            .map_err(|e| AuthError::NetworkError(e.to_string()))?;

        let page_html = page_response.text().await
            .map_err(|e| AuthError::NetworkError(e.to_string()))?;

        info!("Registration page fetched, length: {} bytes", page_html.len());

        // Extract CSRF token
        let csrf_token = Self::extract_csrf_token(&page_html);
        info!("CSRF token found: {} (len={})", csrf_token.is_some(), csrf_token.as_ref().map(|t| t.len()).unwrap_or(0));

        // Build form data with CSRF token
        let mut form_data = vec![
            ("name", name.to_string()),
            ("email", email.to_string()),
            ("email_confirmation", email.to_string()),
            ("phone_number", phone.to_string()),
            ("password", password.to_string()),
            ("password_confirmation", password.to_string()),
            ("g-recaptcha-response", captcha_token.to_string()),
        ];

        // Add CSRF token if found
        if let Some(token) = &csrf_token {
            form_data.push(("_token", token.clone()));
        }

        // Submit registration form
        let response = self.client
            .post(format!("{}/register", GRINTAHUB_URL))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8")
            .header("Accept-Language", "en-US,en;q=0.9,ar;q=0.8")
            .header("Origin", GRINTAHUB_URL)
            .header("Referer", format!("{}/register", GRINTAHUB_URL))
            .form(&form_data)
            .send()
            .await
            .map_err(|e| AuthError::NetworkError(e.to_string()))?;

        let status = response.status();
        let response_url = response.url().to_string();
        let text = response.text().await
            .map_err(|e| AuthError::NetworkError(e.to_string()))?;

        // Log full response for debugging
        info!("Registration response status: {}", status);
        info!("Registration response URL: {}", response_url);
        info!("Registration response length: {} bytes", text.len());
        debug!("Registration response body: {}", crate::safe_truncate(&text, 2000));

        // Try to parse as JSON
        if let Ok(auth_response) = serde_json::from_str::<AuthResponse>(&text) {
            if auth_response.success == Some(true) || auth_response.redirect.is_some() {
                info!("Registration successful for: {}", email);
                return Ok(Account {
                    id: auth_response.user.as_ref().and_then(|u| u.id),
                    name: name.to_string(),
                    email: email.to_string(),
                    phone: Some(phone.to_string()),
                    token: None,
                    cookies: None,
                });
            }

            // Check for errors
            if let Some(errors) = auth_response.errors {
                let error_msg = if errors.is_object() {
                    errors.as_object()
                        .map(|obj| {
                            obj.values()
                                .filter_map(|v| v.as_array())
                                .flatten()
                                .filter_map(|v| v.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        })
                        .unwrap_or_else(|| errors.to_string())
                } else {
                    errors.to_string()
                };
                return Err(AuthError::RegistrationFailed(error_msg));
            }

            if let Some(message) = auth_response.message {
                return Err(AuthError::RegistrationFailed(message));
            }
        }

        // Check for redirect (successful registration often redirects)
        if status.is_redirection() || text.contains("redirect") || text.contains("success") {
            info!("Registration appears successful for: {}", email);
            return Ok(Account {
                id: None,
                name: name.to_string(),
                email: email.to_string(),
                phone: Some(phone.to_string()),
                token: None,
                cookies: None,
            });
        }

        // Look for validation errors in HTML response
        // Laravel typically uses "invalid-feedback", "alert-danger", "text-danger" classes
        let error_patterns = [
            // English errors
            ("already been taken", "Email already exists"),
            ("already registered", "Email already exists"),
            ("email has already", "Email already exists"),
            ("phone has already", "Phone already registered"),
            ("invalid email", "Invalid email format"),
            ("password must", "Password requirements not met"),
            ("passwords do not match", "Passwords do not match"),
            ("too many attempts", "Rate limited"),
            ("throttle", "Rate limited"),
            // Arabic errors
            ("البريد الإلكتروني مُستخدم", "Email already exists"),
            ("رقم الهاتف مُستخدم", "Phone already registered"),
            ("كلمة المرور غير متطابقة", "Passwords do not match"),
            // Captcha specific errors (only if they appear as validation messages)
            ("captcha validation failed", "CAPTCHA validation failed"),
            ("recaptcha verification failed", "CAPTCHA verification failed"),
            ("invalid captcha", "CAPTCHA validation failed"),
        ];

        // Check for specific error patterns
        let text_lower = text.to_lowercase();
        for (pattern, error_msg) in error_patterns {
            if text_lower.contains(pattern) {
                if error_msg.contains("Rate") {
                    return Err(AuthError::RateLimited);
                }
                if error_msg.contains("CAPTCHA") {
                    return Err(AuthError::CaptchaRequired);
                }
                return Err(AuthError::RegistrationFailed(error_msg.to_string()));
            }
        }

        // Look for Laravel validation error structure in HTML
        // Check for "is-invalid" class or error messages near form fields
        if text.contains("is-invalid") || text.contains("invalid-feedback") {
            // Try to extract the error message
            if let Some(start) = text.find("invalid-feedback") {
                let search_area = &text[start..text.len().min(start + 200)];
                if let Some(msg_start) = search_area.find('>') {
                    if let Some(msg_end) = search_area[msg_start..].find('<') {
                        let error_text = &search_area[msg_start + 1..msg_start + msg_end];
                        let error_text = error_text.trim();
                        if !error_text.is_empty() {
                            return Err(AuthError::RegistrationFailed(error_text.to_string()));
                        }
                    }
                }
            }
            return Err(AuthError::RegistrationFailed("Validation error (check form fields)".into()));
        }

        // If we got back the registration page without clear errors, it might be a session/cookie issue
        if text.contains("register-form") || text.contains("id=\"register") {
            // The form was returned again - likely the CAPTCHA wasn't accepted
            return Err(AuthError::RegistrationFailed("Form validation failed - CAPTCHA may have expired or been rejected".into()));
        }

        Err(AuthError::RegistrationFailed(format!("Unknown error (status {})", status)))
    }

    /// Login to an existing account with CAPTCHA solving
    pub async fn login(
        &self,
        email: &str,
        password: &str,
        captcha_token: &str,
    ) -> Result<Account, AuthError> {
        info!("Logging in: {}", email);

        // First, visit the login page to get cookies and CSRF token
        let page_response = self.client
            .get(format!("{}/login", GRINTAHUB_URL))
            .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
            .header("Accept-Language", "en-US,en;q=0.9,ar;q=0.8")
            .send()
            .await
            .map_err(|e| AuthError::NetworkError(e.to_string()))?;

        let page_html = page_response.text().await
            .map_err(|e| AuthError::NetworkError(e.to_string()))?;

        // Extract CSRF token
        let csrf_token = Self::extract_csrf_token(&page_html);
        debug!("CSRF token found: {}", csrf_token.is_some());

        // Build form data with CSRF token
        let mut form_data = vec![
            ("email", email.to_string()),
            ("password", password.to_string()),
            ("g-recaptcha-response", captcha_token.to_string()),
            ("remember", "1".to_string()),
        ];

        // Add CSRF token if found
        if let Some(token) = &csrf_token {
            form_data.push(("_token", token.clone()));
        }

        // Submit login form
        let response = self.client
            .post(format!("{}/login", GRINTAHUB_URL))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8")
            .header("Accept-Language", "en-US,en;q=0.9,ar;q=0.8")
            .header("Origin", GRINTAHUB_URL)
            .header("Referer", format!("{}/login", GRINTAHUB_URL))
            .form(&form_data)
            .send()
            .await
            .map_err(|e| AuthError::NetworkError(e.to_string()))?;

        let status = response.status();
        let text = response.text().await
            .map_err(|e| AuthError::NetworkError(e.to_string()))?;

        debug!("Login response ({}): {}", status, crate::safe_truncate(&text, 500));

        // Try to parse as JSON
        if let Ok(auth_response) = serde_json::from_str::<AuthResponse>(&text) {
            if auth_response.success == Some(true) || auth_response.redirect.is_some() {
                info!("Login successful for: {}", email);
                return Ok(Account {
                    id: auth_response.user.as_ref().and_then(|u| u.id),
                    name: auth_response.user.as_ref()
                        .and_then(|u| u.name.clone())
                        .unwrap_or_default(),
                    email: email.to_string(),
                    phone: None,
                    token: None,
                    cookies: None,
                });
            }

            if let Some(message) = auth_response.message {
                if message.contains("Invalid") || message.contains("incorrect") {
                    return Err(AuthError::InvalidCredentials);
                }
                return Err(AuthError::LoginFailed(message));
            }
        }

        // Check for redirect (successful login often redirects)
        if status.is_redirection() || text.contains("redirect") || text.contains("success") {
            info!("Login appears successful for: {}", email);
            return Ok(Account {
                id: None,
                name: String::new(),
                email: email.to_string(),
                phone: None,
                token: None,
                cookies: None,
            });
        }

        // Check for common error patterns
        if text.contains("Invalid") || text.contains("incorrect") || text.contains("wrong") {
            return Err(AuthError::InvalidCredentials);
        }
        if text.contains("captcha") || text.contains("recaptcha") {
            return Err(AuthError::CaptchaRequired);
        }
        if text.contains("rate") || text.contains("too many") || text.contains("blocked") {
            return Err(AuthError::RateLimited);
        }

        Err(AuthError::LoginFailed(format!("Unknown error: {}", crate::safe_truncate(&text, 200))))
    }

    /// Register with automatic CAPTCHA solving
    /// This method properly sequences: get session -> solve captcha -> submit
    pub async fn register_with_captcha(
        &self,
        captcha_solver: &CaptchaSolver,
        name: &str,
        email: &str,
        phone: &str,
        password: &str,
    ) -> Result<Account, AuthError> {
        info!("Starting registration with CAPTCHA for: {}", email);

        // Step 1: Visit registration page to establish session and get CSRF token
        let page_response = self.client
            .get(format!("{}/register", GRINTAHUB_URL))
            .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
            .header("Accept-Language", "en-US,en;q=0.9,ar;q=0.8")
            .send()
            .await
            .map_err(|e| AuthError::NetworkError(e.to_string()))?;

        let page_html = page_response.text().await
            .map_err(|e| AuthError::NetworkError(e.to_string()))?;

        info!("Registration page fetched, length: {} bytes", page_html.len());

        // Extract CSRF token
        let csrf_token = Self::extract_csrf_token(&page_html);
        info!("CSRF token found: {} (len={})", csrf_token.is_some(), csrf_token.as_ref().map(|t| t.len()).unwrap_or(0));

        // Step 2: Solve CAPTCHA (now that session is established)
        info!("Solving CAPTCHA for registration...");
        let captcha_request = CaptchaRequest::grintahub_registration();
        let captcha_result = captcha_solver.solve(&captcha_request)
            .await
            .map_err(|e| AuthError::RegistrationFailed(format!("CAPTCHA failed: {}", e)))?;

        info!("CAPTCHA solved in {}ms, token length: {}", captcha_result.solve_time_ms, captcha_result.token.len());

        // Step 3: Build and submit form
        // Format phone number - remove country code prefix if present, use just digits
        let phone_digits: String = phone.chars().filter(|c| c.is_ascii_digit()).collect();
        let phone_local = if phone_digits.starts_with("966") {
            phone_digits[3..].to_string()
        } else if phone_digits.starts_with("0") {
            phone_digits[1..].to_string()
        } else {
            phone_digits
        };

        let mut form_data = vec![
            ("name", name.to_string()),
            ("email", email.to_string()),
            ("email_confirmation", email.to_string()),
            ("phone_country", "sa".to_string()), // Hidden field for intlTelInput
            ("phone_number", phone_local),        // Local number without country code
            ("password", password.to_string()),
            ("password_confirmation", password.to_string()),
            ("g-recaptcha-response", captcha_result.token),
        ];

        // Add CSRF token if found
        if let Some(token) = &csrf_token {
            form_data.push(("_token", token.clone()));
        }

        // Submit registration form
        let response = self.client
            .post(format!("{}/register", GRINTAHUB_URL))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8")
            .header("Accept-Language", "en-US,en;q=0.9,ar;q=0.8")
            .header("Origin", GRINTAHUB_URL)
            .header("Referer", format!("{}/register", GRINTAHUB_URL))
            .form(&form_data)
            .send()
            .await
            .map_err(|e| AuthError::NetworkError(e.to_string()))?;

        let status = response.status();
        let response_url = response.url().to_string();
        let text = response.text().await
            .map_err(|e| AuthError::NetworkError(e.to_string()))?;

        info!("Registration response status: {}", status);
        info!("Registration response URL: {}", response_url);
        info!("Registration response length: {} bytes", text.len());

        // Log first 500 chars of response for debugging
        debug!("Registration response preview: {}", crate::safe_truncate(&text, 500));

        // Check for success - redirect to home/dashboard/verify or main page
        if response_url.contains("/home")
            || response_url.contains("/dashboard")
            || response_url.contains("/email/verify")
            || response_url.contains("/verify")
            || response_url == format!("{}/", GRINTAHUB_URL)
            || response_url == GRINTAHUB_URL {
            info!("Registration successful for: {} (redirected to {})", email, response_url);
            return Ok(Account {
                id: None,
                name: name.to_string(),
                email: email.to_string(),
                phone: Some(phone.to_string()),
                token: None,
                cookies: None,
            });
        }

        // Check for success patterns in response
        if text.contains("successfully") || text.contains("تم التسجيل") || text.contains("مرحبا") {
            info!("Registration successful for: {}", email);
            return Ok(Account {
                id: None,
                name: name.to_string(),
                email: email.to_string(),
                phone: Some(phone.to_string()),
                token: None,
                cookies: None,
            });
        }

        // Parse error from response
        self.parse_registration_error(&text, status)
    }

    /// Parse registration error from HTML response
    fn parse_registration_error(&self, text: &str, status: reqwest::StatusCode) -> Result<Account, AuthError> {
        let text_lower = text.to_lowercase();

        // Check for specific error patterns
        let error_patterns = [
            ("already been taken", "Email already exists"),
            ("already registered", "Email already exists"),
            ("email has already", "Email already exists"),
            ("phone has already", "Phone already registered"),
            ("invalid email", "Invalid email format"),
            ("password must", "Password requirements not met"),
            ("passwords do not match", "Passwords do not match"),
            ("too many attempts", "Rate limited"),
            ("throttle", "Rate limited"),
            ("البريد الإلكتروني مُستخدم", "Email already exists"),
            ("البريد الإلكتروني مستخدم", "Email already exists"),
            ("رقم الهاتف مُستخدم", "Phone already registered"),
            ("captcha validation failed", "CAPTCHA validation failed"),
            ("recaptcha verification failed", "CAPTCHA verification failed"),
        ];

        for (pattern, error_msg) in error_patterns {
            if text_lower.contains(pattern) {
                if error_msg.contains("Rate") {
                    return Err(AuthError::RateLimited);
                }
                return Err(AuthError::RegistrationFailed(error_msg.to_string()));
            }
        }

        // Look for validation errors in HTML and try to extract them
        let mut errors = Vec::new();

        // Try to find error messages in various formats
        // Pattern 1: <span class="invalid-feedback">...</span>
        // Pattern 2: <div class="text-danger">...</div>
        // Pattern 3: <small class="text-danger">...</small>
        for pattern in ["invalid-feedback", "text-danger", "error-message", "help-block"] {
            let mut search_pos = 0;
            while let Some(start) = text[search_pos..].find(pattern) {
                let abs_start = search_pos + start;
                // Find the closing tag and extract content
                if let Some(content_start) = text[abs_start..].find('>') {
                    let content_abs_start = abs_start + content_start + 1;
                    if let Some(content_end) = text[content_abs_start..].find('<') {
                        let error_text = text[content_abs_start..content_abs_start + content_end].trim();
                        if !error_text.is_empty() && error_text.len() > 2 && error_text.len() < 200 {
                            errors.push(error_text.to_string());
                        }
                    }
                }
                search_pos = abs_start + pattern.len();
                if search_pos >= text.len() {
                    break;
                }
            }
        }

        // Log found errors
        if !errors.is_empty() {
            // Deduplicate and join
            errors.sort();
            errors.dedup();
            let error_summary = errors.join("; ");
            warn!("Form validation errors found: {}", error_summary);
            return Err(AuthError::RegistrationFailed(format!("Validation errors: {}", error_summary)));
        }

        // If we found is-invalid class but couldn't extract error text
        if text.contains("is-invalid") {
            warn!("Form has invalid fields but couldn't extract error messages");
            // Log a snippet of HTML around is-invalid for debugging
            if let Some(pos) = text.find("is-invalid") {
                let start = pos.saturating_sub(100);
                let end = (pos + 200).min(text.len());
                debug!("HTML around is-invalid: {}", &text[start..end]);
            }
            return Err(AuthError::RegistrationFailed("Form validation failed (fields marked invalid)".into()));
        }

        // If the form was returned without redirect, the submission failed
        if text.contains("register-form") {
            return Err(AuthError::RegistrationFailed("Registration rejected - form returned without success".into()));
        }

        Err(AuthError::RegistrationFailed(format!("Unknown error (status {})", status)))
    }

    /// Login with automatic CAPTCHA solving
    pub async fn login_with_captcha(
        &self,
        captcha_solver: &CaptchaSolver,
        email: &str,
        password: &str,
    ) -> Result<Account, AuthError> {
        // Solve CAPTCHA first
        let captcha_request = CaptchaRequest::grintahub_login();
        let captcha_result = captcha_solver.solve(&captcha_request)
            .await
            .map_err(|e| AuthError::LoginFailed(format!("CAPTCHA failed: {}", e)))?;

        // Login with solved CAPTCHA
        self.login(email, password, &captcha_result.token).await
    }

    /// Batch register accounts with automatic CAPTCHA solving
    pub async fn batch_register(
        &self,
        captcha_solver: &CaptchaSolver,
        count: usize,
        password: &str,
        delay_ms: Option<u64>,
    ) -> Vec<Result<Account, AuthError>> {
        info!("Batch registering {} accounts", count);
        let mut results = Vec::with_capacity(count);

        for i in 0..count {
            let name = FakeData::random_name();
            let email = FakeData::random_email();
            let phone = FakeData::random_saudi_phone();

            info!("Registering account {}/{}: {}", i + 1, count, email);

            let result = self.register_with_captcha(
                captcha_solver,
                &name,
                &email,
                &phone,
                password,
            ).await;

            match &result {
                Ok(account) => info!("Account created: {}", account.email),
                Err(e) => warn!("Registration failed: {}", e),
            }

            results.push(result);

            // Delay between registrations
            if let Some(delay) = delay_ms {
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
        }

        results
    }
}
