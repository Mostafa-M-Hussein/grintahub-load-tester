//! 2Captcha solver implementation
//!
//! Based on ticketforge implementation with support for:
//! - reCAPTCHA v2/v3
//! - Turnstile
//! - Race mode (parallel solves)

use std::time::{Duration, Instant};
use reqwest::Client;
use tracing::{info, debug};

use super::types::*;

/// 2Captcha API base URL
const TWOCAPTCHA_API: &str = "https://api.2captcha.com";

/// CAPTCHA solver using 2Captcha service
pub struct CaptchaSolver {
    api_key: String,
    client: Client,
    poll_interval: Duration,
    max_solve_time: Duration,
}

impl CaptchaSolver {
    /// Create a new CAPTCHA solver
    pub fn new(api_key: &str) -> Result<Self, CaptchaError> {
        if api_key.is_empty() {
            return Err(CaptchaError::ApiKeyMissing);
        }

        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| CaptchaError::NetworkError(e.to_string()))?;

        Ok(Self {
            api_key: api_key.to_string(),
            client,
            poll_interval: Duration::from_secs(5),
            max_solve_time: Duration::from_secs(120),
        })
    }

    /// Set poll interval
    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    /// Set maximum solve time
    pub fn with_max_solve_time(mut self, timeout: Duration) -> Self {
        self.max_solve_time = timeout;
        self
    }

    /// Solve a CAPTCHA (single attempt with polling)
    pub async fn solve(&self, request: &CaptchaRequest) -> Result<CaptchaResult, CaptchaError> {
        let start = Instant::now();

        info!(
            "Solving {} CAPTCHA for {}",
            format!("{:?}", request.captcha_type),
            request.page_url
        );

        // Create task
        let task_id = self.create_task(request).await?;
        debug!("Created task ID: {}", task_id);

        // Poll for result
        let deadline = Instant::now() + self.max_solve_time;

        loop {
            if Instant::now() > deadline {
                return Err(CaptchaError::Timeout(self.max_solve_time.as_secs()));
            }

            tokio::time::sleep(self.poll_interval).await;

            match self.get_result(task_id).await? {
                Some(token) => {
                    let solve_time_ms = start.elapsed().as_millis() as u64;
                    info!("CAPTCHA solved in {}ms", solve_time_ms);
                    return Ok(CaptchaResult { token, solve_time_ms });
                }
                None => {
                    debug!("Task {} still processing...", task_id);
                }
            }
        }
    }

    /// Solve with race mode (parallel attempts, first wins)
    pub async fn solve_race(
        &self,
        request: &CaptchaRequest,
        parallel: usize,
    ) -> Result<CaptchaResult, CaptchaError> {
        use futures::future::select_all;

        let parallel = parallel.max(1).min(5); // 1-5 parallel attempts
        info!("Starting race solve with {} parallel attempts", parallel);

        let futures: Vec<_> = (0..parallel)
            .map(|_| Box::pin(self.solve(request)))
            .collect();

        let (result, _index, _remaining) = select_all(futures).await;
        result
    }

    /// Batch solve (multiple CAPTCHAs concurrently)
    pub async fn solve_batch(
        &self,
        request: &CaptchaRequest,
        count: usize,
        race_parallel: usize,
    ) -> Vec<Result<CaptchaResult, CaptchaError>> {
        use futures::future::join_all;

        info!("Batch solving {} CAPTCHAs", count);

        let futures: Vec<_> = (0..count)
            .map(|_| self.solve_race(request, race_parallel))
            .collect();

        join_all(futures).await
    }

    /// Get account balance from 2Captcha
    pub async fn get_balance(&self) -> Result<f64, CaptchaError> {
        let url = format!(
            "https://2captcha.com/res.php?key={}&action=getbalance&json=1",
            self.api_key
        );

        let response = self.client
            .get(&url)
            .send()
            .await
            .map_err(|e| CaptchaError::NetworkError(e.to_string()))?;

        let text = response.text().await
            .map_err(|e| CaptchaError::NetworkError(e.to_string()))?;

        // Parse balance response
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(balance) = json.get("request").and_then(|v| v.as_str()) {
                return balance.parse().map_err(|_| CaptchaError::InvalidResponse(text));
            }
            if let Some(balance) = json.get("balance").and_then(|v| v.as_f64()) {
                return Ok(balance);
            }
        }

        // Try parsing as plain number
        text.trim().parse().map_err(|_| CaptchaError::InvalidResponse(text))
    }

    /// Create a task with 2Captcha
    async fn create_task(&self, request: &CaptchaRequest) -> Result<i64, CaptchaError> {
        let url = format!("{}/createTask", TWOCAPTCHA_API);

        let task = match request.captcha_type {
            CaptchaType::RecaptchaV2 if request.enterprise => TwoCaptchaTask::RecaptchaV2EnterpriseProxyless {
                website_url: request.page_url.clone(),
                website_key: request.sitekey.clone(),
                recaptcha_data_s_value: request.data_s.clone(),
            },
            CaptchaType::RecaptchaV2 => TwoCaptchaTask::RecaptchaV2Proxyless {
                website_url: request.page_url.clone(),
                website_key: request.sitekey.clone(),
            },
            CaptchaType::RecaptchaV3 => TwoCaptchaTask::RecaptchaV3Proxyless {
                website_url: request.page_url.clone(),
                website_key: request.sitekey.clone(),
                page_action: request.action.clone().unwrap_or_else(|| "verify".to_string()),
                min_score: request.min_score.unwrap_or(0.5),
            },
            CaptchaType::Turnstile => TwoCaptchaTask::TurnstileProxyless {
                website_url: request.page_url.clone(),
                website_key: request.sitekey.clone(),
            },
            CaptchaType::HCaptcha => TwoCaptchaTask::HCaptchaProxyless {
                website_url: request.page_url.clone(),
                website_key: request.sitekey.clone(),
            },
        };

        let create_request = TwoCaptchaCreateTask {
            client_key: self.api_key.clone(),
            task,
        };

        // Debug: Log the request being sent (without API key)
        debug!("2Captcha createTask request: type={:?}, enterprise={}, data_s={}, url={}, sitekey={}...",
            request.captcha_type,
            request.enterprise,
            request.data_s.as_ref().map(|s| format!("{}...", &s[..s.len().min(20)])).unwrap_or_else(|| "none".to_string()),
            &request.page_url[..request.page_url.len().min(80)],
            &request.sitekey[..request.sitekey.len().min(20)]
        );

        let response = self.client
            .post(&url)
            .json(&create_request)
            .send()
            .await
            .map_err(|e| CaptchaError::NetworkError(e.to_string()))?;

        // Get raw response text first for debugging
        let response_text = response.text().await
            .map_err(|e| CaptchaError::NetworkError(e.to_string()))?;

        debug!("2Captcha createTask response: {}", &response_text[..response_text.len().min(500)]);

        let result: TwoCaptchaCreateResponse = serde_json::from_str(&response_text)
            .map_err(|e| CaptchaError::InvalidResponse(format!("Parse error: {} - Response: {}", e, &response_text[..response_text.len().min(200)])))?;

        if result.error_id != 0 {
            // Build detailed error message
            let error_msg = format!(
                "errorId={}, code={}, desc={}",
                result.error_id,
                result.error_code.as_deref().unwrap_or("none"),
                result.error_description.as_deref().unwrap_or("none")
            );
            info!("2Captcha task creation failed: {}", error_msg);
            return Err(CaptchaError::TaskCreationFailed(error_msg));
        }

        let task_id = result.task_id.ok_or_else(|| CaptchaError::InvalidResponse("No task ID in response".into()))?;
        info!("2Captcha task created: ID={}", task_id);
        Ok(task_id)
    }

    /// Get task result from 2Captcha
    async fn get_result(&self, task_id: i64) -> Result<Option<String>, CaptchaError> {
        let url = format!("{}/getTaskResult", TWOCAPTCHA_API);

        let request = TwoCaptchaGetResult {
            client_key: self.api_key.clone(),
            task_id,
        };

        let response = self.client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| CaptchaError::NetworkError(e.to_string()))?;

        let result: TwoCaptchaResultResponse = response.json().await
            .map_err(|e| CaptchaError::InvalidResponse(e.to_string()))?;

        if result.error_id != 0 {
            let error_msg = result.error_description
                .or(result.error_code)
                .unwrap_or_else(|| format!("Error ID: {}", result.error_id));
            return Err(CaptchaError::ApiError(error_msg));
        }

        if result.is_processing() {
            return Ok(None);
        }

        if result.is_ready() {
            if let Some(token) = result.get_token() {
                return Ok(Some(token.to_string()));
            }
        }

        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_captcha_request_creation() {
        let req = CaptchaRequest::grintahub_registration();
        assert_eq!(req.captcha_type, CaptchaType::RecaptchaV3);
        assert_eq!(req.sitekey, "6LfRMGAqAAAAADmJB37u6YTAMGcZ5OMzZg53wPCN");
        assert_eq!(req.action, Some("generic".to_string()));
    }
}
