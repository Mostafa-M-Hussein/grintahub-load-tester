//! Browser automation actions for organic Google traffic to grintahub.com
//!
//! Simulates real user behavior:
//! 1. Go to Google.com (optionally logged in with Google account)
//! 2. Search for keywords
//! 3. Find and click on grintahub.com in search results
//! 4. Browse the website naturally

use std::sync::Arc;
use std::time::Duration;
use rand::{Rng, SeedableRng};
use tracing::{info, debug, warn, error};

use super::{BrowserSession, BrowserError};
use crate::safe_truncate;

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

                // Extract data-s parameter (CRITICAL for Google /sorry/ pages)
                // This is a one-time-use token. If the reCAPTCHA widget loads
                // before we send it to the solver, the data-s is consumed and
                // the solved token will be rejected by Google.
                let dataS = null;
                if (recaptchaDiv) {
                    dataS = recaptchaDiv.getAttribute('data-s');
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
                    dataS: dataS,
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
        let data_s = captcha_info.get("dataS").and_then(|v| v.as_str()).map(|s| s.to_string());
        let is_google_sorry = captcha_info.get("isGoogleSorry").and_then(|v| v.as_bool()).unwrap_or(false);
        let is_enterprise = captcha_info.get("isEnterprise").and_then(|v| v.as_bool()).unwrap_or(false);
        let callback_name = captcha_info.get("callbackName").and_then(|v| v.as_str());
        let form_count = captcha_info.get("formCount").and_then(|v| v.as_u64()).unwrap_or(0);
        let body_preview = captcha_info.get("bodyPreview").and_then(|v| v.as_str()).unwrap_or("");

        info!("Session {} CAPTCHA page: google_sorry={}, enterprise={}, method={}, data_s={}, forms={}, callback={:?}, url={}",
            session.id, is_google_sorry, is_enterprise, method,
            data_s.as_ref().map(|s| format!("{}...({}chars)", safe_truncate(s, 20), s.len())).unwrap_or_else(|| "NONE".to_string()),
            form_count, callback_name, safe_truncate(&page_url, 80));
        debug!("Session {} page body: {}", session.id, body_preview);

        let sitekey = match captcha_info.get("sitekey").and_then(|v| v.as_str()) {
            Some(key) if !key.is_empty() => key.to_string(),
            _ => {
                warn!("Session {} could not find sitekey (method: {})", session.id, method);
                return Ok(false);
            }
        };

        info!("Session {} found sitekey: {} (via {})", session.id, safe_truncate(&sitekey, 20), method);

        // 2. Create solver and solve reCAPTCHA v2
        let solver = match crate::captcha::CaptchaSolver::new(captcha_api_key) {
            Ok(s) => s,
            Err(e) => {
                warn!("Session {} failed to create solver: {}", session.id, e);
                return Ok(false);
            }
        };

        // Use Enterprise variant for Google sorry pages (they use reCAPTCHA Enterprise)
        // CRITICAL: Pass data-s parameter for Google /sorry/ pages - without it,
        // the solved token will be rejected by Google even if 2Captcha solves it correctly.
        let request = if is_enterprise || is_google_sorry {
            if let Some(ref ds) = data_s {
                info!("Session {} using reCAPTCHA Enterprise solver WITH data-s ({}chars)", session.id, ds.len());
                crate::captcha::CaptchaRequest::recaptcha_v2_enterprise_with_data_s(&sitekey, &page_url, ds)
            } else {
                warn!("Session {} using reCAPTCHA Enterprise solver WITHOUT data-s (may fail!)", session.id);
                crate::captcha::CaptchaRequest::recaptcha_v2_enterprise(&sitekey, &page_url)
            }
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

        // 3. Inject token and submit via callback
        // CRITICAL: For Google /sorry/ pages, we MUST:
        //   a) Inject the token into g-recaptcha-response textareas
        //   b) Call the callback function (usually "submitCallback" from data-callback attr)
        //   c) Wait 5-10 seconds for Google to validate the token server-side
        //   d) Google will auto-redirect to search results if valid
        // Just calling form.submit() does NOT work — Google needs the callback path.
        let token = result.token.replace('\\', "\\\\").replace('\'', "\\'").replace('\n', "\\n");
        let callback_name_str = callback_name.unwrap_or("").to_string();

        let inject_script = format!(r#"
            (function() {{
                const token = '{}';
                const callbackName = '{}';
                let injected = false;
                let callbackCalled = false;
                let callbackMethod = 'none';

                // Step 1: Inject token into ALL g-recaptcha-response textareas
                const textareas = document.querySelectorAll('#g-recaptcha-response, textarea[name="g-recaptcha-response"]');
                for (const ta of textareas) {{
                    ta.style.display = 'block';
                    ta.innerHTML = token;
                    ta.value = token;
                    ta.style.display = 'none';
                    injected = true;
                }}

                // Also ensure hidden input exists in form
                const forms = document.querySelectorAll('form');
                for (const form of forms) {{
                    let hiddenInput = form.querySelector('input[name="g-recaptcha-response"]');
                    if (!hiddenInput) {{
                        hiddenInput = document.createElement('input');
                        hiddenInput.type = 'hidden';
                        hiddenInput.name = 'g-recaptcha-response';
                        form.appendChild(hiddenInput);
                    }}
                    hiddenInput.value = token;
                }}

                // Step 2: Call the data-callback function (PRIMARY submission method)
                // For Google /sorry/ pages this is typically "submitCallback"
                // The callback tells Google's JS to validate and submit the token
                if (callbackName && typeof window[callbackName] === 'function') {{
                    try {{
                        window[callbackName](token);
                        callbackCalled = true;
                        callbackMethod = 'data-callback: ' + callbackName;
                    }} catch(e) {{
                        callbackMethod = 'data-callback-error: ' + e.message;
                    }}
                }}

                // Step 3: Try ___grecaptcha_cfg callback (deep search in reCAPTCHA internals)
                if (!callbackCalled && typeof ___grecaptcha_cfg !== 'undefined') {{
                    try {{
                        const clients = ___grecaptcha_cfg.clients;
                        for (const ckey in clients) {{
                            if (callbackCalled) break;
                            const client = clients[ckey];
                            const searchObj = (obj, depth) => {{
                                if (callbackCalled || depth > 5 || !obj) return;
                                for (const key in obj) {{
                                    if (callbackCalled) break;
                                    if (typeof obj[key] === 'function' && key.toLowerCase().includes('callback')) {{
                                        try {{
                                            obj[key](token);
                                            callbackCalled = true;
                                            callbackMethod = 'grecaptcha_cfg.' + ckey + '.' + key;
                                        }} catch(e) {{}}
                                    }} else if (typeof obj[key] === 'object' && obj[key] !== null) {{
                                        searchObj(obj[key], depth + 1);
                                    }}
                                }}
                            }};
                            searchObj(client, 0);
                        }}
                    }} catch(e) {{}}
                }}

                // Step 4: Try well-known global callback names
                if (!callbackCalled) {{
                    const globalCallbacks = ['submitCallback', 'onCaptchaSuccess', 'captchaCallback', 'recaptchaCallback', 'onSuccess'];
                    for (const name of globalCallbacks) {{
                        if (typeof window[name] === 'function') {{
                            try {{
                                window[name](token);
                                callbackCalled = true;
                                callbackMethod = 'global: ' + name;
                                break;
                            }} catch(e) {{}}
                        }}
                    }}
                }}

                // Step 5: LAST RESORT - form submit (less reliable, Google may reject)
                // Only if no callback was found/worked
                let formSubmitted = false;
                if (!callbackCalled) {{
                    for (const form of forms) {{
                        const hasResponse = form.querySelector('[name="g-recaptcha-response"]') ||
                                           form.querySelector('#g-recaptcha-response');
                        if (hasResponse || forms.length === 1) {{
                            form.submit();
                            formSubmitted = true;
                            callbackMethod = 'form.submit (fallback)';
                            break;
                        }}
                    }}
                }}

                return {{
                    injected: injected,
                    callbackCalled: callbackCalled,
                    formSubmitted: formSubmitted,
                    callbackMethod: callbackMethod,
                    textareaCount: textareas.length
                }};
            }})()
        "#, token, callback_name_str);

        let inject_result = session.execute_js(&inject_script).await?;

        let injected = inject_result.get("injected").and_then(|v| v.as_bool()).unwrap_or(false);
        let callback_called = inject_result.get("callbackCalled").and_then(|v| v.as_bool()).unwrap_or(false);
        let form_submitted = inject_result.get("formSubmitted").and_then(|v| v.as_bool()).unwrap_or(false);
        let callback_method = inject_result.get("callbackMethod").and_then(|v| v.as_str()).unwrap_or("none");
        let textarea_count = inject_result.get("textareaCount").and_then(|v| v.as_u64()).unwrap_or(0);

        info!("Session {} token injection: injected={}, callback={}, formSubmit={}, method='{}', textareas={}",
            session.id, injected, callback_called, form_submitted, callback_method, textarea_count);

        if callback_called || form_submitted {
            // CRITICAL: Wait 5-10 seconds for Google to validate the token server-side.
            // Google's /sorry/ page sends the token to its backend for verification.
            // If we check too early, the redirect hasn't happened yet.
            let validation_wait = {
                let mut rng = rand::thread_rng();
                rng.gen_range(5000..=10000u64)
            };
            info!("Session {} waiting {}ms for Google to validate token (via {})...",
                session.id, validation_wait, callback_method);
            tokio::time::sleep(Duration::from_millis(validation_wait)).await;

            // Verify we left the CAPTCHA page
            let still_blocked = Self::check_google_captcha(session).await.unwrap_or(true);
            if !still_blocked {
                info!("Session {} CAPTCHA bypass SUCCESS - redirected to search results!", session.id);
                return Ok(true);
            }

            // Give it more time - Google validation can be slow
            warn!("Session {} still on CAPTCHA page after {}ms, waiting 5s more...", session.id, validation_wait);
            tokio::time::sleep(Duration::from_millis(5000)).await;

            let still_blocked2 = Self::check_google_captcha(session).await.unwrap_or(true);
            if !still_blocked2 {
                info!("Session {} CAPTCHA bypass SUCCESS (delayed validation)", session.id);
                return Ok(true);
            }

            warn!("Session {} CAPTCHA token rejected by Google (method: {})", session.id, callback_method);
            return Ok(false);
        } else if injected {
            warn!("Session {} token injected but NO callback found - trying form.submit() as last resort", session.id);
            let _ = session.execute_js(r#"
                const form = document.querySelector('form');
                if (form) form.submit();
            "#).await;

            // Wait for potential redirect
            tokio::time::sleep(Duration::from_millis(7000)).await;

            let still_blocked = Self::check_google_captcha(session).await.unwrap_or(true);
            if !still_blocked {
                info!("Session {} CAPTCHA resolved after manual form submit", session.id);
                return Ok(true);
            }
            return Ok(false);
        } else {
            warn!("Session {} could not inject token (no textarea found, {} textareas)", session.id, textarea_count);
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

        // TODO: Re-enable warmup browsing when proxy bandwidth is not an issue.
        // Pre-search warm-up was flooding the proxy with ad tracker requests (taboola,
        // pubmatic, doubleclick, etc.) causing 522 timeouts from Oxylabs.
        // let warmup_sites = [
        //     "https://www.wikipedia.org",
        //     "https://weather.com",
        //     "https://www.bbc.com",
        //     "https://www.reuters.com",
        //     "https://timeanddate.com",
        // ];
        // let warmup_idx = rand::thread_rng().gen_range(0..warmup_sites.len());
        // let warmup_url = warmup_sites[warmup_idx];
        // info!("Session {} pre-search warmup: visiting {}", session.id, warmup_url);
        // if let Err(e) = session.navigate(warmup_url).await {
        //     debug!("Session {} warmup navigation failed (non-fatal): {}", session.id, e);
        // } else {
        //     Self::human_delay(3000, 5000).await;
        //     if let Err(e) = session.scroll_human(300).await {
        //         debug!("Session {} warmup scroll failed (non-fatal): {}", session.id, e);
        //     }
        //     Self::human_delay(1000, 2000).await;
        // }

        // Navigate to Google Saudi Arabia
        session.navigate("https://www.google.com.sa/?hl=ar&gl=sa").await?;

        // Wait for Google homepage to fully load (poll readyState, not blind sleep)
        Self::wait_for_page_ready(session, 8000).await;
        // Human pause after seeing the page (eyes adjust, read logo)
        Self::human_delay(800, 600).await;

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
        Self::human_delay(500, 300).await;

        // Human-like behavior on Google homepage: CDP mouse movements (isTrusted: true)
        // Real users look around the page before clicking the search box
        {
            let mut rng = rand::rngs::StdRng::from_entropy();
            let moves = rng.gen_range(2..=4);
            for _ in 0..moves {
                let rand_x = rng.gen_range(200.0..1200.0);
                let rand_y = rng.gen_range(100.0..600.0);
                session.move_mouse_human(rand_x, rand_y).await.ok();
                Self::random_delay(300, 500).await;
            }
        }
        Self::human_delay(500, 500).await;

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

        // Find search input element position for CDP click (real mouse events)
        let input_info = session.execute_js(r#"
            (function() {
                const selectors = [
                    'textarea[name="q"]',
                    'input[name="q"]',
                    '#APjFqb',
                    '.gLFyf',
                    'input[type="text"][title*="Search"]',
                    'input[type="text"][title*="بحث"]',
                    'input[aria-label*="Search"]',
                    'input[aria-label*="بحث"]',
                    'textarea[aria-label*="Search"]',
                    'textarea[aria-label*="بحث"]'
                ];
                for (const sel of selectors) {
                    const el = document.querySelector(sel);
                    if (el && el.offsetParent !== null) {
                        const rect = el.getBoundingClientRect();
                        return {
                            found: true,
                            x: rect.left + rect.width / 2,
                            y: rect.top + rect.height / 2,
                            selector: sel
                        };
                    }
                }
                return { found: false };
            })()
        "#).await?;

        if input_info.get("found").and_then(|v| v.as_bool()) != Some(true) {
            warn!("Session {} could not find Google search input", session.id);
            return Ok(false);
        }

        let x = input_info.get("x").and_then(|v| v.as_f64()).unwrap_or(400.0);
        let y = input_info.get("y").and_then(|v| v.as_f64()).unwrap_or(300.0);

        // Move mouse towards the search input via CDP (isTrusted: true, physics-based bezier)
        session.move_mouse_human(x, y).await.ok();
        Self::random_delay(100, 200).await;

        // Click the search input via CDP (isTrusted: true mouse press/release)
        session.click_human_at(x, y).await?;

        // Select any existing text in the input (so new typing replaces it)
        session.execute_js(&format!(r#"
            (function() {{
                const el = document.elementFromPoint({}, {});
                if (el && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA')) {{
                    el.select();
                }}
            }})()
        "#, x, y)).await.ok();

        // Human thinking pause before typing (500-1500ms — like deciding what to type)
        Self::random_delay(500, 1000).await;

        // Type keyword with realistic typos and variable speed (physics-based)
        session.type_text_with_typos_cdp(keyword).await?;

        // Review what was typed (800-2000ms — humans glance at their query)
        Self::random_delay(800, 1200).await;

        // Press Enter via CDP (real keyboard event, not JS dispatchEvent)
        session.press_enter().await?;

        // Wait for search results to load — poll for URL change instead of blind sleep
        // The URL should change from google.com.sa/ to google.com.sa/search?q=...
        let mut search_submitted = false;
        for attempt in 0..12 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let url_check = session.execute_js(r#"window.location.href"#).await;
            if let Ok(url_val) = url_check {
                let url = url_val.as_str().unwrap_or("");
                if url.contains("/search") || url.contains("q=") {
                    debug!("Session {} search submitted (attempt {}, url: {})", session.id, attempt + 1, safe_truncate(url, 80));
                    search_submitted = true;
                    // Wait a bit more for results to render
                    tokio::time::sleep(Duration::from_millis(1000)).await;
                    break;
                }
            }
        }

        // Fallback: if Enter didn't submit, try JS form submit
        if !search_submitted {
            warn!("Session {} Enter key didn't submit search, trying JS form submit", session.id);
            session.execute_js(r#"
                (function() {
                    // Try submitting the search form
                    const form = document.querySelector('form[action="/search"]') ||
                                 document.querySelector('form[role="search"]') ||
                                 document.querySelector('form');
                    if (form) {
                        form.submit();
                        return 'form_submitted';
                    }
                    // Fallback: click search button
                    const btn = document.querySelector('input[name="btnK"]') ||
                                document.querySelector('button[type="submit"]');
                    if (btn) {
                        btn.click();
                        return 'button_clicked';
                    }
                    return 'no_form_found';
                })()
            "#).await?;
        }

        // Wait for search results to actually render in the DOM.
        // URL change alone doesn't mean results are visible — Google loads progressively.
        let mut results_rendered = false;
        for attempt in 0..15 {
            tokio::time::sleep(Duration::from_millis(400)).await;
            let check = session.execute_js(r#"
                (function() {
                    var ready = document.readyState;
                    var hasResults = document.querySelectorAll('#search a[href], #rso a[href]').length;
                    var hasAds = document.querySelectorAll('#tads a[href], [data-text-ad] a[href]').length;
                    var hasContainers = document.querySelectorAll('#rcnt, #search, #rso, #tads').length;
                    return {
                        ready: ready,
                        resultLinks: hasResults,
                        adLinks: hasAds,
                        containers: hasContainers
                    };
                })()
            "#).await;
            if let Ok(val) = check {
                let ready = val.get("ready").and_then(|v| v.as_str()).unwrap_or("loading");
                let result_links = val.get("resultLinks").and_then(|v| v.as_u64()).unwrap_or(0);
                let containers = val.get("containers").and_then(|v| v.as_u64()).unwrap_or(0);
                if (ready == "complete" || ready == "interactive") && (result_links > 0 || containers > 0) {
                    debug!("Session {} search results rendered: {} links, {} ads, readyState={} (attempt {})",
                        session.id, result_links,
                        val.get("adLinks").and_then(|v| v.as_u64()).unwrap_or(0),
                        ready, attempt + 1);
                    results_rendered = true;
                    break;
                }
            }
        }
        if !results_rendered {
            warn!("Session {} search results may not have fully rendered", session.id);
        }

        // Human reading pause — eyes need time to see the results appear
        Self::human_delay(1500, 1000).await;

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
                const topAds = document.querySelectorAll('#tads a[href], #tadsb a[href], [data-text-ad] a[href], [data-dtld] a[href]');
                const textAds = document.querySelectorAll('[data-text-ad] a[href]');
                const sponsoredLabels = document.querySelectorAll('[aria-label*="Sponsored"], [aria-label*="إعلان"], [aria-label*="إعلانية"], [data-dtld], .uEierd');
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

        info!("Session {} search page: url={}, title={}",
            session.id, safe_truncate(url, 100), safe_truncate(title, 50));
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

    /// Helper function to scan for target domain ads on the page
    /// Find target ad AND click it in a single JS execution.
    /// Returns { clicked: bool, count: int, type: str, debug: {} }
    /// This avoids the DOM-state-change bug where scan finds ads but a separate click call can't.
    async fn find_and_click_target_ad(session: &Arc<BrowserSession>, targets: &[String]) -> Result<serde_json::Value, BrowserError> {
        // Build JS array of target domains and their simplified forms
        let targets_js = targets.iter()
            .map(|d| {
                // For each target, create both full domain and simplified forms
                // e.g., "grintahub.com" -> ["grintahub.com", "grintahub"]
                // e.g., "golden4tic.com" -> ["golden4tic.com", "golden4tic"]
                let simple = d.replace(".com", "").replace(".net", "").replace(".org", "");
                format!("'{}', '{}'", d.replace("'", "\\'"), simple.replace("'", "\\'"))
            })
            .collect::<Vec<_>>()
            .join(", ");

        let js_code = format!(r#"
            (async function() {{
                // Target domains to look for (dynamically injected)
                const targets = [{targets_js}];
                const matchesTarget = (str) => {{
                    const s = (str || '').toLowerCase();
                    return targets.some(t => s.includes(t.toLowerCase()));
                }};

                // Clean up any previous ad target markers
                document.querySelectorAll('[data-grinta-ad-target]').forEach(el => el.removeAttribute('data-grinta-ad-target'));

                // ===== Step 1: Find ALL target domain links on the page =====
                // Pass 1: Links with target domain directly in href (organic results)
                const allTargetLinks = [];
                const seen = new Set(); // track by element reference
                const allLinks = document.querySelectorAll('a[href]');
                for (const link of allLinks) {{
                    const href = link.getAttribute('href') || '';
                    if (matchesTarget(href) && !href.includes('google.com') && !href.includes('google.co')) {{
                        allTargetLinks.push(link);
                        seen.add(link);
                    }}
                }}

                // Pass 2: Google Ad links that REDIRECT to target domain
                // Ad links use googleadservices.com/pagead/aclk?... as href
                // The target domain text is in a sibling <cite> or display URL element
                // We find ad blocks that mention target domain and collect ALL their links
                const adContainers = document.querySelectorAll('#tads, #tadsb, #bottomads');
                let pass2Count = 0;
                for (const container of adContainers) {{
                    // Find individual ad blocks within the container
                    // Each ad is typically a div with data-dtld or data-text-ad, or a direct child block
                    const adBlocks = container.querySelectorAll('[data-dtld], [data-text-ad], .uEierd, .x54gtf');
                    const blocksToCheck = adBlocks.length > 0 ? adBlocks : [container];
                    for (const block of blocksToCheck) {{
                        // Check if this ad block is for target domain
                        const dtld = block.getAttribute('data-dtld') || '';
                        const blockText = (block.innerText || '').toLowerCase();
                        const citeEls = block.querySelectorAll('cite, .qzEoUe, .NJjxre, [data-dtld]');
                        let mentionsTarget = matchesTarget(dtld);
                        if (!mentionsTarget) {{
                            for (const cite of citeEls) {{
                                if (matchesTarget(cite.textContent)) {{
                                    mentionsTarget = true;
                                    break;
                                }}
                            }}
                        }}
                        if (!mentionsTarget) {{
                            mentionsTarget = matchesTarget(blockText);
                        }}

                        if (mentionsTarget) {{
                            // This ad block is for target domain — collect ALL its <a> links
                            const blockLinks = block.querySelectorAll('a[href]');
                            for (const link of blockLinks) {{
                                if (!seen.has(link)) {{
                                    allTargetLinks.push(link);
                                    seen.add(link);
                                    pass2Count++;
                                }}
                            }}
                        }}
                    }}
                }}

                if (allTargetLinks.length === 0) {{
                    return {{ clicked: false, count: 0, debug: {{
                        reason: 'no_target_links_on_page',
                        targets: targets,
                        hasTopAds: !!document.querySelector('#tads'),
                        hasBottomAds: !!document.querySelector('#tadsb, #bottomads'),
                        adContainerCount: adContainers.length
                    }}}};
                }}

                // ===== Step 2: Classify each link as AD or ORGANIC =====
                const adLinks = [];
                const organicLinks = [];

                for (const link of allTargetLinks) {{
                    const href = link.getAttribute('href') || '';
                    let isAd = false;
                    let adType = 'organic';

                    // Check 1: href contains googleadservices or aclk (definite ad click tracking)
                    if (href.includes('googleadservices.com') || href.includes('/aclk?') || href.includes('?aclk')) {{
                        isAd = true;
                        adType = 'adservice';
                    }}

                    // Check 2: link is inside #tads or #tadsb (Google ad containers)
                    if (!isAd && (link.closest('#tads') || link.closest('#tadsb') || link.closest('#bottomads'))) {{
                        isAd = true;
                        adType = 'tads_container';
                    }}

                    // Check 3: link is inside an element with data-text-ad
                    if (!isAd && link.closest('[data-text-ad]')) {{
                        isAd = true;
                        adType = 'text_ad_attr';
                    }}

                    // Check 4: link is inside data-dtld container (Google ad data attribute)
                    if (!isAd && link.closest('[data-dtld]')) {{
                        isAd = true;
                        adType = 'data_dtld';
                    }}

                    // Check 5: Walk UP from the link looking for ad label text
                    // Arabic Google shows "نتيجة إعلانية" (Advertising result) as the ad label
                    if (!isAd) {{
                        let ancestor = link.parentElement;
                        for (let i = 0; i < 12 && ancestor && ancestor !== document.body; i++) {{
                            const txt = ancestor.innerText || '';
                            const txtLower = txt.toLowerCase();
                            if (txt.includes('نتيجة إعلانية') ||
                                txt.includes('إعلانية') ||
                                txt.includes('إعلان') ||
                                txt.includes('ممول') ||
                                txt.includes('مُموَّل') ||
                                txtLower.includes('sponsored') ||
                                txtLower.includes('ad ·') ||
                                txtLower.includes('· ad')) {{
                                // Make sure this is an ad block, not the whole page
                                if (ancestor.offsetHeight < 800 && ancestor.offsetHeight > 20) {{
                                    isAd = true;
                                    adType = 'text_label';
                                    break;
                                }}
                            }}
                            // Also check aria-label
                            const ariaLabel = ancestor.getAttribute('aria-label') || '';
                            if (ariaLabel.includes('Sponsored') || ariaLabel.includes('إعلان') ||
                                ariaLabel.includes('ممول') || ariaLabel.includes('إعلانية') ||
                                ariaLabel.includes('نتيجة إعلانية')) {{
                                isAd = true;
                                adType = 'aria_label';
                                break;
                            }}
                            ancestor = ancestor.parentElement;
                        }}
                    }}

                    // Check 6: Known ad CSS classes
                    if (!isAd) {{
                        const el = link.closest('.uEierd, .x54gtf, .d5oMvf, .nMdasd, .ads-ad, .commercial-unit-desktop-top, [data-sokoban-container], .cu-container, .pla-unit');
                        if (el) {{
                            isAd = true;
                            adType = 'css_class';
                        }}
                    }}

                    const linkText = (link.innerText || link.textContent || '').trim();
                    const rect = link.getBoundingClientRect();
                    const entry = {{
                        el: link,
                        href: href,
                        text: linkText.substring(0, 100),
                        type: adType,
                        y: rect.top,
                        isSitelink: linkText.length > 0 && linkText.length <= 40,
                        visible: rect.width > 0 && rect.height > 0 && rect.top > -100 && rect.top < window.innerHeight + 100
                    }};

                    if (isAd) {{
                        adLinks.push(entry);
                    }} else {{
                        organicLinks.push(entry);
                    }}
                }}

                // ===== Step 3: Choose the best link to click =====
                // Priority: visible ad sitelink > visible ad link > organic fallback
                let chosen = null;

                const visibleAdLinks = adLinks.filter(l => l.visible && l.text.length > 0);

                if (visibleAdLinks.length > 0) {{
                    // Prefer sitelinks (shorter text = specific landing pages)
                    const sitelinks = visibleAdLinks.filter(l => l.isSitelink);
                    if (sitelinks.length > 0) {{
                        chosen = sitelinks[Math.floor(Math.random() * sitelinks.length)];
                        chosen.type = chosen.type + '_sitelink';
                    }} else {{
                        chosen = visibleAdLinks[0];
                    }}
                }} else if (adLinks.length > 0) {{
                    chosen = adLinks[0]; // Not visible but still an ad
                }}

                // NO organic fallback — only click actual Google Ads campaign links
                // If no ad found, return false so bot rotates IP

                const bodyText = document.body ? document.body.innerText : '';
                const debug = {{
                    totalTargetLinks: allTargetLinks.length,
                    adLinksFound: adLinks.length,
                    organicLinksFound: organicLinks.length,
                    adRedirectLinks: pass2Count,
                    hasTopAds: !!document.querySelector('#tads'),
                    hasBottomAds: !!document.querySelector('#tadsb, #bottomads'),
                    pageHasAdLabel: bodyText.includes('نتيجة إعلانية') || bodyText.includes('إعلان') || bodyText.includes('Sponsored'),
                    adTypes: adLinks.map(l => l.type + ':' + l.text.substring(0, 30)).join(' | '),
                    scrollY: window.scrollY,
                    targets: targets
                }};

                if (!chosen) {{
                    return {{ clicked: false, count: 0, debug }};
                }}

                // ===== Step 4: Prepare chosen link for CDP click =====
                // Mark this element so fallback JS can re-find it
                chosen.el.dataset.grintaAdTarget = 'true';
                chosen.el.removeAttribute('target');
                chosen.el.setAttribute('target', '_self');

                // Scroll into view (instant, not smooth — avoid timing issues)
                chosen.el.scrollIntoView({{ behavior: 'instant', block: 'center' }});
                await new Promise(r => setTimeout(r, 300));

                // Validate coordinates are within viewport
                let finalRect = chosen.el.getBoundingClientRect();
                if (finalRect.top < 0 || finalRect.bottom > window.innerHeight) {{
                    // Re-scroll if element not in viewport
                    chosen.el.scrollIntoView({{ behavior: 'instant', block: 'center' }});
                    await new Promise(r => setTimeout(r, 200));
                    finalRect = chosen.el.getBoundingClientRect();
                }}

                // Randomized click position within the link (humans don't click exact center)
                // Use gaussian-like distribution: mostly near center but with natural variance
                const vpH = window.innerHeight;
                const randOffset = () => (Math.random() + Math.random() + Math.random()) / 3; // pseudo-gaussian 0-1
                const rawX = finalRect.left + finalRect.width * (0.2 + randOffset() * 0.6); // 20%-80% of width
                const rawY = finalRect.top + finalRect.height * (0.25 + randOffset() * 0.5); // 25%-75% of height
                const clickX = Math.min(Math.max(rawX, 10), window.innerWidth - 10);
                const clickY = Math.min(Math.max(rawY, 10), vpH - 10);

                return {{
                    clicked: false,
                    ready_for_cdp_click: true,
                    count: adLinks.length,
                    organic_count: organicLinks.length,
                    type: chosen.type,
                    href: chosen.href,
                    text: chosen.text,
                    x: clickX,
                    y: clickY,
                    debug
                }};
            }})()
        "#, targets_js = targets_js);
        session.execute_js(&js_code).await
    }

    /// Perform click on an ad using MULTIPLE methods until one works.
    /// Tries 7 different click strategies — from most natural (CDP) to most reliable (direct nav).
    /// Returns true if any method navigated away from the search page.
    async fn cdp_click_ad(session: &Arc<BrowserSession>, result: &serde_json::Value, phase: &str, targets: &[String]) -> Result<bool, BrowserError> {
        let ready = result.get("ready_for_cdp_click").and_then(|v| v.as_bool()).unwrap_or(false);
        if !ready {
            return Ok(false);
        }

        let x = result.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let y = result.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let count = result.get("count").and_then(|v| v.as_u64()).unwrap_or(0);
        let ad_type = result.get("type").and_then(|v| v.as_str()).unwrap_or("?");
        let href = result.get("href").and_then(|v| v.as_str()).unwrap_or("");
        let text = result.get("text").and_then(|v| v.as_str()).unwrap_or("");

        info!("Session {} clicking target ad {} ({} found, type: {}, text: '{}')",
            session.id, phase, count, ad_type, safe_truncate(text, 60));

        // Build JS target matching code for fallback methods
        let targets_js: String = targets.iter()
            .map(|d| {
                let simple = d.replace(".com", "").replace(".net", "").replace(".org", "");
                format!("'{}', '{}'", d.replace('\'', "\\'"), simple.replace('\'', "\\'"))
            })
            .collect::<Vec<_>>()
            .join(", ");

        // Record URL before click
        let url_before = session.execute_js("window.location.href").await
            .ok().and_then(|v| v.as_str().map(|s| s.to_string())).unwrap_or_default();

        // Helper: poll for navigation (URL change is sufficient — don't wait for readyState)
        // Uses both JS and CDP-level URL check. CDP works even when page is stuck loading
        // (e.g., target site unreachable through proxy — JS hangs but CDP still reports the URL).
        // Returns (navigated, url_after, is_error, is_network_error)
        // is_network_error = true means Chrome navigated but target was unreachable (ERR_TIMED_OUT etc.)
        async fn poll_navigation(session: &Arc<BrowserSession>, url_before: &str, max_polls: u32) -> (bool, String, bool, bool) {
            let mut url_after = String::new();
            let mut navigated = false;
            for poll in 0..max_polls {
                tokio::time::sleep(Duration::from_millis(400)).await;

                // Try 1: JS-level check (fast, gets readyState too)
                let state = session.execute_js_with_timeout(
                    r#"({ url: window.location.href, ready: document.readyState })"#,
                    5
                ).await;
                if let Ok(s) = &state {
                    let url = s.get("url").and_then(|v| v.as_str()).unwrap_or("");
                    if url != url_before && !url.is_empty() {
                        url_after = url.to_string();
                        navigated = true;
                        let ready = s.get("ready").and_then(|v| v.as_str()).unwrap_or("loading");
                        debug!("Navigation detected via JS (poll {}, readyState={})", poll + 1, ready);
                        break;
                    }
                } else {
                    // Try 2: CDP-level URL check — works even when page is stuck loading
                    // (JS fails when page context is destroyed/loading through dead proxy)
                    if let Ok(cdp_url) = session.get_current_url().await {
                        if cdp_url != url_before && !cdp_url.is_empty() {
                            url_after = cdp_url;
                            navigated = true;
                            debug!("Navigation detected via CDP (poll {}, JS was unavailable)", poll + 1);
                            break;
                        }
                    }
                }
            }

            // Check for Chrome error page URLs
            let is_error = url_after.starts_with("chrome-error://")
                || url_after.starts_with("about:")
                || url_after.starts_with("chrome://");

            // If no URL change detected, check if Chrome is showing a network error page
            // (e.g., ERR_TIMED_OUT, ERR_CONNECTION_RESET). The URL stays the same but
            // document.title contains the error code — meaning navigation was attempted.
            let mut is_network_error = false;
            if !navigated {
                let err_check = session.execute_js_with_timeout(
                    r#"({
                        title: document.title || '',
                        bodyText: (document.body && document.body.innerText || '').substring(0, 500)
                    })"#,
                    3
                ).await;
                if let Ok(v) = &err_check {
                    let title = v.get("title").and_then(|v| v.as_str()).unwrap_or("");
                    let body = v.get("bodyText").and_then(|v| v.as_str()).unwrap_or("");
                    let combined = format!("{} {}", title, body);
                    let error_indicators = [
                        "ERR_TIMED_OUT", "ERR_CONNECTION_RESET", "ERR_CONNECTION_REFUSED",
                        "ERR_CONNECTION_CLOSED", "ERR_PROXY_CONNECTION_FAILED",
                        "ERR_TUNNEL_CONNECTION_FAILED", "ERR_NAME_NOT_RESOLVED",
                        "ERR_SSL_PROTOCOL_ERROR", "ERR_CONNECTION_TIMED_OUT",
                    ];
                    if error_indicators.iter().any(|e| combined.contains(e)) {
                        is_network_error = true;
                        navigated = true; // Click did trigger navigation, target just unreachable
                        debug!("Network error page detected (title: '{}') — click navigated but target unreachable", title);
                    }
                }
            }

            (navigated, url_after, is_error, is_network_error)
        }

        // Helper: go back to search page after error
        async fn go_back(session: &Arc<BrowserSession>) {
            session.execute_js("window.history.back()").await.ok();
            tokio::time::sleep(Duration::from_millis(2000)).await;
        }

        // ================================================================
        // PRE-CLICK BEHAVIOR: Hover over other results first (anti-detection)
        // Real users scan multiple results before deciding which to click
        // ================================================================
        {
            // Get positions of other search results to hover over
            let other_results = session.execute_js(r#"
                (function() {
                    const results = [];
                    // Get organic results (not ads) to hover over
                    document.querySelectorAll('div.g h3, div[data-hveid] h3').forEach((h3, i) => {
                        if (i < 5) {
                            const rect = h3.getBoundingClientRect();
                            if (rect.top > 0 && rect.top < window.innerHeight) {
                                results.push({
                                    x: rect.left + rect.width / 2,
                                    y: rect.top + rect.height / 2
                                });
                            }
                        }
                    });
                    return results;
                })()
            "#).await.unwrap_or_default();

            // Hover over 1-3 other results before the target
            if let Some(results) = other_results.as_array() {
                // Generate hover_count before any await (RNG is not Send)
                let hover_count = {
                    use rand::Rng;
                    rand::thread_rng().gen_range(1..=3).min(results.len())
                };
                for i in 0..hover_count {
                    if let Some(r) = results.get(i) {
                        let hx = r.get("x").and_then(|v| v.as_f64()).unwrap_or(300.0);
                        let hy = r.get("y").and_then(|v| v.as_f64()).unwrap_or(200.0);
                        // Move to this result
                        session.move_mouse_human(hx, hy).await.ok();
                        // Pause as if reading the title (300-800ms)
                        Self::random_delay(300, 800).await;
                    }
                }
            }
        }

        // ================================================================
        // METHOD 1: CDP mouse hover + bezier click (isTrusted: true)
        // ================================================================
        info!("Session {} METHOD 1: CDP hover + bezier click", session.id);
        {
            let (scan_x, scan_y) = {
                use rand::Rng;
                let mut rng = rand::thread_rng();
                (x + rng.gen_range(-80.0..80.0_f64), (y - rng.gen_range(60.0..140.0_f64)).max(10.0))
            };
            session.move_mouse_human(scan_x, scan_y).await.ok();
            Self::random_delay(300, 600).await;
            session.move_mouse_human(x, y).await.ok();
            // Extended pre-click delay: 500-2000ms (human decision time)
            Self::random_delay(500, 2000).await;
        }
        session.click_human_at(x, y).await?;

        let (nav, url_after, is_err, net_err) = poll_navigation(session, &url_before, 12).await;
        if nav && !is_err && !net_err {
            info!("Session {} METHOD 1 SUCCESS -> {}", session.id, safe_truncate(&url_after, 80));
            return Ok(true);
        }
        if net_err {
            info!("Session {} METHOD 1 click navigated but target unreachable — counting as success (Google tracked the click)", session.id);
            return Ok(true);
        }
        if is_err { go_back(session).await; }

        // ================================================================
        // METHOD 2: CDP click at FRESH coordinates (element may have shifted)
        // ================================================================
        if !session.is_alive() {
            warn!("Session {} CDP connection dead before METHOD 2, bailing", session.id);
            return Ok(false);
        }
        warn!("Session {} METHOD 1 failed, trying METHOD 2: CDP fresh-coord click", session.id);
        let fresh_coords = session.execute_js(r#"
            (function() {
                let link = document.querySelector('a[data-grinta-ad-target="true"]');
                if (!link) return null;
                link.scrollIntoView({ behavior: 'instant', block: 'center' });
                const r = link.getBoundingClientRect();
                if (r.width === 0 || r.height === 0) return null;
                const rx = () => (Math.random() + Math.random() + Math.random()) / 3;
                return {
                    x: Math.min(Math.max(r.left + r.width * (0.2 + rx() * 0.6), 10), window.innerWidth - 10),
                    y: Math.min(Math.max(r.top + r.height * (0.25 + rx() * 0.5), 10), window.innerHeight - 10)
                };
            })()
        "#).await.ok();

        if let Some(ref coords) = fresh_coords {
            if !coords.is_null() {
                let fx = coords.get("x").and_then(|v| v.as_f64()).unwrap_or(x);
                let fy = coords.get("y").and_then(|v| v.as_f64()).unwrap_or(y);
                Self::random_delay(200, 400).await;
                session.click_human_at(fx, fy).await?;

                let (nav, url_after, is_err, net_err) = poll_navigation(session, &url_before, 10).await;
                if nav && !is_err && !net_err {
                    info!("Session {} METHOD 2 SUCCESS -> {}", session.id, safe_truncate(&url_after, 80));
                    return Ok(true);
                }
                if net_err {
                    info!("Session {} METHOD 2 click navigated but target unreachable — counting as success", session.id);
                    return Ok(true);
                }
                if is_err { go_back(session).await; }
            }
        }

        // ================================================================
        // METHOD 3: JS full mouse event sequence + native .click()
        // ================================================================
        if !session.is_alive() {
            warn!("Session {} CDP connection dead before METHOD 3, bailing", session.id);
            return Ok(false);
        }
        warn!("Session {} METHOD 2 failed, trying METHOD 3: JS mouse events + .click()", session.id);
        let js_result = session.execute_js(&format!(r#"
            (async function() {{
                const targets = [{targets_js}];
                const matchesTarget = (str) => {{ const s = (str || '').toLowerCase(); return targets.some(t => s.includes(t.toLowerCase())); }};
                let link = document.querySelector('a[data-grinta-ad-target="true"]');
                if (!link) {{
                    const containers = document.querySelectorAll('#tads, #tadsb, #bottomads');
                    for (const c of containers) {{
                        const blocks = c.querySelectorAll('[data-dtld], [data-text-ad], .uEierd');
                        const toCheck = blocks.length > 0 ? blocks : [c];
                        for (const block of toCheck) {{
                            const dtld = block.getAttribute('data-dtld') || '';
                            const txt = (block.innerText || '').toLowerCase();
                            const cites = block.querySelectorAll('cite');
                            let found = matchesTarget(dtld) || matchesTarget(txt);
                            if (!found) {{ for (const ci of cites) {{ if (matchesTarget(ci.textContent)) {{ found = true; break; }} }} }}
                            if (found) {{ link = block.querySelector('a[href]'); if (link) break; }}
                        }}
                        if (link) break;
                    }}
                }}
                if (!link) return {{ clicked: false, reason: 'element_not_found' }};

                link.removeAttribute('target');
                link.setAttribute('target', '_self');
                link.scrollIntoView({{ behavior: 'instant', block: 'center' }});
                await new Promise(r => setTimeout(r, 200));
                const rect = link.getBoundingClientRect();
                const cx = rect.left + rect.width / 2;
                const cy = rect.top + rect.height / 2;

                const opts = {{ bubbles: true, cancelable: true, view: window, clientX: cx, clientY: cy }};
                link.dispatchEvent(new MouseEvent('mouseenter', opts));
                link.dispatchEvent(new MouseEvent('mouseover', opts));
                await new Promise(r => setTimeout(r, 30));
                link.dispatchEvent(new MouseEvent('mousedown', {{ ...opts, button: 0 }}));
                await new Promise(r => setTimeout(r, 80 + Math.random() * 40));
                link.dispatchEvent(new MouseEvent('mouseup', {{ ...opts, button: 0 }}));
                await new Promise(r => setTimeout(r, 10));
                link.dispatchEvent(new MouseEvent('click', {{ ...opts, button: 0 }}));

                await new Promise(r => setTimeout(r, 100));
                link.click();

                return {{ clicked: true, method: 'js_full_events', href: link.getAttribute('href') || '' }};
            }})()
        "#, targets_js = targets_js)).await.ok();

        let js_clicked = js_result.as_ref()
            .and_then(|v| v.get("clicked"))
            .and_then(|v| v.as_bool()).unwrap_or(false);

        if js_clicked {
            let (nav, url_after, is_err, net_err) = poll_navigation(session, &url_before, 10).await;
            if nav && !is_err && !net_err {
                info!("Session {} METHOD 3 SUCCESS -> {}", session.id, safe_truncate(&url_after, 80));
                return Ok(true);
            }
            if net_err {
                info!("Session {} METHOD 3 click navigated but target unreachable — counting as success", session.id);
                return Ok(true);
            }
            if is_err { go_back(session).await; }
        } else {
            let reason = js_result.as_ref()
                .and_then(|v| v.get("reason"))
                .and_then(|v| v.as_str()).unwrap_or("unknown");
            warn!("Session {} METHOD 3 element not found: {}", session.id, reason);
        }

        // ================================================================
        // METHOD 4: Focus element + CDP Enter keypress
        // ================================================================
        if !session.is_alive() {
            warn!("Session {} CDP connection dead before METHOD 4, bailing", session.id);
            return Ok(false);
        }
        warn!("Session {} METHOD 3 failed, trying METHOD 4: Focus + Enter key", session.id);
        let focus_result = session.execute_js(&format!(r#"
            (function() {{
                const targets = [{targets_js}];
                const matchesTarget = (str) => {{ const s = (str || '').toLowerCase(); return targets.some(t => s.includes(t.toLowerCase())); }};
                let link = document.querySelector('a[data-grinta-ad-target="true"]');
                if (!link) {{
                    const containers = document.querySelectorAll('#tads, #tadsb, #bottomads');
                    for (const c of containers) {{
                        const blocks = c.querySelectorAll('[data-dtld], [data-text-ad], .uEierd');
                        const toCheck = blocks.length > 0 ? blocks : [c];
                        for (const block of toCheck) {{
                            const dtld = block.getAttribute('data-dtld') || '';
                            const txt = (block.innerText || '').toLowerCase();
                            let found = matchesTarget(dtld) || matchesTarget(txt);
                            if (found) {{ link = block.querySelector('a[href]'); if (link) break; }}
                        }}
                        if (link) break;
                    }}
                }}
                if (!link) return {{ focused: false }};
                link.removeAttribute('target');
                link.setAttribute('target', '_self');
                link.focus();
                return {{ focused: true, href: link.getAttribute('href') || '' }};
            }})()
        "#, targets_js = targets_js)).await.ok();

        let focused = focus_result.as_ref()
            .and_then(|v| v.get("focused"))
            .and_then(|v| v.as_bool()).unwrap_or(false);

        if focused {
            // Send Enter keypress via CDP
            Self::random_delay(100, 300).await;
            session.execute_js(r#"
                document.activeElement && document.activeElement.dispatchEvent(
                    new KeyboardEvent('keydown', { key: 'Enter', code: 'Enter', keyCode: 13, bubbles: true })
                );
                document.activeElement && document.activeElement.dispatchEvent(
                    new KeyboardEvent('keypress', { key: 'Enter', code: 'Enter', keyCode: 13, bubbles: true })
                );
                document.activeElement && document.activeElement.dispatchEvent(
                    new KeyboardEvent('keyup', { key: 'Enter', code: 'Enter', keyCode: 13, bubbles: true })
                );
            "#).await.ok();

            let (nav, url_after, is_err, net_err) = poll_navigation(session, &url_before, 10).await;
            if nav && !is_err && !net_err {
                info!("Session {} METHOD 4 SUCCESS -> {}", session.id, safe_truncate(&url_after, 80));
                return Ok(true);
            }
            if net_err {
                info!("Session {} METHOD 4 click navigated but target unreachable — counting as success", session.id);
                return Ok(true);
            }
            if is_err { go_back(session).await; }
        }

        // ================================================================
        // METHOD 5: JS window.location.href = ad URL
        // ================================================================
        let ad_href = focus_result.as_ref()
            .and_then(|v| v.get("href"))
            .and_then(|v| v.as_str())
            .unwrap_or(href);

        if !ad_href.is_empty() && session.is_alive() {
            warn!("Session {} METHOD 4 failed, trying METHOD 5: window.location.href", session.id);
            session.execute_js(&format!(
                "window.location.href = {};",
                serde_json::to_string(ad_href).unwrap_or_else(|_| format!("\"{}\"", ad_href))
            )).await.ok();

            let (nav, url_after, is_err, net_err) = poll_navigation(session, &url_before, 12).await;
            if nav && !is_err && !net_err {
                info!("Session {} METHOD 5 SUCCESS -> {}", session.id, safe_truncate(&url_after, 80));
                return Ok(true);
            }
            if net_err {
                info!("Session {} METHOD 5 click navigated but target unreachable — counting as success", session.id);
                return Ok(true);
            }
            if is_err { go_back(session).await; }
        }

        // ================================================================
        // METHOD 6: Create hidden anchor + .click() (bypasses event listeners)
        // ================================================================
        if !ad_href.is_empty() && session.is_alive() {
            warn!("Session {} METHOD 5 failed, trying METHOD 6: anchor clone click", session.id);
            let href_json = serde_json::to_string(ad_href).unwrap_or_else(|_| format!("\"{}\"", ad_href));
            session.execute_js(&format!(r#"
                (function() {{
                    var a = document.createElement('a');
                    a.href = {};
                    a.target = '_self';
                    a.style.position = 'fixed';
                    a.style.top = '-9999px';
                    a.textContent = 'nav';
                    document.body.appendChild(a);
                    a.click();
                    setTimeout(function() {{ a.remove(); }}, 500);
                }})()
            "#, href_json)).await.ok();

            let (nav, url_after, is_err, net_err) = poll_navigation(session, &url_before, 12).await;
            if nav && !is_err && !net_err {
                info!("Session {} METHOD 6 SUCCESS -> {}", session.id, safe_truncate(&url_after, 80));
                return Ok(true);
            }
            if net_err {
                info!("Session {} METHOD 6 click navigated but target unreachable — counting as success", session.id);
                return Ok(true);
            }
            if is_err { go_back(session).await; }
        }

        // ================================================================
        // METHOD 7: Direct navigation via session.navigate() (last resort)
        // ================================================================
        if !ad_href.is_empty() && session.is_alive() {
            warn!("Session {} METHOD 6 failed, trying METHOD 7: direct navigate()", session.id);
            let nav_result = session.navigate(ad_href).await;
            match nav_result {
                Ok(_) => {
                    let (nav, url_after, is_err, net_err) = poll_navigation(session, &url_before, 12).await;
                    if nav && !is_err && !net_err {
                        info!("Session {} METHOD 7 SUCCESS -> {}", session.id, safe_truncate(&url_after, 80));
                        return Ok(true);
                    }
                    if net_err {
                        info!("Session {} METHOD 7 click navigated but target unreachable — counting as success", session.id);
                        return Ok(true);
                    }
                    if is_err {
                        warn!("Session {} METHOD 7 error page: {}", session.id, safe_truncate(&url_after, 60));
                    }
                }
                Err(e) => {
                    warn!("Session {} METHOD 7 navigate() error: {}", session.id, e);
                }
            }
        }

        // All 7 methods failed
        warn!("Session {} ALL 7 click methods failed — ad not clicked", session.id);
        Ok(false)
    }

    /// Find and click on target domain CAMPAIGN AD in Google results.
    /// Scans the page for target ads (using "نتيجة إعلانية", "إعلان", sitelinks, #tads, etc.)
    /// Only clicks actual Google Ads — never organic results.
    /// After clicking, verifies navigation actually happened (CDP → JS click → direct nav).
    pub async fn click_target_ad(session: &Arc<BrowserSession>, targets: &[String]) -> Result<bool, BrowserError> {
        info!("Session {} looking for CAMPAIGN ADS for targets: {:?}", session.id, targets);

        // Brief pause to "read" results like a human
        Self::random_delay(800, 1200).await;

        // Move cursor into the results area (human brings hand to mouse/trackpad)
        {
            let (scan_x, scan_y) = {
                use rand::Rng;
                let mut rng = rand::thread_rng();
                (rng.gen_range(200.0..600.0_f64), rng.gen_range(150.0..350.0_f64))
            };
            session.move_mouse_human(scan_x, scan_y).await.ok();
            Self::random_delay(200, 400).await;
        }

        // ========== PHASE 1: Scan from top of page ==========
        let result = Self::find_and_click_target_ad(session, targets).await?;
        let debug_info = result.get("debug").cloned().unwrap_or_default();
        let ad_count = debug_info.get("adLinksFound").and_then(|v| v.as_u64()).unwrap_or(0);
        let organic_count = debug_info.get("organicLinksFound").and_then(|v| v.as_u64()).unwrap_or(0);
        let total_target = debug_info.get("totalTargetLinks").and_then(|v| v.as_u64()).unwrap_or(0);
        let redirect_count = debug_info.get("adRedirectLinks").and_then(|v| v.as_u64()).unwrap_or(0);
        let has_ad_label = debug_info.get("pageHasAdLabel").and_then(|v| v.as_bool()).unwrap_or(false);
        let ad_types = debug_info.get("adTypes").and_then(|v| v.as_str()).unwrap_or("");

        info!("Session {} scan: {} target links ({} ads [{}via redirect], {} organic), adLabel={}, types=[{}]",
            session.id, total_target, ad_count, redirect_count, organic_count, has_ad_label, safe_truncate(ad_types, 100));

        if Self::cdp_click_ad(session, &result, "initial scan", targets).await? {
            return Ok(true);
        }

        // ========== PHASE 2: Scroll down and re-scan ==========
        // Some ads only appear when scrolled into view (lazy loading)
        for step in 0..3 {
            let scroll_amount = 400 + rand::thread_rng().gen_range(0..300);
            session.execute_js(&format!(
                "window.scrollBy({{ top: {}, behavior: 'smooth' }})", scroll_amount
            )).await?;
            Self::random_delay(600, 900).await;

            let result = Self::find_and_click_target_ad(session, targets).await?;
            if Self::cdp_click_ad(session, &result, &format!("scroll {}", step + 1), targets).await? {
                return Ok(true);
            }
        }

        // ========== PHASE 3: Check bottom of page ==========
        session.execute_js(r#"
            window.scrollTo({ top: document.body.scrollHeight - window.innerHeight - 50, behavior: 'smooth' })
        "#).await?;
        Self::random_delay(700, 1000).await;

        let result = Self::find_and_click_target_ad(session, targets).await?;
        if Self::cdp_click_ad(session, &result, "bottom", targets).await? {
            return Ok(true);
        }

        // ========== PHASE 4: Back to top for final attempt ==========
        session.execute_js("window.scrollTo({ top: 0, behavior: 'smooth' })").await?;
        Self::random_delay(500, 800).await;

        let result = Self::find_and_click_target_ad(session, targets).await?;
        if Self::cdp_click_ad(session, &result, "final", targets).await? {
            return Ok(true);
        }

        warn!("Session {} NO CAMPAIGN ADS found for targets {:?} (ads={}, organic_skipped={}, total={})",
            session.id, targets, ad_count, organic_count, total_target);
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

        // Find next page button and get coordinates
        let next_info = session.execute_js(r#"
            (function() {
                const next = document.querySelector('#pnnext, a[aria-label="Next page"], a[id="pnnext"]');
                if (next) {
                    next.removeAttribute('target');
                    next.scrollIntoView({ behavior: 'smooth', block: 'center' });
                    const rect = next.getBoundingClientRect();
                    return {
                        found: true,
                        x: rect.left + rect.width / 2,
                        y: rect.top + rect.height / 2
                    };
                }
                return { found: false };
            })()
        "#).await?;

        if next_info.get("found").and_then(|v| v.as_bool()) == Some(true) {
            let nx = next_info.get("x").and_then(|v| v.as_f64()).unwrap_or(400.0);
            let ny = next_info.get("y").and_then(|v| v.as_f64()).unwrap_or(400.0);

            // CDP mouse movement + click (isTrusted: true)
            Self::random_delay(200, 400).await;
            session.move_mouse_human(nx, ny).await.ok();
            Self::random_delay(100, 200).await;
            session.click_human_at(nx, ny).await?;
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

    /// Browse the target page after clicking from Google
    /// Uses enhanced human-like reading behavior
    pub async fn browse_target_page(session: &Arc<BrowserSession>) -> Result<(), BrowserError> {
        debug!("Session {} browsing target page with human-like behavior", session.id);

        // Verify we're not on Google anymore (we've navigated to the ad target)
        let on_google = session.execute_js(r#"
            window.location.hostname.includes('google')
        "#).await?;

        if on_google.as_bool() == Some(true) {
            warn!("Session {} still on Google, not on target page", session.id);
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
            // Find a random link and get its coordinates (JS only finds, doesn't click)
            let link_info = session.execute_js(r#"
                (async function() {
                    const links = document.querySelectorAll('a[href*="/ads/"], a[href*="/listing/"], .card a, .item a');
                    if (links.length === 0) return { found: false };
                    const randomLink = links[Math.floor(Math.random() * links.length)];
                    randomLink.removeAttribute('target');
                    randomLink.scrollIntoView({ behavior: 'smooth', block: 'center' });
                    await new Promise(r => setTimeout(r, 400));
                    const rect = randomLink.getBoundingClientRect();
                    return {
                        found: true,
                        x: rect.left + rect.width / 2,
                        y: rect.top + rect.height / 2
                    };
                })()
            "#).await?;

            if link_info.get("found").and_then(|v| v.as_bool()) == Some(true) {
                let lx = link_info.get("x").and_then(|v| v.as_f64()).unwrap_or(400.0);
                let ly = link_info.get("y").and_then(|v| v.as_f64()).unwrap_or(300.0);

                // CDP mouse movement + click (isTrusted: true)
                session.move_mouse_human(lx, ly).await.ok();
                Self::random_delay(100, 200).await;
                session.click_human_at(lx, ly).await.ok();

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

    /// Wait for page to fully load by polling document.readyState.
    /// Returns true if page reached 'complete' or 'interactive', false if timed out.
    pub async fn wait_for_page_ready(session: &Arc<BrowserSession>, max_wait_ms: u64) -> bool {
        let polls = (max_wait_ms / 300).max(1);
        for attempt in 0..polls {
            tokio::time::sleep(Duration::from_millis(300)).await;
            let result = session.execute_js(r#"document.readyState"#).await;
            if let Ok(val) = result {
                let state = val.as_str().unwrap_or("loading");
                if state == "complete" || state == "interactive" {
                    debug!("Session {} page ready: {} (attempt {})", session.id, state, attempt + 1);
                    return true;
                }
            }
        }
        warn!("Session {} page readyState timeout after {}ms", session.id, max_wait_ms);
        false
    }

    /// Mouse simulation disabled - not effective for anti-detection
    pub async fn simulate_human_mouse(_session: &Arc<BrowserSession>) -> Result<(), BrowserError> {
        Ok(())
    }

    /// Simulate human-like reading of search results (3-8 seconds total).
    /// A real human beginner looks at the page, scrolls slowly through results,
    /// pauses on interesting items, sometimes scrolls back up.
    pub async fn simulate_reading(session: &Arc<BrowserSession>) -> Result<(), BrowserError> {
        // Initial gaze — eyes land on results, take a moment to focus
        Self::human_delay(1000, 800).await;

        // Scroll down through results like reading — 2-4 small scrolls
        session.execute_js(r#"
            (async function() {
                const scrollSteps = 2 + Math.floor(Math.random() * 3);
                for (let i = 0; i < scrollSteps; i++) {
                    // Scroll 100-250px per step (one result block height)
                    const amount = 100 + Math.floor(Math.random() * 150);
                    window.scrollBy({ top: amount, behavior: 'smooth' });
                    // Pause to "read" each result: 800-2000ms
                    await new Promise(r => setTimeout(r, 800 + Math.random() * 1200));
                }
                // 30% chance: scroll back up a bit (re-read something)
                if (Math.random() < 0.3) {
                    window.scrollBy({ top: -(50 + Math.floor(Math.random() * 100)), behavior: 'smooth' });
                    await new Promise(r => setTimeout(r, 500 + Math.random() * 700));
                }
            })()
        "#).await?;

        // Brief pause after reading before taking action
        Self::human_delay(500, 500).await;
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

                    // Mouse movements are handled via CDP (isTrusted: true) outside this scroll loop
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
    /// If stats provided, records click IMMEDIATELY when landing confirmed (not after dwell)
    pub async fn run_cycle(
        session: &Arc<BrowserSession>,
        keyword: &str,
        min_delay_ms: u64,
        max_delay_ms: u64,
        stats: Option<&Arc<crate::stats::GlobalStats>>,
        target_domains: &[String],
    ) -> Result<bool, BrowserError> {
        // Use default if empty
        let targets: Vec<String> = if target_domains.is_empty() {
            vec!["grintahub.com".to_string()]
        } else {
            target_domains.to_vec()
        };
        info!("Session {} starting Google search cycle for: {} (targets: {:?})", session.id, keyword, targets);

        // 1. Go to Google with human-like behavior
        Self::goto_google(session).await?;

        // 2. Search on Google with the keyword
        let has_results = Self::google_search(session, keyword).await?;

        if !has_results {
            warn!("Session {} no Google results found - fast IP change", session.id);
            return Err(BrowserError::ElementNotFound("No Google results - need new IP".into()));
        }

        // Human pause — search results just loaded, eyes adjust to the page
        Self::human_delay(800, 600).await;

        // Read through search results like a real user (3-8 seconds of scrolling)
        Self::simulate_reading(session).await?;

        // Decision pause — human decides which result to click
        Self::human_delay(600, 400).await;

        // 3. Try to find and click target domain SPONSORED AD - FIRST PAGE ONLY
        let clicked = Self::click_target_ad(session, &targets).await?;

        if clicked {
            // Wait for the redirect to complete and verify we landed on target domain
            info!("Session {} ad clicked - waiting for redirect to target domain...", session.id);

            // Build JS array of target domains for checking
            let targets_js = targets.iter()
                .map(|d| format!("'{}'", d.replace("'", "\\'")))
                .collect::<Vec<_>>()
                .join(",");

            // Wait for page load (check document.readyState and URL)
            // Track redirect chain for debugging
            let mut landed_on_target = false;
            let mut redirect_chain: Vec<String> = Vec::new();
            let mut last_error: Option<String> = None;

            for attempt in 0..10 {
                tokio::time::sleep(Duration::from_millis(500)).await;
                let check_js = format!(r#"
                    (function() {{
                        const targets = [{}];
                        const hostname = window.location.hostname.toLowerCase();
                        const url = window.location.href;
                        const onTarget = targets.some(t => hostname.includes(t.replace('.com', '').replace('.', '')));
                        // Detect error pages
                        const isError = url.startsWith('chrome-error://') ||
                                        url.startsWith('about:') ||
                                        url.startsWith('chrome://') ||
                                        document.title.toLowerCase().includes('error') ||
                                        document.body?.innerText?.includes('ERR_');
                        return {{
                            url: url,
                            ready: document.readyState,
                            onTarget: onTarget,
                            hostname: hostname,
                            isError: isError,
                            title: document.title || ''
                        }};
                    }})()
                "#, targets_js);
                let page_state = session.execute_js(&check_js).await;

                match page_state {
                    Ok(state) => {
                        let url = state.get("url").and_then(|v| v.as_str()).unwrap_or("");
                        let on_target = state.get("onTarget").and_then(|v| v.as_bool()).unwrap_or(false);
                        let ready = state.get("ready").and_then(|v| v.as_str()).unwrap_or("loading");
                        let hostname = state.get("hostname").and_then(|v| v.as_str()).unwrap_or("unknown");
                        let is_error = state.get("isError").and_then(|v| v.as_bool()).unwrap_or(false);
                        let title = state.get("title").and_then(|v| v.as_str()).unwrap_or("");

                        // Track redirect chain
                        if redirect_chain.last().map(|s| s.as_str()) != Some(url) && !url.is_empty() {
                            redirect_chain.push(url.to_string());
                        }

                        // Detect error pages immediately
                        if is_error {
                            last_error = Some(format!("Error page detected: {} (title: {})", url, title));
                            warn!("Session {} redirect landed on ERROR page: {} - aborting", session.id, safe_truncate(url, 80));
                            break;
                        }

                        if on_target {
                            landed_on_target = true;
                            session.increment_clicks();
                            if let Some(s) = stats {
                                s.record_click(0);
                            }
                            info!("Session {} AD CLICK SUCCESS - landed on {} (attempt {}) [CLICK COUNTED]", session.id, hostname, attempt + 1);
                            break;
                        }
                        debug!("Session {} page state: ready={}, onTarget={}, host={} (attempt {})", session.id, ready, on_target, hostname, attempt + 1);
                    }
                    Err(_) => {
                        // JS failed — page may be stuck loading through dead proxy.
                        // Use CDP-level URL check as fallback (works even during loading).
                        if let Ok(cdp_url) = session.get_current_url().await {
                            // Track in redirect chain
                            if redirect_chain.last().map(|s| s.as_str()) != Some(&cdp_url) && !cdp_url.is_empty() {
                                redirect_chain.push(cdp_url.clone());
                            }

                            // Check for error URLs
                            if cdp_url.starts_with("chrome-error://") || cdp_url.starts_with("about:") {
                                last_error = Some(format!("Browser error page: {}", cdp_url));
                                warn!("Session {} redirect failed with browser error: {}", session.id, safe_truncate(&cdp_url, 80));
                                break;
                            }

                            let cdp_host = cdp_url.split('/').nth(2).unwrap_or("").to_lowercase();
                            let on_target = targets.iter().any(|t| {
                                let simple = t.replace(".com", "").replace(".net", "").replace(".org", "");
                                cdp_host.contains(&simple)
                            });
                            if on_target {
                                landed_on_target = true;
                                session.increment_clicks();
                                if let Some(s) = stats {
                                    s.record_click(0);
                                }
                                info!("Session {} AD CLICK SUCCESS via CDP - URL: {} (attempt {}) [CLICK COUNTED]", session.id, safe_truncate(&cdp_url, 80), attempt + 1);
                                break;
                            }
                        }
                        debug!("Session {} waiting for navigation to settle (attempt {})", session.id, attempt + 1);
                    }
                }
            }

            // Log redirect chain for debugging
            if !redirect_chain.is_empty() {
                debug!("Session {} redirect chain: {}", session.id,
                    redirect_chain.iter().map(|u| safe_truncate(u, 50)).collect::<Vec<_>>().join(" -> "));
            }

            // Handle error pages and unconfirmed landings.
            // We're inside `if clicked { ... }` — click_target_ad() only returns true when:
            //   1. A verified Google Ad element was found and clicked
            //   2. Navigation was triggered (URL changed OR network error detected)
            // Google tracks clicks at googleadservices.com (first redirect hop), which always
            // succeeds even if the final target is unreachable through proxy. So if we got
            // here with an error page, the click IS already billed by Google.
            if let Some(ref error) = last_error {
                if !landed_on_target {
                    // Error page detected — target unreachable but Google already tracked the click
                    landed_on_target = true;
                    session.increment_clicks();
                    if let Some(s) = stats {
                        s.record_click(0);
                    }
                    info!("Session {} AD CLICK COUNTED — ad clicked, redirect started, but target unreachable: {} [CLICK COUNTED]",
                        session.id, error);
                }
            }

            // Stricter verification: only count clicks with confirmed landing or confirmed error
            if !landed_on_target {
                warn!("Session {} click happened but landing NOT confirmed - NOT counting (stricter verification)", session.id);
                return Ok(false);
            }

            // 4. Browse the target page (click was counted above when landing confirmed)
            let browse_start = std::time::Instant::now();
            if let Err(e) = Self::browse_target_page(session).await {
                warn!("Session {} browse error (click already counted): {}", session.id, e);
            }
            let browse_elapsed = browse_start.elapsed().as_millis() as u64;

            // 5. Extended dwell on landing page — anti-detection, simulates real user
            // Increased from 10-25s to 15-45s for more natural behavior
            let min_dwell_ms: u64 = 15_000;
            let max_dwell_ms: u64 = 45_000;
            let dwell_time = {
                let mut rng = rand::thread_rng();
                rng.gen_range(min_dwell_ms..=max_dwell_ms)
            };
            if browse_elapsed < dwell_time {
                let remaining = dwell_time - browse_elapsed;
                info!("Session {} dwell time: staying {}ms more on target (browsed {}ms, target {}ms)",
                    session.id, remaining, browse_elapsed, dwell_time);
                tokio::time::sleep(Duration::from_millis(remaining)).await;
            }

            // Random delay before next cycle
            Self::human_delay(min_delay_ms, max_delay_ms - min_delay_ms).await;

            info!("Session {} completed ad click cycle (dwell: {}ms)", session.id, dwell_time);
            Ok(true)
        } else {
            // NO AD FOUND on first page = FAST IP change, no delay
            info!("Session {} no SPONSORED AD found for targets {:?} - fast IP change", session.id, targets);
            Err(BrowserError::ElementNotFound("No sponsored ad found - need new IP".into()))
        }
    }

    /// Run a full cycle with optional Google account login
    /// Login happens once at the start of the session
    /// If stats provided, records click IMMEDIATELY when landing confirmed
    pub async fn run_cycle_with_login(
        session: &Arc<BrowserSession>,
        keyword: &str,
        min_delay_ms: u64,
        max_delay_ms: u64,
        account: Option<&GoogleAccount>,
        already_logged_in: &mut bool,
        stats: Option<&Arc<crate::stats::GlobalStats>>,
        target_domains: &[String],
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
        Self::run_cycle(session, keyword, min_delay_ms, max_delay_ms, stats, target_domains).await
    }

    // =================== TRUST-BUILDING FUNCTIONS ===================

    /// Lightweight warm-up sites (no heavy ad networks, low bandwidth)
    const WARMUP_SITES: &'static [&'static str] = &[
        "https://en.wikipedia.org",
        "https://www.timeanddate.com",
        "https://stackoverflow.com",
    ];

    /// Organic search queries (Saudi-relevant, natural mix)
    const ORGANIC_QUERIES: &'static [&'static str] = &[
        "weather riyadh",
        "prayer times riyadh",
        "usd to sar",
        "saudi arabia news",
        "best restaurants jeddah",
        "riyadh events",
        "football scores today",
        "kabsa recipe",
        "saudi vision 2030",
        "jeddah airport flights",
        "riyadh mall hours",
        "eid holidays saudi",
    ];

    /// Warm-up session before first Google search.
    /// Builds trust by visiting Google (gets NID/CONSENT cookies) and
    /// browsing 1-2 lightweight sites. Ad tracker domains are already
    /// blocked in block_unnecessary_resources().
    pub async fn warm_up_session(session: &Arc<BrowserSession>) -> Result<(), BrowserError> {
        info!("Session {} starting warm-up phase (build trust before searching)", session.id);

        // Phase 1: Visit Google first to get NID/CONSENT cookies
        session.navigate("https://www.google.com.sa/?hl=ar&gl=sa").await?;
        Self::human_delay(2500, 1500).await;

        // Handle consent dialog (reuses the same consent handling as goto_google)
        let consent_result = session.execute_js(r#"
            (async function() {
                await new Promise(r => setTimeout(r, 1000));
                const selectors = [
                    'button[id*="L2AGLb"]', 'button[id*="accept"]', 'button[id*="agree"]',
                    '[aria-label*="Accept"]', '[aria-label*="agree"]',
                    '[aria-label*="قبول"]', '[aria-label*="موافق"]',
                    '.tHlp8d', '#introAgreeButton', 'button.VfPpkd-LgbsSe',
                    'form[action*="consent"] button', 'button.VfPpkd-LgbsSe-OWXEXe-k8QpJ'
                ];
                for (const sel of selectors) {
                    try {
                        const btns = document.querySelectorAll(sel);
                        for (const btn of btns) {
                            if (btn.offsetParent !== null && btn.offsetWidth > 0) {
                                btn.click();
                                await new Promise(r => setTimeout(r, 500));
                                return 'clicked';
                            }
                        }
                    } catch(e) {}
                }
                return 'no_consent';
            })()
        "#).await;
        debug!("Session {} warm-up consent: {:?}", session.id, consent_result);
        Self::human_delay(800, 400).await;

        // Verify Google cookies
        let cookies = session.execute_js(r#"
            (function() {
                const c = document.cookie;
                return { hasNID: c.includes('NID'), hasCONSENT: c.includes('CONSENT'), len: c.length };
            })()
        "#).await;
        if let Ok(ref v) = cookies {
            debug!("Session {} Google cookies after warm-up: {:?}", session.id, v);
        }

        // Phase 2: Browse 1-2 lightweight sites (ad trackers already blocked)
        let mut rng = rand::rngs::StdRng::from_entropy();
        let warmup_count = rng.gen_range(1..=2);

        let mut indices: Vec<usize> = (0..Self::WARMUP_SITES.len()).collect();
        // Simple shuffle
        for i in (1..indices.len()).rev() {
            let j = rng.gen_range(0..=i);
            indices.swap(i, j);
        }

        for i in 0..warmup_count {
            let site = Self::WARMUP_SITES[indices[i]];
            info!("Session {} warm-up: visiting {} ({}/{})", session.id, site, i + 1, warmup_count);

            if let Err(e) = session.navigate(site).await {
                debug!("Session {} warm-up nav to {} failed (non-fatal): {}", session.id, site, e);
                continue;
            }

            // Brief "reading" behavior
            Self::human_delay(3000, 3000).await;
            let _ = session.scroll_human(200 + rng.gen_range(0..300) as i32).await;
            Self::human_delay(1000, 2000).await;
        }

        info!("Session {} warm-up complete — ready for Google search", session.id);
        Ok(())
    }

    /// Do an organic Google search (not for grintahub).
    /// Builds search history to make session look like a real user.
    /// 30% chance of clicking an organic result.
    pub async fn organic_search(session: &Arc<BrowserSession>) -> Result<(), BrowserError> {
        let mut rng = rand::rngs::StdRng::from_entropy();
        let query = Self::ORGANIC_QUERIES[rng.gen_range(0..Self::ORGANIC_QUERIES.len())];

        info!("Session {} organic search: \"{}\"", session.id, query);

        // Navigate to Google (consent already handled during warm-up)
        session.navigate("https://www.google.com.sa/?hl=ar&gl=sa").await?;
        Self::human_delay(1500, 1000).await;

        // Do the search using existing google_search flow
        match Self::google_search(session, query).await {
            Ok(_has_results) => {
                // Scroll through results like a real user
                Self::human_delay(1500, 1500).await;
                let _ = Self::human_scroll_read(session).await;
                Self::human_delay(1000, 2000).await;

                // 30% chance: click a random organic result (NOT grintahub, NOT ads)
                if rng.gen_bool(0.3) {
                    let result = session.execute_js(r#"
                        (function() {
                            const links = document.querySelectorAll('#search a[href]:not([href*="grintahub"]):not([href*="googleadservices"])');
                            const visible = [...links].filter(l => l.offsetParent !== null && l.getBoundingClientRect().top > 0);
                            if (visible.length === 0) return { found: false };
                            const idx = Math.floor(Math.random() * Math.min(5, visible.length));
                            const chosen = visible[idx];
                            chosen.removeAttribute('target');
                            chosen.scrollIntoView({ behavior: 'smooth', block: 'center' });
                            const rect = chosen.getBoundingClientRect();
                            return { found: true, x: rect.left + rect.width/2, y: rect.top + rect.height/2, text: chosen.textContent.substring(0, 50) };
                        })()
                    "#).await;

                    if let Ok(ref v) = result {
                        if v.get("found").and_then(|v| v.as_bool()) == Some(true) {
                            let x = v.get("x").and_then(|v| v.as_f64()).unwrap_or(400.0);
                            let y = v.get("y").and_then(|v| v.as_f64()).unwrap_or(300.0);
                            let text = v.get("text").and_then(|v| v.as_str()).unwrap_or("");
                            debug!("Session {} organic click: \"{}\" at ({}, {})", session.id, text, x, y);
                            let _ = session.click_human_at(x, y).await;
                            Self::human_delay(4000, 6000).await; // Browse the result briefly
                        }
                    }
                }
            }
            Err(BrowserError::CaptchaDetected(msg)) => {
                // CAPTCHA during organic search — don't propagate, just log and return
                warn!("Session {} organic search hit CAPTCHA: {}", session.id, msg);
                return Err(BrowserError::CaptchaDetected(msg));
            }
            Err(e) => {
                debug!("Session {} organic search failed (non-fatal): {}", session.id, e);
            }
        }

        info!("Session {} organic search complete", session.id);
        Ok(())
    }
}
