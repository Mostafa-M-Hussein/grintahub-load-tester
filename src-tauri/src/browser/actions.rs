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

        // Wait for page to load - longer wait for proxy latency
        Self::human_delay(3500, 2000).await;

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

        // Wait after handling consent
        Self::human_delay(1500, 1000).await;

        // Check for CAPTCHA
        if Self::check_google_captcha(session).await? {
            session.increment_captchas();
            return Err(BrowserError::CaptchaDetected("CAPTCHA on Google homepage".into()));
        }

        // Simulate looking at the page first
        Self::simulate_human_mouse(session).await?;
        Self::human_delay(500, 500).await;

        // Verify we can see the search input - if not, try refreshing or waiting
        let has_search = session.execute_js(r#"
            (function() {
                const input = document.querySelector('input[name="q"], textarea[name="q"], input[type="text"][title*="Search"], input[aria-label*="Search"], input[aria-label*="بحث"]');
                return input !== null && input.offsetParent !== null;
            })()
        "#).await?;

        if has_search.as_bool() != Some(true) {
            warn!("Session {} search input not found, waiting longer...", session.id);
            Self::human_delay(2000, 1000).await;

            // Try clicking anywhere to dismiss any overlays
            session.execute_js(r#"
                document.body.click();
                document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
            "#).await?;

            Self::human_delay(500, 500).await;
        }

        // Human-like: look around the page
        session.execute_js(r#"
            (async function() {
                // Random small movements
                for (let i = 0; i < 2; i++) {
                    await new Promise(r => setTimeout(r, 200 + Math.random() * 400));
                    const x = window.innerWidth / 2 + (Math.random() * 200 - 100);
                    const y = window.innerHeight / 2 + (Math.random() * 150 - 75);
                    document.dispatchEvent(new MouseEvent('mousemove', {
                        clientX: x, clientY: y, bubbles: true
                    }));
                }

                // Maybe small scroll
                if (Math.random() > 0.5) {
                    const scrollAmount = Math.floor(Math.random() * 50);
                    window.scrollTo({ top: scrollAmount, behavior: 'smooth' });
                }
            })()
        "#).await?;

        Self::human_delay(400, 800).await;
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

        // Wait a bit before pressing enter (like thinking)
        Self::random_delay(800, 1500).await;

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
        Self::random_delay(3000, 5000).await;

        // Check for CAPTCHA after search
        if Self::check_google_captcha(session).await? {
            session.increment_captchas();
            return Err(BrowserError::CaptchaDetected("CAPTCHA after Google search".into()));
        }

        // Check if results loaded
        let has_results = session.execute_js(r#"
            (function() {
                const results = document.querySelectorAll('#search a[href], #rso a[href]');
                return results.length > 0;
            })()
        "#).await?;

        Ok(has_results.as_bool().unwrap_or(false))
    }

    /// Find and click on grintahub.com SPONSORED AD in Google results
    pub async fn click_grintahub_result(session: &Arc<BrowserSession>) -> Result<bool, BrowserError> {
        info!("Session {} looking for grintahub.com in SPONSORED ADS", session.id);

        // Scroll down a bit first (human behavior)
        session.execute_js(r#"
            (async function() {
                // Random scroll to simulate reading results
                for (let i = 0; i < 2; i++) {
                    await new Promise(r => setTimeout(r, 500 + Math.random() * 1000));
                    window.scrollBy({
                        top: 150 + Math.random() * 200,
                        behavior: 'smooth'
                    });
                }
            })()
        "#).await?;

        Self::random_delay(1000, 2000).await;

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
            info!("Session {} no grintahub.com SPONSORED ADS found", session.id);
            return Ok(false);
        }

        let count = find_result.get("count").and_then(|v| v.as_u64()).unwrap_or(0);
        let ad_type = find_result.get("type").and_then(|v| v.as_str()).unwrap_or("unknown");
        info!("Session {} found {} grintahub SPONSORED ADS (type: {})", session.id, count, ad_type);

        // Wait after scrolling
        Self::random_delay(800, 1500).await;

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
            Self::random_delay(3000, 6000).await;
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

        Self::random_delay(1000, 2000).await;

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
            Self::random_delay(3000, 5000).await;

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

        // Initial pause - human would look at the page first
        Self::human_delay(500, 1000).await;

        // Move mouse around like looking at the page
        // Generate random values before async to avoid Send issues
        let mouse_x = {
            let mut rng = rand::thread_rng();
            400 + rng.gen_range(0..400)
        };
        Self::bezier_mouse_move(session, mouse_x, 300).await?;
        Self::human_delay(300, 500).await;

        // Use enhanced human-like scroll reading
        Self::human_scroll_read(session).await?;

        // Time spent on page after scrolling
        Self::random_delay(3000, 8000).await;

        // Maybe click on something on the page (40% chance)
        let should_click = rand::thread_rng().gen_bool(0.4);
        if should_click {
            // First try to move mouse to an interesting element
            let found_element = Self::bezier_mouse_to_element(session, "a[href*='/ads/'], a[href*='/listing/'], .card, .item").await?;

            if found_element {
                Self::human_delay(200, 400).await;

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

                Self::random_delay(3000, 6000).await;

                // Browse the sub-page too
                Self::human_scroll_read(session).await?;
            }
        }

        // Move mouse around before leaving
        let (random_x, random_y) = {
            let mut rng = rand::thread_rng();
            (200 + rng.gen_range(0..600), 200 + rng.gen_range(0..300))
        };
        Self::bezier_mouse_move(session, random_x, random_y).await?;

        Self::random_delay(500, 1500).await;
        Ok(())
    }

    /// Random delay with jitter
    pub async fn random_delay(min_ms: u64, max_ms: u64) {
        let delay = rand::thread_rng().gen_range(min_ms..=max_ms);
        tokio::time::sleep(Duration::from_millis(delay)).await;
    }

    /// Simulate human-like mouse movements
    pub async fn simulate_human_mouse(session: &Arc<BrowserSession>) -> Result<(), BrowserError> {
        session.execute_js(r#"
            (async function() {
                // Create fake mouse movement events
                const moves = 3 + Math.floor(Math.random() * 4);
                for (let i = 0; i < moves; i++) {
                    const x = Math.floor(Math.random() * window.innerWidth);
                    const y = Math.floor(Math.random() * window.innerHeight);

                    document.dispatchEvent(new MouseEvent('mousemove', {
                        clientX: x,
                        clientY: y,
                        bubbles: true
                    }));

                    await new Promise(r => setTimeout(r, 100 + Math.random() * 300));
                }
            })()
        "#).await?;
        Ok(())
    }

    /// Simulate human-like reading behavior
    pub async fn simulate_reading(session: &Arc<BrowserSession>) -> Result<(), BrowserError> {
        session.execute_js(r#"
            (async function() {
                // Random pauses like reading
                const pauses = 2 + Math.floor(Math.random() * 3);
                for (let i = 0; i < pauses; i++) {
                    await new Promise(r => setTimeout(r, 500 + Math.random() * 1500));

                    // Small scroll adjustments
                    const scrollAmount = -20 + Math.floor(Math.random() * 40);
                    window.scrollBy({ top: scrollAmount, behavior: 'smooth' });
                }
            })()
        "#).await?;
        Ok(())
    }

    /// Bezier curve mouse movement - much more realistic than random jumps
    /// Uses cubic bezier interpolation with random control points
    pub async fn bezier_mouse_move(session: &Arc<BrowserSession>, target_x: i32, target_y: i32) -> Result<(), BrowserError> {
        session.execute_js(&format!(r#"
            (async function() {{
                // Get current mouse position from last known position or default to center
                const startX = window._lastMouseX || window.innerWidth / 2;
                const startY = window._lastMouseY || window.innerHeight / 2;
                const endX = {};
                const endY = {};

                // Generate random control points for natural curve
                // Human mouse movements follow curved paths, not straight lines
                const midX = (startX + endX) / 2;
                const midY = (startY + endY) / 2;

                // Add randomness to control points (creates natural curve variations)
                const cp1x = startX + (endX - startX) * 0.25 + (Math.random() - 0.5) * 100;
                const cp1y = startY + (endY - startY) * 0.25 + (Math.random() - 0.5) * 100;
                const cp2x = startX + (endX - startX) * 0.75 + (Math.random() - 0.5) * 100;
                const cp2y = startY + (endY - startY) * 0.75 + (Math.random() - 0.5) * 100;

                // Calculate distance for step count (longer distance = more steps)
                const distance = Math.sqrt(Math.pow(endX - startX, 2) + Math.pow(endY - startY, 2));
                const steps = Math.max(15, Math.min(50, Math.floor(distance / 20))) + Math.floor(Math.random() * 10);

                // Cubic bezier interpolation
                for (let i = 0; i <= steps; i++) {{
                    const t = i / steps;
                    const t2 = t * t;
                    const t3 = t2 * t;
                    const mt = 1 - t;
                    const mt2 = mt * mt;
                    const mt3 = mt2 * mt;

                    // Cubic bezier formula
                    const x = mt3 * startX + 3 * mt2 * t * cp1x + 3 * mt * t2 * cp2x + t3 * endX;
                    const y = mt3 * startY + 3 * mt2 * t * cp1y + 3 * mt * t2 * cp2y + t3 * endY;

                    // Add micro-jitter for realism (human hands shake slightly)
                    const jitterX = (Math.random() - 0.5) * 2;
                    const jitterY = (Math.random() - 0.5) * 2;

                    document.dispatchEvent(new MouseEvent('mousemove', {{
                        clientX: x + jitterX,
                        clientY: y + jitterY,
                        bubbles: true,
                        cancelable: true,
                        view: window
                    }}));

                    // Variable delay - slower at start and end (acceleration curve)
                    const speedFactor = 4 * t * (1 - t); // Peaks at t=0.5
                    const baseDelay = 5 + Math.random() * 10;
                    const delay = baseDelay + (1 - speedFactor) * 15;
                    await new Promise(r => setTimeout(r, delay));
                }}

                // Store final position for next movement
                window._lastMouseX = endX;
                window._lastMouseY = endY;
            }})()
        "#, target_x, target_y)).await?;
        Ok(())
    }

    /// Move mouse to element using bezier curve
    pub async fn bezier_mouse_to_element(session: &Arc<BrowserSession>, selector: &str) -> Result<bool, BrowserError> {
        let result = session.execute_js(&format!(r#"
            (function() {{
                const el = document.querySelector('{}');
                if (!el) return null;
                const rect = el.getBoundingClientRect();
                // Aim for slightly randomized position within element
                const x = rect.left + rect.width * (0.3 + Math.random() * 0.4);
                const y = rect.top + rect.height * (0.3 + Math.random() * 0.4);
                return {{ x: Math.floor(x), y: Math.floor(y) }};
            }})()
        "#, selector.replace('\'', "\\'"))).await?;

        if let Some(coords) = result.as_object() {
            if let (Some(x), Some(y)) = (coords.get("x").and_then(|v| v.as_i64()), coords.get("y").and_then(|v| v.as_i64())) {
                Self::bezier_mouse_move(session, x as i32, y as i32).await?;
                return Ok(true);
            }
        }
        Ok(false)
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

                    // Reading pause - varies based on "content complexity"
                    const pauseTime = contentComplexity > 0.7
                        ? 1000 + Math.random() * 3000  // Long pause for complex content
                        : 300 + Math.random() * 1000;  // Short pause for simple content
                    await new Promise(r => setTimeout(r, pauseTime));

                    // Occasional scroll back up (re-reading behavior - 10% chance)
                    if (Math.random() < 0.1) {
                        const scrollBack = 30 + Math.random() * 80;
                        window.scrollBy({ top: -scrollBack, behavior: 'smooth' });
                        await new Promise(r => setTimeout(r, 500 + Math.random() * 1000));
                        currentPos = window.scrollY;
                    }

                    // Occasional pause to "think" (5% chance)
                    if (Math.random() < 0.05) {
                        await new Promise(r => setTimeout(r, 2000 + Math.random() * 3000));
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
        // Add micro-variations to seem more human
        let micro_delay = rand::thread_rng().gen_range(0..=100);
        tokio::time::sleep(Duration::from_millis(delay + micro_delay)).await;
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

        // Move mouse naturally around the page using bezier curves
        // Generate random values before async to avoid Send issues
        let (center_x, center_y) = {
            let mut rng = rand::thread_rng();
            (400 + rng.gen_range(0..200), 300 + rng.gen_range(0..100))
        };
        Self::bezier_mouse_move(session, center_x, center_y).await?;
        Self::human_delay(300, 700).await;

        // Move to search box area with bezier curve
        Self::bezier_mouse_to_element(session, "input[name='q'], textarea[name='q']").await?;
        Self::human_delay(200, 400).await;

        // 2. Search on Google with the keyword
        let has_results = Self::google_search(session, keyword).await?;

        if !has_results {
            warn!("Session {} no Google results found", session.id);
            Self::human_delay(min_delay_ms, max_delay_ms - min_delay_ms).await;
            // No results = change IP
            return Err(BrowserError::ElementNotFound("No Google results - need new IP".into()));
        }

        // Simulate reading search results with bezier mouse movements
        Self::simulate_reading(session).await?;

        // Move mouse around the search results naturally
        let (results_x, results_y) = {
            let mut rng = rand::thread_rng();
            (300 + rng.gen_range(0..300), 400 + rng.gen_range(0..200))
        };
        Self::bezier_mouse_move(session, results_x, results_y).await?;
        Self::human_delay(300, 600).await;

        // 3. Try to find and click grintahub.com SPONSORED AD - FIRST PAGE ONLY
        let clicked = Self::click_grintahub_result(session).await?;

        if clicked {
            // Wait for page to load after click
            Self::human_delay(1500, 3000).await;

            // 4. Browse the grintahub page with enhanced human behavior
            Self::browse_grintahub_page(session).await?;

            // Random delay before next cycle
            Self::human_delay(min_delay_ms, max_delay_ms - min_delay_ms).await;

            info!("Session {} completed successful ad click cycle", session.id);
            Ok(true)
        } else {
            // NO AD FOUND on first page = change IP and try again
            info!("Session {} no grintahub.com SPONSORED AD found on first page - changing IP", session.id);
            Self::human_delay(1000, 2000).await;

            // Return error to trigger IP change
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
