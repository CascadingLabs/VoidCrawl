//! Anti-detection / stealth configuration for browser sessions.

use serde::{Deserialize, Serialize};

/// Configuration for browser stealth features that help avoid bot detection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StealthConfig {
    /// User-Agent string override. `None` = use browser default.
    pub user_agent:          Option<String>,
    /// Viewport width in pixels.
    pub viewport_width:      u32,
    /// Viewport height in pixels.
    pub viewport_height:     u32,
    /// Accept-Language header value.
    pub locale:              String,
    /// JavaScript snippet injected via `Page.addScriptToEvaluateOnNewDocument`
    /// *before* every page load. Runs in the page's main world.
    pub inject_js:           Option<String>,
    /// Whether to use chromiumoxide's built-in `enable_stealth_mode`.
    pub use_builtin_stealth: bool,
    /// Whether to bypass Content-Security-Policy for JS injection.
    pub bypass_csp:          bool,
}

impl Default for StealthConfig {
    fn default() -> Self {
        Self::chrome_like()
    }
}

impl StealthConfig {
    /// Preset that mimics a real desktop Chrome session.
    ///
    /// This follows the zendriver / nodriver philosophy: rely on clean
    /// Chrome launch flags rather than heavy JS injection.  We do NOT
    /// override the user-agent (avoids version mismatches with the real
    /// browser) and we do NOT use chromiumoxide's built-in stealth mode
    /// (it fires multiple `addScriptToEvaluateOnNewDocument` CDP calls
    /// that sophisticated WAFs can fingerprint).
    ///
    /// No JS is injected at all: `navigator.webdriver` comes out `false` from
    /// the launch flag, and the old force-open-shadow-DOM patch is gone (it
    /// broke Cloudflare Turnstile's closed-shadow tamper check). UA / platform
    /// / Client-Hints consistency is applied via CDP `setUserAgentOverride`
    /// (see `Page::apply_stealth`), not page-world JS.
    pub fn chrome_like() -> Self {
        Self {
            // None = keep the browser's real UA, preventing version
            // mismatches between the UA string and the actual Chrome build.
            user_agent:          None,
            viewport_width:      1920,
            viewport_height:     1080,
            locale:              "en-US,en;q=0.9".into(),
            // No JS injection. We previously force-opened all shadow DOMs to
            // "reach Turnstile iframes" — but that tripped Turnstile's
            // closed-shadow-root tamper check (ERROR 600010) and FAILED the
            // challenge. Interacting with a challenge widget works via real
            // compositor clicks at pixel coordinates regardless of shadow
            // mode, so the patch was unnecessary and harmful. Dropping it also
            // removes the `addScriptToEvaluateOnNewDocument` fingerprint
            // entirely. (Verified: managed Turnstile auto-passes,
            // siteverify success=true, interactive=false.)
            inject_js:           None,
            // Disabled: chromiumoxide's stealth sends detectable CDP patterns.
            use_builtin_stealth: false,
            // false: `Page.setBypassCSP` is itself a bot signal (rebrowser
            // `bypassCsp` test flags it as "invalid behavior for a normal
            // browser"). Our `addScriptToEvaluateOnNewDocument` injection runs
            // via CDP and is not subject to page CSP anyway, so we don't need
            // it. Flip to true only if a specific site's CSP blocks injection.
            bypass_csp:          false,
        }
    }

    /// Minimal config — no overrides, no injection, just headless defaults.
    pub fn none() -> Self {
        Self {
            user_agent:          None,
            viewport_width:      1920,
            viewport_height:     1080,
            locale:              "en-US,en;q=0.9".into(),
            inject_js:           None,
            use_builtin_stealth: false,
            bypass_csp:          false,
        }
    }
}
