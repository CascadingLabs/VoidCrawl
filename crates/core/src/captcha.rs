//! DOM-only captcha / bot-wall detection.
//!
//! Runs a single JS snippet against the page and reports the first
//! matching marker. Heuristics are intentionally conservative —
//! false negatives are preferable to false positives that would poison
//! retry logic upstream. See `docs/captcha-detection.md` for the full
//! list and known gaps.

use serde_json::Value;

use crate::{error::Result, page::Page};

/// Identified captcha / challenge surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptchaKind {
    Recaptcha,
    Hcaptcha,
    Turnstile,
    CloudflareChallenge,
    DatadomeBlock,
    /// Unknown but strong captcha signal (e.g. page title "Just a moment…").
    Unknown(String),
}

impl CaptchaKind {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Recaptcha => "recaptcha",
            Self::Hcaptcha => "hcaptcha",
            Self::Turnstile => "turnstile",
            Self::CloudflareChallenge => "cloudflare_challenge",
            Self::DatadomeBlock => "datadome",
            Self::Unknown(s) => s,
        }
    }
}

/// Single-pass DOM probe. Returns `Some(kind)` if any marker matched.
pub async fn detect_captcha(page: &Page) -> Result<Option<CaptchaKind>> {
    // One evaluate_js call, short-circuits on first match. Returns a
    // string tag we map to CaptchaKind, or null.
    const JS: &str = r#"
        (function() {
            try {
                // reCAPTCHA (v2/v3): iframe src contains google.com/recaptcha
                if (document.querySelector('iframe[src*="google.com/recaptcha"], iframe[src*="recaptcha/api2"]')) return 'recaptcha';
                if (document.querySelector('.g-recaptcha, #g-recaptcha')) return 'recaptcha';
                // hCaptcha
                if (document.querySelector('iframe[src*="hcaptcha.com"], .h-captcha, [data-hcaptcha-widget-id]')) return 'hcaptcha';
                // Cloudflare Turnstile
                if (document.querySelector('iframe[src*="challenges.cloudflare.com/turnstile"], .cf-turnstile')) return 'turnstile';
                // Cloudflare interstitial
                if (document.querySelector('#cf-challenge-running, #challenge-running, #cf-chl-widget, .cf-browser-verification')) return 'cloudflare_challenge';
                var t = (document.title || '').toLowerCase();
                if (t.includes('just a moment') || t.includes('attention required')) return 'cloudflare_challenge';
                // DataDome
                if (document.querySelector('#datadome-captcha, [id^="dd_"]')) return 'datadome';
                // PerimeterX / Akamai bot-manager pages often use these markers
                if (document.querySelector('#px-captcha')) return 'unknown:perimeterx';
                return null;
            } catch (e) { return null; }
        })()
    "#;

    let val = page.evaluate_js(JS).await?;
    Ok(match val {
        Value::String(s) => Some(match s.as_str() {
            "recaptcha" => CaptchaKind::Recaptcha,
            "hcaptcha" => CaptchaKind::Hcaptcha,
            "turnstile" => CaptchaKind::Turnstile,
            "cloudflare_challenge" => CaptchaKind::CloudflareChallenge,
            "datadome" => CaptchaKind::DatadomeBlock,
            other => {
                CaptchaKind::Unknown(other.strip_prefix("unknown:").unwrap_or(other).to_string())
            }
        }),
        _ => None,
    })
}
