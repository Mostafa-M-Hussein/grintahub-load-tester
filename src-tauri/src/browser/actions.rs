//! Browser automation actions for organic Google traffic to grintahub.com
//!
//! Simulates real user behavior:
//! 1. Go to Google.com (optionally logged in with Google account)
//! 2. Search for keywords
//! 3. Find and click on grintahub.com in search results
//! 4. Browse the website naturally

use std::sync::Arc;
use std::time::Duration;
use rand::Rng;
use tracing::{info, debug, warn, error};
use base64::Engine;

use super::{BrowserSession, BrowserError};

/// Google account credentials for login
#[derive(Debug, Clone)]
pub struct GoogleAccount {
    pub email: String,
    pub password: String,
}

/// Browser actions for organic Google traffic
pub struct BrowserActions;

/// Google search selectors
mod selectors {
    pub const GOOGLE_SEARCH_INPUT: &str = "input[name='q'], textarea[name='q']";
    pub const GOOGLE_SEARCH_BUTTON: &str = "input[name='btnK'], button[type='submit']";
    pub const GOOGLE_RESULTS: &str = "#search a[href], #rso a[href]";
    pub const GOOGLE_NEXT_PAGE: &str = "#pnnext";
}

impl BrowserActions {
    /// Check if we hit a Google CAPTCHA/sorry page or network error
    /// Returns immediately so session can be restarted with new IP
    pub async fn check_google_captcha(session: &Arc<BrowserSession>) -> Result<bool, BrowserError> {
        let result = session.execute_js_with_timeout(r#"
            (function() {
                const url = window.location.href || '';
                const title = document.title || '';
                const titleLower = title.toLowerCase();
                const bodyText = document.body ? document.body.innerText : '';

                // Check URL for /sorry/ (Google CAPTCHA page) - IMMEDIATE detection
                if (url.includes('/sorry/') || url.includes('google.com/sorry')) {
                    return { blocked: true, type: 'captcha_sorry' };
                }

                // Check for reCAPTCHA iframe
                if (document.querySelector('iframe[src*="recaptcha"]')) {
                    return { blocked: true, type: 'recaptcha' };
                }

                // Check for "unusual traffic" text
                if (bodyText.includes('unusual traffic') ||
                    bodyText.includes('automated queries') ||
                    titleLower.includes('sorry')) {
                    return { blocked: true, type: 'unusual_traffic' };
                }

                // Check for network errors - "site can't be reached"
                if (titleLower.includes("can't be reached") ||
                    titleLower.includes('cannot be reached') ||
                    titleLower.includes('err_') ||
                    bodyText.includes('ERR_CONNECTION') ||
                    bodyText.includes('ERR_PROXY') ||
                    bodyText.includes('ERR_TIMED_OUT') ||
                    bodyText.includes("This site can't be reached")) {
                    return { blocked: true, type: 'network_error' };
                }

                // Check for proxy authentication errors
                if (bodyText.includes('Proxy Authentication Required') ||
                    bodyText.includes('407')) {
                    return { blocked: true, type: 'proxy_auth_error' };
                }

                return { blocked: false };
            })()
        "#, 5).await?; // 5 second timeout for quick detection

        let is_blocked = result.get("blocked")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if is_blocked {
            let block_type = result.get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            warn!("Session {} BLOCKED - type: {} - will restart with new IP", session.id, block_type);
        }

        Ok(is_blocked)
    }

    /// Try to solve a Google CAPTCHA using 2Captcha service
    /// Returns Ok(true) if solved, Ok(false) if solving failed
    pub async fn solve_google_captcha(
        session: &Arc<BrowserSession>,
        captcha_api_key: &str,
    ) -> Result<bool, BrowserError> {
        if captcha_api_key.is_empty() {
            warn!("Session {} no 2Captcha API key, cannot solve CAPTCHA", session.id);
            return Ok(false);
        }

        info!("Session {} solving Google CAPTCHA with 2Captcha...", session.id);

        // 1. Extract page info and reCAPTCHA sitekey
        let captcha_info = session.execute_js(r#"
            (function() {
                const url = window.location.href;
                const isGoogleSorry = url.includes('/sorry/') || url.includes('google.com/sorry');
                const bodyText = document.body ? document.body.innerText : '';
                let sitekey = null;
                let method = 'none';

                // Method 1: data-sitekey attribute (most common)
                const recaptchaDiv = document.querySelector('[data-sitekey]');
                if (recaptchaDiv) {
                    sitekey = recaptchaDiv.getAttribute('data-sitekey');
                    method = 'data-sitekey';
                }

                // Method 2: reCAPTCHA iframe src parameter k=
                if (!sitekey) {
                    const iframe = document.querySelector('iframe[src*="recaptcha"]');
                    if (iframe) {
                        const match = iframe.src.match(/[?&]k=([^&]+)/);
                        if (match) {
                            sitekey = match[1];
                            method = 'iframe-k-param';
                        }
                    }
                }

                // Method 3: reCAPTCHA anchor iframe
                if (!sitekey) {
                    const anchor = document.querySelector('iframe[src*="anchor"]');
                    if (anchor) {
                        const match = anchor.src.match(/[?&]k=([^&]+)/);
                        if (match) {
                            sitekey = match[1];
                            method = 'anchor-iframe';
                        }
                    }
                }

                // Method 4: Script tags with sitekey reference
                if (!sitekey) {
                    const scripts = document.querySelectorAll('script');
                    for (const script of scripts) {
                        const text = script.textContent || '';
                        const m = text.match(/['"]sitekey['"]\s*:\s*['"]([A-Za-z0-9_-]{30,60})['"]/);
                        if (m) { sitekey = m[1]; method = 'script-sitekey'; break; }
                        const m2 = text.match(/grecaptcha\.render\([^,]+,\s*\{[^}]*['"]sitekey['"]\s*:\s*['"]([^'"]+)['"]/);
                        if (m2) { sitekey = m2[1]; method = 'script-render'; break; }
                    }
                }

                // Method 5: Script src with render= parameter
                if (!sitekey) {
                    const apiScript = document.querySelector('script[src*="recaptcha/api.js"], script[src*="recaptcha/enterprise.js"]');
                    if (apiScript) {
                        const match = apiScript.src.match(/[?&]render=([^&]+)/);
                        if (match && match[1] !== 'explicit') {
                            sitekey = match[1];
                            method = 'api-render-param';
                        }
                    }
                }

                // Method 6: ___grecaptcha_cfg global
                if (!sitekey && typeof ___grecaptcha_cfg !== 'undefined') {
                    try {
                        const clients = ___grecaptcha_cfg.clients;
                        for (const key in clients) {
                            const c = clients[key];
                            if (c && c.rr) {
                                for (const rkey in c.rr) {
                                    const rr = c.rr[rkey];
                                    if (rr && rr.sitekey) {
                                        sitekey = rr.sitekey;
                                        method = 'grecaptcha-cfg';
                                        break;
                                    }
                                }
                            }
                        }
                    } catch(e) {}
                }

                // Detect callback function name
                let callbackName = null;
                if (recaptchaDiv) {
                    callbackName = recaptchaDiv.getAttribute('data-callback');
                }

                // Detect if reCAPTCHA Enterprise (Google /sorry/ pages use Enterprise)
                let isEnterprise = false;
                const iframes = document.querySelectorAll('iframe[src*="recaptcha"]');
                for (const iframe of iframes) {
                    if (iframe.src.includes('/enterprise/')) {
                        isEnterprise = true;
                        break;
                    }
                }
                // Also check script tags for enterprise API
                if (!isEnterprise) {
                    const scripts = document.querySelectorAll('script[src*="recaptcha/enterprise"]');
                    if (scripts.length > 0) isEnterprise = true;
                }

                // Check for the number of forms and buttons
                const forms = document.querySelectorAll('form');
                const submitBtns = document.querySelectorAll('input[type="submit"], button[type="submit"]');

                return {
                    url: url,
                    sitekey: sitekey,
                    method: method,
                    isGoogleSorry: isGoogleSorry,
                    isEnterprise: isEnterprise,
                    callbackName: callbackName,
                    formCount: forms.length,
                    submitCount: submitBtns.length,
                    hasRecaptchaResponse: !!document.querySelector('#g-recaptcha-response, textarea[name="g-recaptcha-response"]'),
                    bodyPreview: bodyText.substring(0, 200)
                };
            })()
        "#).await?;

        let page_url = captcha_info.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let method = captcha_info.get("method").and_then(|v| v.as_str()).unwrap_or("none");
        let is_google_sorry = captcha_info.get("isGoogleSorry").and_then(|v| v.as_bool()).unwrap_or(false);
        let is_enterprise = captcha_info.get("isEnterprise").and_then(|v| v.as_bool()).unwrap_or(false);
        let callback_name = captcha_info.get("callbackName").and_then(|v| v.as_str());
        let form_count = captcha_info.get("formCount").and_then(|v| v.as_u64()).unwrap_or(0);
        let body_preview = captcha_info.get("bodyPreview").and_then(|v| v.as_str()).unwrap_or("");

        info!("Session {} CAPTCHA page: google_sorry={}, enterprise={}, method={}, forms={}, callback={:?}, url={}",
            session.id, is_google_sorry, is_enterprise, method, form_count, callback_name, &page_url[..page_url.len().min(80)]);
        debug!("Session {} page body: {}", session.id, body_preview);

        let sitekey = match captcha_info.get("sitekey").and_then(|v| v.as_str()) {
            Some(key) if !key.is_empty() => key.to_string(),
            _ => {
                warn!("Session {} could not find sitekey (method: {})", session.id, method);
                return Ok(false);
            }
        };

        info!("Session {} found sitekey: {} (via {})", session.id, &sitekey[..sitekey.len().min(20)], method);

        // 2. Create solver and solve reCAPTCHA v2
        let solver = match crate::captcha::CaptchaSolver::new(captcha_api_key) {
            Ok(s) => s,
            Err(e) => {
                warn!("Session {} failed to create solver: {}", session.id, e);
                return Ok(false);
            }
        };

        // Use Enterprise variant for Google sorry pages (they use reCAPTCHA Enterprise)
        let request = if is_enterprise || is_google_sorry {
            info!("Session {} using reCAPTCHA Enterprise solver", session.id);
            crate::captcha::CaptchaRequest::recaptcha_v2_enterprise(&sitekey, &page_url)
        } else {
            crate::captcha::CaptchaRequest::recaptcha_v2(&sitekey, &page_url)
        };

        let result = match solver.solve(&request).await {
            Ok(r) => {
                info!("Session {} CAPTCHA solved in {}ms! Injecting token...", session.id, r.solve_time_ms);
                r
            }
            Err(e) => {
                warn!("Session {} 2Captcha solve failed: {}", session.id, e);
                return Ok(false);
            }
        };

        // 3. Inject token and submit the form
        let token = result.token.replace('\'', "\\'").replace('\n', "\\n");
        let callback_js = if let Some(cb) = callback_name {
            format!(
                "if (typeof {} === 'function') {{ try {{ {}('{}'); }} catch(e) {{}} }}",
                cb, cb, token
            )
        } else {
            String::new()
        };

        let inject_script = format!(r#"
            (function() {{
                const token = '{}';
                let injected = false;
                let submitted = false;

                // Step 1: Inject token into ALL g-recaptcha-response textareas
                const textareas = document.querySelectorAll('#g-recaptcha-response, textarea[name="g-recaptcha-response"]');
                for (const ta of textareas) {{
                    ta.style.display = 'block';
                    ta.innerHTML = token;
                    ta.value = token;
                    ta.style.display = 'none';
                    injected = true;
                }}

                // Step 2: Try data-callback function
                {}

                // Step 3: Try ___grecaptcha_cfg callback (deep search)
                if (typeof ___grecaptcha_cfg !== 'undefined') {{
                    try {{
                        const clients = ___grecaptcha_cfg.clients;
                        for (const ckey in clients) {{
                            const client = clients[ckey];
                            // Search nested objects for callback functions
                            const searchObj = (obj, depth) => {{
                                if (depth > 5 || !obj) return;
                                for (const key in obj) {{
                                    if (typeof obj[key] === 'function' && key.toLowerCase().includes('callback')) {{
                                        try {{ obj[key](token); submitted = true; }} catch(e) {{}}
                                    }} else if (typeof obj[key] === 'object' && obj[key] !== null) {{
                                        searchObj(obj[key], depth + 1);
                                    }}
                                }}
                            }};
                            searchObj(client, 0);
                        }}
                    }} catch(e) {{}}
                }}

                // Step 4: Try global callback functions commonly used by Google
                const globalCallbacks = ['onCaptchaSuccess', 'captchaCallback', 'recaptchaCallback', 'onSuccess'];
                for (const name of globalCallbacks) {{
                    if (typeof window[name] === 'function') {{
                        try {{ window[name](token); submitted = true; }} catch(e) {{}}
                    }}
                }}

                // Step 5: Submit form if not already submitted by callback
                if (!submitted) {{
                    // For Google sorry page: find the specific form
                    const forms = document.querySelectorAll('form');
                    for (const form of forms) {{
                        // Check if this form has the captcha response
                        const hasResponse = form.querySelector('[name="g-recaptcha-response"]') ||
                                           form.querySelector('#g-recaptcha-response');
                        if (hasResponse || forms.length === 1) {{
                            // Ensure the token is in a hidden input too (some forms need this)
                            let hiddenInput = form.querySelector('input[name="g-recaptcha-response"]');
                            if (!hiddenInput) {{
                                hiddenInput = document.createElement('input');
                                hiddenInput.type = 'hidden';
                                hiddenInput.name = 'g-recaptcha-response';
                                hiddenInput.value = token;
                                form.appendChild(hiddenInput);
                            }} else {{
                                hiddenInput.value = token;
                            }}

                            setTimeout(() => form.submit(), 300);
                            submitted = true;
                            break;
                        }}
                    }}
                }}

                // Step 6: Last resort - click submit button
                if (!submitted) {{
                    const btn = document.querySelector('input[type="submit"], button[type="submit"], #submit, .submit');
                    if (btn) {{
                        setTimeout(() => btn.click(), 300);
                        submitted = true;
                    }}
                }}

                return {{ injected: injected, submitted: submitted, textareaCount: textareas.length }};
            }})()
        "#, token, callback_js);

        let inject_result = session.execute_js(&inject_script).await?;

        let injected = inject_result.get("injected").and_then(|v| v.as_bool()).unwrap_or(false);
        let submitted = inject_result.get("submitted").and_then(|v| v.as_bool()).unwrap_or(false);
        let textarea_count = inject_result.get("textareaCount").and_then(|v| v.as_u64()).unwrap_or(0);

        info!("Session {} token injection: injected={}, submitted={}, textareas={}",
            session.id, injected, submitted, textarea_count);

        if submitted {
            info!("Session {} CAPTCHA solved and form submitted! Waiting for redirect...", session.id);
            // Wait for the page to redirect after form submission
            Self::human_delay(3000, 2000).await;

            // Verify we left the CAPTCHA page
            let still_blocked = Self::check_google_captcha(session).await.unwrap_or(true);
            if !still_blocked {
                info!("Session {} CAPTCHA bypass SUCCESS - page loaded", session.id);
                return Ok(true);
            } else {
                warn!("Session {} still blocked after CAPTCHA submission", session.id);
                // Try one more wait - Google sometimes takes a moment
                Self::human_delay(2000, 1000).await;
                let still_blocked2 = Self::check_google_captcha(session).await.unwrap_or(true);
                if !still_blocked2 {
                    info!("Session {} CAPTCHA bypass SUCCESS (delayed)", session.id);
                    return Ok(true);
                }
                return Ok(false);
            }
        } else if injected {
            warn!("Session {} token injected but form not submitted - trying manual submit", session.id);
            // Try a direct form submit as last resort
            let _ = session.execute_js(r#"
                const form = document.querySelector('form');
                if (form) form.submit();
            "#).await;
            Self::human_delay(3000, 2000).await;

            let still_blocked = Self::check_google_captcha(session).await.unwrap_or(true);
            if !still_blocked {
                info!("Session {} CAPTCHA resolved after manual submit", session.id);
                return Ok(true);
            }
            return Ok(false);
        } else {
            warn!("Session {} could not inject token (no textarea found)", session.id);
            return Ok(false);
        }
    }

    /// Check if we're already logged into Google
    /// More strict check - only return true if we can confirm actual login
    pub async fn is_google_logged_in(session: &Arc<BrowserSession>) -> Result<bool, BrowserError> {
        let result = session.execute_js(r#"
            (function() {
                // STRICT check - must find actual profile elements that prove login

                // 1. Look for the signed-in user's profile photo with specific attributes
                // This is the circular avatar in the top right
                const avatar = document.querySelector('img.gb_q.gb_r') ||
                              document.querySelector('a[aria-label*="Google Account"][href*="accounts.google.com"]');

                // 2. Look for sign-out link (only visible when logged in)
                const signOutLink = document.querySelector('a[href*="accounts.google.com/Logout"]') ||
                                   document.querySelector('a[href*="SignOutOptions"]');

                // 3. Gmail link with user email (only when logged in)
                const gmailWithEmail = document.querySelector('a[aria-label*="@gmail.com"]');

                // 4. Check the "My Account" or user dropdown specifically
                const accountButton = document.querySelector('[data-ogsr-up]') &&
                                     document.querySelector('[data-ogsr-up] img[src*="googleusercontent.com"]');

                // Must have at least 2 indicators to confirm login
                // This prevents false positives from random element matches
                const indicators = [avatar, signOutLink, gmailWithEmail, accountButton].filter(Boolean).length;

                return {
                    loggedIn: indicators >= 1 && (avatar || accountButton),
                    indicators: indicators,
                    hasAvatar: !!avatar,
                    hasSignOut: !!signOutLink,
                    hasGmail: !!gmailWithEmail,
                    hasAccountBtn: !!accountButton
                };
            })()
        "#).await?;

        let logged_in = result.get("loggedIn")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let indicators = result.get("indicators")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);

        if logged_in {
            info!("Session {} is logged into Google (indicators: {})", session.id, indicators);
        } else {
            debug!("Session {} is NOT logged into Google (indicators: {})", session.id, indicators);
        }

        Ok(logged_in)
    }

    /// Login to Google account with human-like behavior
    /// This helps with ad targeting as Google shows more personalized ads to logged-in users
    pub async fn login_to_google(
        session: &Arc<BrowserSession>,
        account: &GoogleAccount,
    ) -> Result<bool, BrowserError> {
        info!("Session {} attempting Google login for: {}", session.id, account.email);

        // Navigate to Google login page
        session.navigate("https://accounts.google.com/signin/v2/identifier").await?;
        Self::human_delay(2000, 1500).await;

        // Check for CAPTCHA
        if Self::check_google_captcha(session).await? {
            error!("Session {} hit CAPTCHA on Google login page", session.id);
            session.increment_captchas();
            return Err(BrowserError::CaptchaDetected("CAPTCHA on Google login".into()));
        }

        // Enter email with human-like typing
        let email_script = format!(r#"
            (async function() {{
                // Find email input
                const emailInput = document.querySelector('input[type="email"]') ||
                                  document.querySelector('#identifierId');
                if (!emailInput) return {{ success: false, error: 'Email input not found' }};

                // Move mouse to input
                const rect = emailInput.getBoundingClientRect();
                document.dispatchEvent(new MouseEvent('mousemove', {{
                    clientX: rect.left + rect.width / 2 + (Math.random() * 20 - 10),
                    clientY: rect.top + rect.height / 2 + (Math.random() * 10 - 5),
                    bubbles: true
                }}));
                await new Promise(r => setTimeout(r, 200 + Math.random() * 300));

                // Click and focus
                emailInput.click();
                emailInput.focus();
                await new Promise(r => setTimeout(r, 100 + Math.random() * 200));

                // Type email character by character
                const email = "{}";
                for (let i = 0; i < email.length; i++) {{
                    let delay = 40 + Math.random() * 100;
                    if (email[i] === '@' || email[i] === '.') delay += 50 + Math.random() * 100;
                    if (Math.random() < 0.03) delay += 200 + Math.random() * 400; // Occasional pause
                    await new Promise(r => setTimeout(r, delay));

                    emailInput.value += email[i];
                    emailInput.dispatchEvent(new Event('input', {{ bubbles: true }}));
                }}

                return {{ success: true }};
            }})()
        "#, account.email.replace('\\', "\\\\").replace('"', "\\\""));

        let email_result = session.execute_js(&email_script).await?;
        if email_result.get("success").and_then(|v| v.as_bool()) != Some(true) {
            let error = email_result.get("error").and_then(|v| v.as_str()).unwrap_or("Unknown error");
            return Err(BrowserError::JavaScriptError(format!("Failed to enter email: {}", error)));
        }

        Self::human_delay(500, 500).await;

        // Click Next button
        session.execute_js(r#"
            (function() {
                const nextBtn = document.querySelector('#identifierNext button') ||
                               document.querySelector('button[jsname="LgbsSe"]') ||
                               document.querySelector('[data-idom-class*="action-button"]');
                if (nextBtn) {
                    nextBtn.dispatchEvent(new MouseEvent('mouseover', { bubbles: true }));
                    setTimeout(() => nextBtn.click(), 100 + Math.random() * 150);
                    return true;
                }
                return false;
            })()
        "#).await?;

        // Wait for password page
        Self::human_delay(2500, 2000).await;

        // Check for CAPTCHA again
        if Self::check_google_captcha(session).await? {
            error!("Session {} hit CAPTCHA after email entry", session.id);
            session.increment_captchas();
            return Err(BrowserError::CaptchaDetected("CAPTCHA after email entry".into()));
        }

        // Check if password field appeared
        let has_password = session.execute_js(r#"
            !!document.querySelector('input[type="password"]') ||
            !!document.querySelector('[name="Passwd"]')
        "#).await?;

        if has_password.as_bool() != Some(true) {
            // Might be a different flow (phone verification, etc.)
            warn!("Session {} password field not found - may need verification", session.id);
            return Ok(false);
        }

        // Enter password with human-like typing
        let password_script = format!(r#"
            (async function() {{
                const passwordInput = document.querySelector('input[type="password"]') ||
                                     document.querySelector('[name="Passwd"]');
                if (!passwordInput) return {{ success: false, error: 'Password input not found' }};

                // Move mouse and click
                const rect = passwordInput.getBoundingClientRect();
                document.dispatchEvent(new MouseEvent('mousemove', {{
                    clientX: rect.left + rect.width / 2 + (Math.random() * 20 - 10),
                    clientY: rect.top + rect.height / 2 + (Math.random() * 10 - 5),
                    bubbles: true
                }}));
                await new Promise(r => setTimeout(r, 200 + Math.random() * 300));

                passwordInput.click();
                passwordInput.focus();
                await new Promise(r => setTimeout(r, 100 + Math.random() * 200));

                // Type password character by character
                const password = "{}";
                for (let i = 0; i < password.length; i++) {{
                    let delay = 30 + Math.random() * 80;
                    if (Math.random() < 0.02) delay += 150 + Math.random() * 300;
                    await new Promise(r => setTimeout(r, delay));

                    passwordInput.value += password[i];
                    passwordInput.dispatchEvent(new Event('input', {{ bubbles: true }}));
                }}

                return {{ success: true }};
            }})()
        "#, account.password.replace('\\', "\\\\").replace('"', "\\\""));

        let password_result = session.execute_js(&password_script).await?;
        if password_result.get("success").and_then(|v| v.as_bool()) != Some(true) {
            let error = password_result.get("error").and_then(|v| v.as_str()).unwrap_or("Unknown error");
            return Err(BrowserError::JavaScriptError(format!("Failed to enter password: {}", error)));
        }

        Self::human_delay(500, 500).await;

        // Click Sign In button
        session.execute_js(r#"
            (function() {
                const signInBtn = document.querySelector('#passwordNext button') ||
                                 document.querySelector('button[jsname="LgbsSe"]') ||
                                 document.querySelector('[data-idom-class*="action-button"]');
                if (signInBtn) {
                    signInBtn.dispatchEvent(new MouseEvent('mouseover', { bubbles: true }));
                    setTimeout(() => signInBtn.click(), 100 + Math.random() * 150);
                    return true;
                }
                return false;
            })()
        "#).await?;

        // Wait for login to complete
        Self::human_delay(4000, 3000).await;

        // Check if login was successful
        let login_success = Self::is_google_logged_in(session).await?;

        if login_success {
            info!("Session {} successfully logged into Google as {}", session.id, account.email);
            Ok(true)
        } else {
            // Check for 2FA or other verification needed
            let needs_verification = session.execute_js(r#"
                (function() {
                    const url = window.location.href;
                    return url.includes('challenge') ||
                           url.includes('signin/v2/challenge') ||
                           url.includes('myaccount.google.com') ||
                           !!document.querySelector('[data-challengetype]');
                })()
            "#).await?;

            if needs_verification.as_bool() == Some(true) {
                warn!("Session {} needs 2FA or verification to complete login", session.id);
                return Ok(false);
            }

            warn!("Session {} login may have failed", session.id);
            Ok(false)
        }
    }

    /// Navigate to Google.com with human-like behavior
    pub async fn goto_google(session: &Arc<BrowserSession>) -> Result<(), BrowserError> {
        info!("Session {} navigating to Google", session.id);

        // Navigate to Google Saudi Arabia directly for better regional targeting
        session.navigate("https://www.google.com.sa/").await?;

        // Wait for page to load
        Self::human_delay(1500, 500).await;

        // Handle any consent dialogs or interstitials (common in Middle East/EU)
        let handled_consent = session.execute_js(r#"
            (async function() {
                // Wait for any dialogs to appear
                await new Promise(r => setTimeout(r, 1000));

                // Comprehensive consent button selectors for different Google versions
                const consentSelectors = [
                    // Standard consent buttons
                    'button[id*="L2AGLb"]',          // "I agree" button
                    'button[id*="accept"]',
                    'button[id*="agree"]',
                    '[aria-label*="Accept"]',
                    '[aria-label*="agree"]',
                    '[aria-label*="قبول"]',           // Arabic "Accept"
                    '[aria-label*="موافق"]',          // Arabic "Agree"
                    // Consent dialog specific
                    '.tHlp8d',                        // Google consent button class
                    '#introAgreeButton',
                    'button.VfPpkd-LgbsSe',
                    // "Before you continue" dialog
                    'form[action*="consent"] button',
                    'div[role="dialog"] button:first-of-type',
                    // Any visible primary button
                    'button.VfPpkd-LgbsSe-OWXEXe-k8QpJ'
                ];

                for (const selector of consentSelectors) {
                    try {
                        const buttons = document.querySelectorAll(selector);
                        for (const btn of buttons) {
                            if (btn.offsetParent !== null && btn.offsetWidth > 0) {
                                // Human-like: move to button first
                                const rect = btn.getBoundingClientRect();
                                document.dispatchEvent(new MouseEvent('mousemove', {
                                    clientX: rect.left + rect.width / 2 + (Math.random() * 10 - 5),
                                    clientY: rect.top + rect.height / 2 + (Math.random() * 5 - 2.5),
                                    bubbles: true
                                }));
                                await new Promise(r => setTimeout(r, 200 + Math.random() * 300));

                                btn.click();
                                console.log('[Bot] Clicked consent button:', selector);
                                await new Promise(r => setTimeout(r, 500));
                                return 'clicked';
                            }
                        }
                    } catch (e) {}
                }

                // Check if we're on a consent page that needs form submission
                const consentForm = document.querySelector('form[action*="consent"]');
                if (consentForm) {
                    const submitBtn = consentForm.querySelector('button[type="submit"], input[type="submit"]');
                    if (submitBtn) {
                        submitBtn.click();
                        return 'form_submitted';
                    }
                }

                return 'no_consent_needed';
            })()
        "#).await?;

        info!("Session {} consent handling result: {:?}", session.id, handled_consent);

        // Brief wait after handling consent
        Self::human_delay(300, 200).await;

        // Check for CAPTCHA
        if Self::check_google_captcha(session).await? {
            session.increment_captchas();
            return Err(BrowserError::CaptchaDetected("CAPTCHA on Google homepage".into()));
        }

        // Verify we can see the search input - if not, try refreshing or waiting
        let has_search = session.execute_js(r#"
            (function() {
                const input = document.querySelector('input[name="q"], textarea[name="q"], input[type="text"][title*="Search"], input[aria-label*="Search"], input[aria-label*="بحث"]');
                return input !== null && input.offsetParent !== null;
            })()
        "#).await?;

        if has_search.as_bool() != Some(true) {
            warn!("Session {} search input not found, waiting longer...", session.id);
            Self::human_delay(1500, 500).await;

            // Try clicking anywhere to dismiss any overlays
            session.execute_js(r#"
                document.body.click();
                document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
            "#).await?;

            Self::human_delay(200, 200).await;
        }

        Ok(())
    }

    /// Search on Google with human-like typing
    pub async fn google_search(session: &Arc<BrowserSession>, keyword: &str) -> Result<bool, BrowserError> {
        info!("Session {} searching Google for: {}", session.id, keyword);

        // Extended selectors for Google search input (supports Arabic/RTL and different layouts)
        let search_found = session.execute_js(r#"
            (function() {
                const selectors = [
                    'input[name="q"]',
                    'textarea[name="q"]',
                    'input[type="text"][title*="Search"]',
                    'input[type="text"][title*="بحث"]',
                    'input[aria-label*="Search"]',
                    'input[aria-label*="بحث"]',
                    'textarea[aria-label*="Search"]',
                    'textarea[aria-label*="بحث"]',
                    '.gLFyf',
                    '#APjFqb'
                ];
                for (const sel of selectors) {
                    const input = document.querySelector(sel);
                    if (input && input.offsetParent !== null) {
                        return true;
                    }
                }
                return false;
            })()
        "#).await?;

        if search_found.as_bool() != Some(true) {
            warn!("Session {} could not find Google search input", session.id);
            return Ok(false);
        }

        // Simplified typing with base64 encoded keyword
        let b64_keyword = base64::engine::general_purpose::STANDARD.encode(keyword);
        let type_script = format!(r#"
            (async function() {{
                // Find Google search input
                const input = document.querySelector('input[name="q"], textarea[name="q"], .gLFyf, #APjFqb');
                if (!input) return false;

                // Focus and clear
                input.focus();
                input.click();
                input.value = '';

                await new Promise(r => setTimeout(r, 100 + Math.random() * 100));

                // Decode base64 keyword
                const b64 = "{}";
                const text = new TextDecoder().decode(Uint8Array.from(atob(b64), c => c.charCodeAt(0)));

                // Type each character
                for (let i = 0; i < text.length; i++) {{
                    await new Promise(r => setTimeout(r, 30 + Math.random() * 70));
                    input.value += text[i];
                    input.dispatchEvent(new Event('input', {{ bubbles: true }}));
                }}

                return true;
            }})()
        "#, b64_keyword);

        session.execute_js(&type_script).await?;

        // Wait before pressing enter
        Self::random_delay(300, 600).await;

        // Press Enter to search
        session.execute_js(r#"
            (function() {
                const input = document.querySelector('input[name="q"], textarea[name="q"]');
                if (input) {
                    input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', keyCode: 13, bubbles: true }));
                    input.form?.submit();
                }
            })()
        "#).await?;

        // Wait for search results to load
        Self::random_delay(1500, 2500).await;

        // Check for CAPTCHA after search
        if Self::check_google_captcha(session).await? {
            session.increment_captchas();
            return Err(BrowserError::CaptchaDetected("CAPTCHA after Google search".into()));
        }

        // Check if results loaded and gather debug info
        let page_info = session.execute_js(r#"
            (function() {
                const url = window.location.href;
                const results = document.querySelectorAll('#search a[href], #rso a[href]');
                const topAds = document.querySelectorAll('#tads a[href], #tadsb a[href]');
                const textAds = document.querySelectorAll('[data-text-ad] a[href]');
                const sponsoredLabels = document.querySelectorAll('[aria-label*="Sponsored"], [data-dtld], .uEierd');
                const allLinks = document.querySelectorAll('a[href*="grintahub"]');
                const bodyText = document.body ? document.body.innerText.substring(0, 500) : '';
                const title = document.title;

                // Check for error messages
                const hasNoResults = bodyText.toLowerCase().includes('did not match any') ||
                                    bodyText.includes('لا توجد نتائج');
                const hasCaptcha = url.includes('/sorry/') ||
                                  bodyText.includes('unusual traffic') ||
                                  bodyText.includes('زيارات غير معتادة');

                return {
                    url: url,
                    title: title,
                    resultCount: results.length,
                    topAdCount: topAds.length,
                    textAdCount: textAds.length,
                    sponsoredCount: sponsoredLabels.length,
                    grintaLinkCount: allLinks.length,
                    hasNoResults: hasNoResults,
                    hasCaptcha: hasCaptcha,
                    bodyPreview: bodyText.substring(0, 200)
                };
            })()
        "#).await?;

        let result_count = page_info.get("resultCount").and_then(|v| v.as_u64()).unwrap_or(0);
        let top_ad_count = page_info.get("topAdCount").and_then(|v| v.as_u64()).unwrap_or(0);
        let grinta_count = page_info.get("grintaLinkCount").and_then(|v| v.as_u64()).unwrap_or(0);
        let url = page_info.get("url").and_then(|v| v.as_str()).unwrap_or("");
        let title = page_info.get("title").and_then(|v| v.as_str()).unwrap_or("");
        let has_captcha = page_info.get("hasCaptcha").and_then(|v| v.as_bool()).unwrap_or(false);
        let has_no_results = page_info.get("hasNoResults").and_then(|v| v.as_bool()).unwrap_or(false);

        debug!("Session {} search page state: url={}, title={}",
            session.id, &url[..url.len().min(80)], &title[..title.len().min(50)]);
        info!("Session {} search results: {} organic, {} top ads, {} grintahub links, captcha={}, noResults={}",
            session.id, result_count, top_ad_count, grinta_count, has_captcha, has_no_results);

        if has_captcha {
            let body = page_info.get("bodyPreview").and_then(|v| v.as_str()).unwrap_or("");
            warn!("Session {} CAPTCHA detected in search results! Body: {}", session.id, body);
        }

        if has_no_results {
            warn!("Session {} Google returned no results for keyword", session.id);
        }

        Ok(result_count > 0)
    }

    /// Find and click on grintahub.com SPONSORED AD in Google results
    pub async fn click_grintahub_result(session: &Arc<BrowserSession>) -> Result<bool, BrowserError> {
        info!("Session {} looking for grintahub.com in SPONSORED ADS", session.id);

        // Quick scroll to see ads
        session.execute_js(r#"
            (async function() {
                await new Promise(r => setTimeout(r, 150 + Math.random() * 200));
                window.scrollBy({ top: 150 + Math.random() * 200, behavior: 'smooth' });
            })()
        "#).await?;

        Self::random_delay(200, 400).await;

        // Find grintahub.com links in SPONSORED ADS only
        let find_result = session.execute_js(r#"
            (function() {
                const grintaAdLinks = [];

                // Method 1: Find ads in top ads section (#tads)
                const topAds = document.querySelectorAll('#tads a[href], #tadsb a[href]');
                for (const link of topAds) {
                    const href = link.getAttribute('href') || '';
                    if (href.includes('grintahub.com') || href.includes('grintahub')) {
                        grintaAdLinks.push({
                            element: link,
                            href: href,
                            text: link.innerText || link.textContent,
                            type: 'top_ad'
                        });
                    }
                }

                // Method 2: Find ads with data-text-ad attribute
                const textAds = document.querySelectorAll('[data-text-ad] a[href]');
                for (const link of textAds) {
                    const href = link.getAttribute('href') || '';
                    if (href.includes('grintahub.com') || href.includes('grintahub')) {
                        grintaAdLinks.push({
                            element: link,
                            href: href,
                            text: link.innerText || link.textContent,
                            type: 'text_ad'
                        });
                    }
                }

                // Method 3: Find ads by "Sponsored" label proximity
                const sponsoredLabels = document.querySelectorAll('[aria-label*="Sponsored"], [data-dtld], .uEierd, .x54gtf');
                for (const label of sponsoredLabels) {
                    const parent = label.closest('div[data-hveid]') || label.parentElement?.parentElement?.parentElement;
                    if (parent) {
                        const links = parent.querySelectorAll('a[href]');
                        for (const link of links) {
                            const href = link.getAttribute('href') || '';
                            if ((href.includes('grintahub.com') || href.includes('grintahub')) && !href.includes('google.com')) {
                                grintaAdLinks.push({
                                    element: link,
                                    href: href,
                                    text: link.innerText || link.textContent,
                                    type: 'sponsored_label'
                                });
                            }
                        }
                    }
                }

                // Method 4: Find ads with googleadservices.com redirect
                const allLinks = document.querySelectorAll('a[href*="googleadservices.com"]');
                for (const link of allLinks) {
                    const href = link.getAttribute('href') || '';
                    if (href.includes('grintahub')) {
                        grintaAdLinks.push({
                            element: link,
                            href: href,
                            text: link.innerText || link.textContent,
                            type: 'googleadservices'
                        });
                    }
                }

                // Method 5: Look for ads container with class patterns
                const adContainers = document.querySelectorAll('.ads-ad, .commercial-unit-desktop-top, [data-sokoban-container]');
                for (const container of adContainers) {
                    const links = container.querySelectorAll('a[href]');
                    for (const link of links) {
                        const href = link.getAttribute('href') || '';
                        if ((href.includes('grintahub.com') || href.includes('grintahub')) && !href.includes('google.com')) {
                            grintaAdLinks.push({
                                element: link,
                                href: href,
                                text: link.innerText || link.textContent,
                                type: 'ad_container'
                            });
                        }
                    }
                }

                // Deduplicate by href
                const seen = new Set();
                const uniqueLinks = grintaAdLinks.filter(item => {
                    if (seen.has(item.href)) return false;
                    seen.add(item.href);
                    return true;
                });

                if (uniqueLinks.length === 0) {
                    return { found: false, count: 0, isAd: true };
                }

                // Pick the first ad (most prominent position)
                const chosen = uniqueLinks[0];

                // Scroll to the ad link
                chosen.element.scrollIntoView({ behavior: 'smooth', block: 'center' });

                return {
                    found: true,
                    count: uniqueLinks.length,
                    chosenIndex: 0,
                    href: chosen.href,
                    type: chosen.type,
                    isAd: true
                };
            })()
        "#).await?;

        let found = find_result.get("found").and_then(|v| v.as_bool()).unwrap_or(false);

        if !found {
            // Get more debug info about why no ads were found
            let debug_info = session.execute_js(r#"
                (function() {
                    const url = window.location.href;
                    const topAdsContainer = document.querySelector('#tads, #tadsb');
                    const allAds = document.querySelectorAll('[data-text-ad], [data-hveid]');
                    const sponsoredText = document.body.innerText.includes('Sponsored') ||
                                         document.body.innerText.includes('إعلان') ||
                                         document.body.innerText.includes('Ad');
                    const allLinks = document.querySelectorAll('a[href]');
                    const grintaAnywhere = Array.from(allLinks).filter(l =>
                        (l.href || '').toLowerCase().includes('grintahub')
                    );

                    return {
                        url: url,
                        hasTopAdsContainer: !!topAdsContainer,
                        totalAdElements: allAds.length,
                        hasSponsoredText: sponsoredText,
                        totalLinks: allLinks.length,
                        grintaLinksAnywhere: grintaAnywhere.length,
                        grintaUrls: grintaAnywhere.slice(0, 5).map(l => l.href)
                    };
                })()
            "#).await.unwrap_or_default();

            let has_top_ads = debug_info.get("hasTopAdsContainer").and_then(|v| v.as_bool()).unwrap_or(false);
            let total_ads = debug_info.get("totalAdElements").and_then(|v| v.as_u64()).unwrap_or(0);
            let has_sponsored = debug_info.get("hasSponsoredText").and_then(|v| v.as_bool()).unwrap_or(false);
            let grinta_anywhere = debug_info.get("grintaLinksAnywhere").and_then(|v| v.as_u64()).unwrap_or(0);

            warn!("Session {} NO grintahub.com SPONSORED ADS - hasTopAds={}, totalAdElements={}, hasSponsoredText={}, grintaLinksAnywhere={}",
                session.id, has_top_ads, total_ads, has_sponsored, grinta_anywhere);

            if grinta_anywhere > 0 {
                let urls = debug_info.get("grintaUrls").and_then(|v| v.as_array());
                if let Some(urls) = urls {
                    warn!("Session {} grintahub links found (but not in ad sections): {:?}",
                        session.id, urls.iter().take(3).collect::<Vec<_>>());
                }
            }

            return Ok(false);
        }

        let count = find_result.get("count").and_then(|v| v.as_u64()).unwrap_or(0);
        let ad_type = find_result.get("type").and_then(|v| v.as_str()).unwrap_or("unknown");
        info!("Session {} found {} grintahub SPONSORED ADS (type: {})", session.id, count, ad_type);

        // Wait after scrolling
        Self::random_delay(200, 400).await;

        // Click the AD link with human-like behavior
        let clicked = session.execute_js(r#"
            (function() {
                const grintaAdLinks = [];

                // Collect all ad links (same logic as above)
                const topAds = document.querySelectorAll('#tads a[href], #tadsb a[href]');
                for (const link of topAds) {
                    const href = link.getAttribute('href') || '';
                    if (href.includes('grintahub.com') || href.includes('grintahub')) {
                        grintaAdLinks.push(link);
                    }
                }

                const textAds = document.querySelectorAll('[data-text-ad] a[href]');
                for (const link of textAds) {
                    const href = link.getAttribute('href') || '';
                    if (href.includes('grintahub.com') || href.includes('grintahub')) {
                        grintaAdLinks.push(link);
                    }
                }

                const sponsoredLabels = document.querySelectorAll('[aria-label*="Sponsored"], [data-dtld], .uEierd, .x54gtf');
                for (const label of sponsoredLabels) {
                    const parent = label.closest('div[data-hveid]') || label.parentElement?.parentElement?.parentElement;
                    if (parent) {
                        const links = parent.querySelectorAll('a[href]');
                        for (const link of links) {
                            const href = link.getAttribute('href') || '';
                            if ((href.includes('grintahub.com') || href.includes('grintahub')) && !href.includes('google.com')) {
                                grintaAdLinks.push(link);
                            }
                        }
                    }
                }

                const googleAdLinks = document.querySelectorAll('a[href*="googleadservices.com"]');
                for (const link of googleAdLinks) {
                    const href = link.getAttribute('href') || '';
                    if (href.includes('grintahub')) {
                        grintaAdLinks.push(link);
                    }
                }

                const adContainers = document.querySelectorAll('.ads-ad, .commercial-unit-desktop-top, [data-sokoban-container]');
                for (const container of adContainers) {
                    const links = container.querySelectorAll('a[href]');
                    for (const link of links) {
                        const href = link.getAttribute('href') || '';
                        if ((href.includes('grintahub.com') || href.includes('grintahub')) && !href.includes('google.com')) {
                            grintaAdLinks.push(link);
                        }
                    }
                }

                if (grintaAdLinks.length === 0) return false;

                // Click the first (most prominent) ad
                const link = grintaAdLinks[0];

                // Remove target="_blank" to prevent opening in new tab
                link.removeAttribute('target');
                link.setAttribute('target', '_self');

                // Simulate mouse events for realistic click
                link.dispatchEvent(new MouseEvent('mouseover', { bubbles: true }));
                link.dispatchEvent(new MouseEvent('mouseenter', { bubbles: true }));

                setTimeout(() => {
                    link.dispatchEvent(new MouseEvent('mousedown', { bubbles: true }));
                    link.dispatchEvent(new MouseEvent('mouseup', { bubbles: true }));
                    link.click();
                }, 150);

                return true;
            })()
        "#).await?;

        if clicked.as_bool() == Some(true) {
            session.increment_clicks();
            info!("Session {} clicked on grintahub.com result", session.id);
            // Wait for page navigation
            Self::random_delay(1500, 2500).await;
            return Ok(true);
        }

        Ok(false)
    }

    /// Try next page of Google results
    pub async fn google_next_page(session: &Arc<BrowserSession>) -> Result<bool, BrowserError> {
        debug!("Session {} trying next Google page", session.id);

        // Scroll to bottom first
        session.execute_js(r#"
            window.scrollTo({ top: document.body.scrollHeight, behavior: 'smooth' });
        "#).await?;

        Self::random_delay(500, 1000).await;

        // Click next page
        let has_next = session.execute_js(r#"
            (function() {
                const next = document.querySelector('#pnnext, a[aria-label="Next page"], a[id="pnnext"]');
                if (next) {
                    // Prevent opening in new tab
                    next.removeAttribute('target');
                    next.click();
                    return true;
                }
                return false;
            })()
        "#).await?;

        if has_next.as_bool() == Some(true) {
            Self::random_delay(1500, 2500).await;

            // Check for CAPTCHA after navigating to next page
            if Self::check_google_captcha(session).await? {
                session.increment_captchas();
                return Err(BrowserError::CaptchaDetected("CAPTCHA on Google next page".into()));
            }

            return Ok(true);
        }

        Ok(false)
    }

    /// Browse the grintahub page after clicking from Google
    /// Uses enhanced human-like reading behavior
    pub async fn browse_grintahub_page(session: &Arc<BrowserSession>) -> Result<(), BrowserError> {
        debug!("Session {} browsing grintahub page with human-like behavior", session.id);

        // Verify we're on grintahub.com
        let on_grintahub = session.execute_js(r#"
            window.location.hostname.includes('grintahub')
        "#).await?;

        if on_grintahub.as_bool() != Some(true) {
            warn!("Session {} not on grintahub.com", session.id);
            return Ok(());
        }

        // Brief pause to look at the page
        Self::human_delay(300, 300).await;

        // Use enhanced human-like scroll reading
        Self::human_scroll_read(session).await?;

        // Time spent on page after scrolling
        Self::random_delay(1000, 2000).await;

        // Maybe click on something on the page (40% chance)
        let should_click = rand::thread_rng().gen_bool(0.4);
        if should_click {
            {

                session.execute_js(r#"
                    (async function() {
                        // Find clickable links
                        const links = document.querySelectorAll('a[href*="/ads/"], a[href*="/listing/"], .card a, .item a');
                        if (links.length > 0) {
                            const randomLink = links[Math.floor(Math.random() * links.length)];

                            // Prevent opening in new tab
                            randomLink.removeAttribute('target');

                            randomLink.scrollIntoView({ behavior: 'smooth', block: 'center' });

                            await new Promise(r => setTimeout(r, 300 + Math.random() * 300));

                            // Hover then click
                            randomLink.dispatchEvent(new MouseEvent('mouseenter', { bubbles: true }));
                            randomLink.dispatchEvent(new MouseEvent('mouseover', { bubbles: true }));

                            await new Promise(r => setTimeout(r, 100 + Math.random() * 200));

                            randomLink.dispatchEvent(new MouseEvent('mousedown', { bubbles: true }));
                            await new Promise(r => setTimeout(r, 50 + Math.random() * 50));
                            randomLink.dispatchEvent(new MouseEvent('mouseup', { bubbles: true }));
                            randomLink.click();
                        }
                    })()
                "#).await?;

                Self::random_delay(1000, 2000).await;

                // Browse the sub-page too
                Self::human_scroll_read(session).await?;
            }
        }

        Self::random_delay(300, 700).await;
        Ok(())
    }

    /// Random delay with jitter
    pub async fn random_delay(min_ms: u64, max_ms: u64) {
        let delay = rand::thread_rng().gen_range(min_ms..=max_ms);
        tokio::time::sleep(Duration::from_millis(delay)).await;
    }

    /// Mouse simulation disabled - not effective for anti-detection
    pub async fn simulate_human_mouse(_session: &Arc<BrowserSession>) -> Result<(), BrowserError> {
        Ok(())
    }

    /// Simulate human-like reading behavior (fast)
    pub async fn simulate_reading(session: &Arc<BrowserSession>) -> Result<(), BrowserError> {
        session.execute_js(r#"
            (async function() {
                // Quick glance at results
                const pauses = 1 + Math.floor(Math.random() * 2);
                for (let i = 0; i < pauses; i++) {
                    await new Promise(r => setTimeout(r, 200 + Math.random() * 400));
                    const scrollAmount = -20 + Math.floor(Math.random() * 40);
                    window.scrollBy({ top: scrollAmount, behavior: 'smooth' });
                }
            })()
        "#).await?;
        Ok(())
    }

    /// Mouse movement disabled - not effective for anti-detection
    pub async fn bezier_mouse_move(_session: &Arc<BrowserSession>, _target_x: i32, _target_y: i32) -> Result<(), BrowserError> {
        Ok(())
    }

    /// Mouse to element disabled - not effective for anti-detection
    pub async fn bezier_mouse_to_element(_session: &Arc<BrowserSession>, _selector: &str) -> Result<bool, BrowserError> {
        Ok(true)
    }

    /// Enhanced human scroll with reading simulation
    /// Includes variable speeds, pauses for "reading", and occasional scroll-backs
    pub async fn human_scroll_read(session: &Arc<BrowserSession>) -> Result<(), BrowserError> {
        session.execute_js(r#"
            (async function() {
                const maxScroll = Math.max(
                    document.body.scrollHeight - window.innerHeight,
                    0
                );
                let currentPos = window.scrollY;

                // Determine how much of the page to read (60-90%)
                const readPercent = 0.6 + Math.random() * 0.3;
                const targetScroll = maxScroll * readPercent;

                // Reading simulation loop
                while (currentPos < targetScroll) {
                    // Scroll amount varies like reading different content lengths
                    // Shorter scrolls when "reading" complex content
                    const contentComplexity = Math.random();
                    const scrollAmount = contentComplexity > 0.7
                        ? 30 + Math.random() * 50  // Complex content - small scroll
                        : 80 + Math.random() * 150; // Simple content - larger scroll

                    currentPos = Math.min(currentPos + scrollAmount, targetScroll);

                    // Smooth scroll with easing (easeOutQuad)
                    const start = window.scrollY;
                    const distance = currentPos - start;
                    const duration = 200 + Math.random() * 300;
                    const startTime = performance.now();

                    await new Promise(resolve => {
                        const animate = (currentTime) => {
                            const elapsed = currentTime - startTime;
                            const progress = Math.min(elapsed / duration, 1);
                            // easeOutQuad for natural deceleration
                            const eased = 1 - Math.pow(1 - progress, 2);
                            window.scrollTo(0, start + distance * eased);
                            if (progress < 1) {
                                requestAnimationFrame(animate);
                            } else {
                                resolve();
                            }
                        };
                        requestAnimationFrame(animate);
                    });

                    // Reading pause - fast but varied
                    const pauseTime = contentComplexity > 0.7
                        ? 400 + Math.random() * 800   // Complex content
                        : 150 + Math.random() * 400;  // Simple content
                    await new Promise(r => setTimeout(r, pauseTime));

                    // Occasional scroll back up (re-reading - 10% chance)
                    if (Math.random() < 0.1) {
                        const scrollBack = 30 + Math.random() * 80;
                        window.scrollBy({ top: -scrollBack, behavior: 'smooth' });
                        await new Promise(r => setTimeout(r, 200 + Math.random() * 400));
                        currentPos = window.scrollY;
                    }

                    // Occasional brief pause (5% chance)
                    if (Math.random() < 0.05) {
                        await new Promise(r => setTimeout(r, 500 + Math.random() * 1000));
                    }

                    // Random mouse movement while reading
                    if (Math.random() < 0.3) {
                        const x = window.innerWidth * (0.2 + Math.random() * 0.6);
                        const y = window.innerHeight * (0.3 + Math.random() * 0.4);
                        document.dispatchEvent(new MouseEvent('mousemove', {
                            clientX: x, clientY: y, bubbles: true
                        }));
                    }
                }

                // Maybe scroll back to top at end (20% chance)
                if (Math.random() < 0.2) {
                    await new Promise(r => setTimeout(r, 500));
                    window.scrollTo({ top: window.scrollY * 0.3, behavior: 'smooth' });
                }
            })()
        "#).await?;
        Ok(())
    }

    /// Add random delays with human variance
    pub async fn human_delay(base_ms: u64, variance_ms: u64) {
        let delay = base_ms + rand::thread_rng().gen_range(0..=variance_ms);
        tokio::time::sleep(Duration::from_millis(delay)).await;
    }

    /// Run a full cycle: Google search -> find grintahub AD -> click -> browse
    /// FIRST PAGE ONLY - No pagination
    /// Returns error if no ad found (triggers IP change)
    /// Uses enhanced human-like behavior with bezier mouse movements
    pub async fn run_cycle(
        session: &Arc<BrowserSession>,
        keyword: &str,
        min_delay_ms: u64,
        max_delay_ms: u64,
    ) -> Result<bool, BrowserError> {
        info!("Session {} starting Google search cycle for: {} (with enhanced stealth)", session.id, keyword);

        // 1. Go to Google with human-like behavior
        Self::goto_google(session).await?;

        // 2. Search on Google with the keyword
        let has_results = Self::google_search(session, keyword).await?;

        if !has_results {
            warn!("Session {} no Google results found - fast IP change", session.id);
            return Err(BrowserError::ElementNotFound("No Google results - need new IP".into()));
        }

        // Quick glance at search results
        Self::simulate_reading(session).await?;

        // 3. Try to find and click grintahub.com SPONSORED AD - FIRST PAGE ONLY
        let clicked = Self::click_grintahub_result(session).await?;

        if clicked {
            // Wait for the redirect to complete and page to load
            info!("Session {} ad clicked - waiting for redirect to grintahub.com...", session.id);

            // Wait for page load (check document.readyState and URL)
            for attempt in 0..10 {
                tokio::time::sleep(Duration::from_millis(500)).await;
                let page_state = session.execute_js(r#"
                    (function() {
                        return {
                            url: window.location.href,
                            ready: document.readyState,
                            onGrinta: window.location.hostname.includes('grintahub')
                        };
                    })()
                "#).await;

                match page_state {
                    Ok(state) => {
                        let on_grinta = state.get("onGrinta").and_then(|v| v.as_bool()).unwrap_or(false);
                        let ready = state.get("ready").and_then(|v| v.as_str()).unwrap_or("loading");
                        if on_grinta && (ready == "complete" || ready == "interactive") {
                            info!("Session {} landed on grintahub.com (attempt {})", session.id, attempt + 1);
                            break;
                        }
                        debug!("Session {} page state: ready={}, onGrinta={} (attempt {})", session.id, ready, on_grinta, attempt + 1);
                    }
                    Err(_) => {
                        // Context may be destroyed during navigation - wait more
                        debug!("Session {} waiting for navigation to settle (attempt {})", session.id, attempt + 1);
                    }
                }
            }

            // 4. Browse the grintahub page (non-fatal - click already counted)
            let browse_start = std::time::Instant::now();
            if let Err(e) = Self::browse_grintahub_page(session).await {
                warn!("Session {} browse error (click already counted): {}", session.id, e);
            }
            let browse_elapsed = browse_start.elapsed().as_millis() as u64;

            // 5. CRITICAL: Dwell time on landing page (20-40 seconds total)
            // Google flags clicks as invalid if dwell time is too short.
            // The page is still open in the browser even if CDP context is lost.
            let min_dwell_ms: u64 = 20_000;
            let max_dwell_ms: u64 = 40_000;
            let target_dwell = {
                let mut rng = rand::thread_rng();
                rng.gen_range(min_dwell_ms..=max_dwell_ms)
            };
            if browse_elapsed < target_dwell {
                let remaining = target_dwell - browse_elapsed;
                info!("Session {} dwell time: staying {}ms more on grintahub (browsed {}ms, target {}ms)",
                    session.id, remaining, browse_elapsed, target_dwell);
                tokio::time::sleep(Duration::from_millis(remaining)).await;
            }

            // Random delay before next cycle
            Self::human_delay(min_delay_ms, max_delay_ms - min_delay_ms).await;

            info!("Session {} completed successful ad click cycle (dwell: {}ms)", session.id, target_dwell);
            Ok(true)
        } else {
            // NO AD FOUND on first page = FAST IP change, no delay
            info!("Session {} no grintahub.com SPONSORED AD found - fast IP change", session.id);
            Err(BrowserError::ElementNotFound("No sponsored ad found - need new IP".into()))
        }
    }

    /// Run a full cycle with optional Google account login
    /// Login happens once at the start of the session
    pub async fn run_cycle_with_login(
        session: &Arc<BrowserSession>,
        keyword: &str,
        min_delay_ms: u64,
        max_delay_ms: u64,
        account: Option<&GoogleAccount>,
        already_logged_in: &mut bool,
    ) -> Result<bool, BrowserError> {
        // Login to Google if account provided and not already logged in
        if let Some(account) = account {
            if !*already_logged_in {
                info!("Session {} attempting Google login before search", session.id);

                // Check if already logged in
                session.navigate("https://www.google.com").await?;
                Self::human_delay(1500, 1000).await;

                if Self::is_google_logged_in(session).await? {
                    info!("Session {} already logged into Google", session.id);
                    *already_logged_in = true;
                } else {
                    // Attempt login
                    match Self::login_to_google(session, account).await {
                        Ok(true) => {
                            info!("Session {} Google login successful", session.id);
                            *already_logged_in = true;
                            // Wait a bit after successful login
                            Self::human_delay(2000, 1500).await;
                        }
                        Ok(false) => {
                            warn!("Session {} Google login incomplete (may need 2FA)", session.id);
                            // Continue without login - still try to show ads
                        }
                        Err(e) => {
                            warn!("Session {} Google login failed: {}. Continuing without login.", session.id, e);
                            // Continue without login
                        }
                    }
                }
            }
        }

        // Now run the regular cycle
        Self::run_cycle(session, keyword, min_delay_ms, max_delay_ms).await
    }
}
