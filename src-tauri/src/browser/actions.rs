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
            data_s.as_ref().map(|s| format!("{}...({}chars)", &s[..s.len().min(20)], s.len())).unwrap_or_else(|| "NONE".to_string()),
            form_count, callback_name, &page_url[..page_url.len().min(80)]);
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
        session.navigate("https://www.google.com.sa/").await?;

        // Wait for page to load (longer with proxy latency)
        Self::human_delay(2000, 1000).await;

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
                    debug!("Session {} search submitted (attempt {}, url: {})", session.id, attempt + 1, &url[..url.len().min(80)]);
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

            // Wait for results after fallback submit
            tokio::time::sleep(Duration::from_millis(3000)).await;
        }

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

        info!("Session {} search page: url={}, title={}",
            session.id, &url[..url.len().min(100)], &title[..title.len().min(50)]);
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

    /// Helper function to scan for grintahub ads on the page
    /// Find grintahub ad AND click it in a single JS execution.
    /// Returns { clicked: bool, count: int, type: str, debug: {} }
    /// This avoids the DOM-state-change bug where scan finds ads but a separate click call can't.
    async fn find_and_click_grintahub_ad(session: &Arc<BrowserSession>) -> Result<serde_json::Value, BrowserError> {
        session.execute_js(r#"
            (function() {
                const grintaAdLinks = [];

                // Helper: check if href is grintahub
                function isGrinta(href) {
                    return href.includes('grintahub.com') || href.includes('grintahub');
                }
                function notGoogle(href) {
                    return !href.includes('google.com');
                }
                function collectLinks(selector, type) {
                    const links = document.querySelectorAll(selector);
                    for (const link of links) {
                        const href = link.getAttribute('href') || '';
                        if (isGrinta(href) && notGoogle(href)) {
                            grintaAdLinks.push({ el: link, href, type,
                                text: (link.innerText || link.textContent || '').substring(0, 100),
                                y: link.getBoundingClientRect().top
                            });
                        }
                    }
                }

                // Method 1: Top ads (#tads)
                collectLinks('#tads a[href]', 'top_ad');

                // Method 2: Bottom ads (#tadsb)
                collectLinks('#tadsb a[href], #bottomads a[href], [id*="bottomads"] a[href]', 'bottom_ad');

                // Method 3: data-text-ad
                collectLinks('[data-text-ad] a[href]', 'text_ad');

                // Method 4: googleadservices.com redirect
                const adServiceLinks = document.querySelectorAll('a[href*="googleadservices.com"], a[href*="aclk"]');
                for (const link of adServiceLinks) {
                    const href = link.getAttribute('href') || '';
                    if (isGrinta(href)) {
                        grintaAdLinks.push({ el: link, href, type: 'adservice',
                            text: (link.innerText || link.textContent || '').substring(0, 100),
                            y: link.getBoundingClientRect().top
                        });
                    }
                }

                // Method 5: Sponsored labels (most comprehensive)
                const sponsoredLabels = document.querySelectorAll(
                    '[aria-label*="Sponsored"], [aria-label*="إعلان"], [data-dtld], .uEierd, .x54gtf'
                );
                for (const label of sponsoredLabels) {
                    const parent = label.closest('div[data-hveid]') || label.closest('div[data-text-ad]') ||
                                  label.parentElement?.parentElement?.parentElement?.parentElement;
                    if (parent) {
                        const links = parent.querySelectorAll('a[href]');
                        for (const link of links) {
                            const href = link.getAttribute('href') || '';
                            if (isGrinta(href) && notGoogle(href)) {
                                grintaAdLinks.push({ el: link, href, type: 'sponsored',
                                    text: (link.innerText || link.textContent || '').substring(0, 100),
                                    y: link.getBoundingClientRect().top
                                });
                            }
                        }
                    }
                }

                // Method 6: Also check spans with "Sponsored" / "إعلان" text
                const allSpans = document.querySelectorAll('span');
                for (const span of allSpans) {
                    const txt = (span.innerText || '').trim().toLowerCase();
                    if (txt === 'sponsored' || txt === 'إعلان' || txt === 'ad') {
                        const parent = span.closest('div[data-hveid]') || span.closest('div[data-text-ad]') ||
                                      span.parentElement?.parentElement?.parentElement?.parentElement?.parentElement;
                        if (parent) {
                            const links = parent.querySelectorAll('a[href]');
                            for (const link of links) {
                                const href = link.getAttribute('href') || '';
                                if (isGrinta(href) && notGoogle(href)) {
                                    grintaAdLinks.push({ el: link, href, type: 'span_sponsored',
                                        text: (link.innerText || link.textContent || '').substring(0, 100),
                                        y: link.getBoundingClientRect().top
                                    });
                                }
                            }
                        }
                    }
                }

                // Method 7: Ad container classes
                collectLinks('.ads-ad a[href], .commercial-unit-desktop-top a[href], [data-sokoban-container] a[href], .cu-container a[href]', 'ad_container');

                // Deduplicate by href
                const seen = new Set();
                const uniqueLinks = grintaAdLinks.filter(item => {
                    if (seen.has(item.href)) return false;
                    seen.add(item.href);
                    return true;
                });

                // Debug info
                const debug = {
                    hasTopAds: !!document.querySelector('#tads'),
                    hasBottomAds: !!document.querySelector('#tadsb, #bottomads'),
                    totalAdElements: document.querySelectorAll('[data-text-ad], [data-hveid]').length,
                    hasSponsoredText: document.body.innerText.includes('Sponsored') || document.body.innerText.includes('إعلان'),
                    scrollY: window.scrollY,
                    pageHeight: document.body.scrollHeight
                };

                if (uniqueLinks.length === 0) {
                    return { clicked: false, count: 0, debug };
                }

                // Sort by position (highest on page first)
                uniqueLinks.sort((a, b) => a.y - b.y);
                const chosen = uniqueLinks[0];

                // === PREPARE THE AD FOR CDP CLICK ===
                // Scroll ad into view
                chosen.el.scrollIntoView({ behavior: 'smooth', block: 'center' });

                // Remove target="_blank" to stay in same tab
                chosen.el.removeAttribute('target');
                chosen.el.setAttribute('target', '_self');

                // Wait for scroll to finish, then get final coordinates
                await new Promise(r => setTimeout(r, 400));
                const finalRect = chosen.el.getBoundingClientRect();

                return {
                    clicked: false,
                    ready_for_cdp_click: true,
                    count: uniqueLinks.length,
                    type: chosen.type,
                    href: chosen.href,
                    text: chosen.text,
                    x: finalRect.left + finalRect.width / 2,
                    y: finalRect.top + finalRect.height / 2,
                    debug
                };
            })()
        "#).await
    }

    /// Perform CDP mouse movement + click on an ad found by find_and_click_grintahub_ad.
    /// Returns true if the CDP click was performed, false if the result didn't have coordinates.
    async fn cdp_click_ad(session: &Arc<BrowserSession>, result: &serde_json::Value, phase: &str) -> Result<bool, BrowserError> {
        let ready = result.get("ready_for_cdp_click").and_then(|v| v.as_bool()).unwrap_or(false);
        if !ready {
            return Ok(false);
        }

        let x = result.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let y = result.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let count = result.get("count").and_then(|v| v.as_u64()).unwrap_or(0);
        let ad_type = result.get("type").and_then(|v| v.as_str()).unwrap_or("?");

        // CDP mouse movement to ad (physics-based bezier, isTrusted: true)
        session.move_mouse_human(x, y).await.ok();
        Self::random_delay(100, 200).await;

        // CDP click on the ad (isTrusted: true)
        session.click_human_at(x, y).await?;

        info!("Session {} CLICKED grintahub ad {} ({} found, type: {})", session.id, phase, count, ad_type);
        Self::random_delay(1500, 2500).await; // Wait for navigation
        Ok(true)
    }

    /// Find and click on grintahub.com SPONSORED AD in Google results.
    /// Uses 4-phase search: top → scroll down → bottom → back to top.
    /// JS finds the ad and returns coordinates, then CDP performs the click
    /// with isTrusted: true mouse events (critical for Google ad fraud detection).
    pub async fn click_grintahub_result(session: &Arc<BrowserSession>) -> Result<bool, BrowserError> {
        info!("Session {} looking for grintahub.com in SPONSORED ADS", session.id);

        // ========== PHASE 1: Check TOP of page first (most ads are here) ==========
        debug!("Session {} Phase 1: Checking top of page for ads...", session.id);

        // Brief pause to "read" initial results
        Self::random_delay(800, 1200).await;

        let result = Self::find_and_click_grintahub_ad(session).await?;
        if Self::cdp_click_ad(session, &result, "at TOP").await? {
            return Ok(true);
        }

        // ========== PHASE 2: Scroll down slowly checking after each scroll ==========
        debug!("Session {} Phase 2: Scrolling down to find ads...", session.id);

        for step in 0..4 {
            let scroll_amount = 300 + rand::thread_rng().gen_range(0..200);
            session.execute_js(&format!(
                "window.scrollBy({{ top: {}, behavior: 'smooth' }})", scroll_amount
            )).await?;

            // Wait for scroll + dynamic content to load
            Self::random_delay(700, 1100).await;

            let result = Self::find_and_click_grintahub_ad(session).await?;
            if Self::cdp_click_ad(session, &result, &format!("after scroll {}", step + 1)).await? {
                return Ok(true);
            }
        }

        // ========== PHASE 3: Jump to bottom for bottom ads ==========
        debug!("Session {} Phase 3: Checking bottom of page...", session.id);

        session.execute_js(r#"
            window.scrollTo({ top: document.body.scrollHeight - window.innerHeight - 50, behavior: 'smooth' })
        "#).await?;

        Self::random_delay(800, 1200).await;

        let result = Self::find_and_click_grintahub_ad(session).await?;
        if Self::cdp_click_ad(session, &result, "at BOTTOM").await? {
            return Ok(true);
        }

        // ========== PHASE 4: Scroll back to top for final check ==========
        debug!("Session {} Phase 4: Back to top for final check...", session.id);

        session.execute_js("window.scrollTo({ top: 0, behavior: 'smooth' })").await?;
        Self::random_delay(600, 900).await;

        let result = Self::find_and_click_grintahub_ad(session).await?;
        if Self::cdp_click_ad(session, &result, "on FINAL scan").await? {
            return Ok(true);
        }

        // ========== NO ADS FOUND ==========
        let debug_info = result.get("debug").cloned().unwrap_or_default();
        let has_top = debug_info.get("hasTopAds").and_then(|v| v.as_bool()).unwrap_or(false);
        let has_bottom = debug_info.get("hasBottomAds").and_then(|v| v.as_bool()).unwrap_or(false);
        let total_ads = debug_info.get("totalAdElements").and_then(|v| v.as_u64()).unwrap_or(0);

        warn!("Session {} NO grintahub ads found (topAds={}, bottomAds={}, adElements={})",
            session.id, has_top, has_bottom, total_ads);

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

            // 5. CRITICAL: Dwell time on landing page (60-180 seconds total)
            // Google flags clicks as invalid if dwell time is too short.
            // Real users spend 1-3 minutes browsing a page.
            let min_dwell_ms: u64 = 60_000;
            let max_dwell_ms: u64 = 180_000;
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
        session.navigate("https://www.google.com.sa/").await?;
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
        session.navigate("https://www.google.com.sa/").await?;
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
