//! Signature-based anti-bot / CDN vendor fingerprinting of an HTTP response.
//!
//! When a fetch hits a wall we want to know *which* vendor is gating the page
//! and whether it is **actively challenging** us (a block) versus merely
//! **present** (a CDN fronting a site that served us fine). That distinction
//! drives deterministic routing — Cloudflare Turnstile → headful + warm
//! profile, DataDome block → rotate residential proxy, plain Akamai presence →
//! no action — instead of today's blind retry.
//!
//! This is the **inbound** half of the problem (which wall is in front of us),
//! deliberately separate from the **outbound** half (do *we* look like a bot —
//! see [`crate::stealth`]). It stays generic: vendor fingerprints against the
//! response, never per-site adapters.
//!
//! ## How it works
//!
//! Each vendor is one entry in [`CORPUS_JSON`] with a `signals[]` array of
//! regexes and an optional `challenge[]` subset that indicates active blocking.
//! The response is normalized into a single newline-joined string — the status
//! line prefixed `S:`, each lowercased header prefixed `H:`, and the body
//! prefixed `B:` — and signals match against that form. All signals compile
//! into one [`RegexSet`] (cached in a [`OnceLock`], no build step — mirroring
//! [`crate::scanner`]'s embedded ruleset) so detection is a single regex pass,
//! cheap enough to run on every fetch.
//!
//! ## Header-first tiering
//!
//! The highest-signal tells live in headers (`server: cloudflare`,
//! `x-datadome`, `set-cookie: datadome=`), so [`classify`] runs the status +
//! header lines first and only normalizes a **bounded body prefix** as a
//! fallback for vendors that cloak behind a 200 with no telling header. The
//! returned [`Evidence`] records which tier produced the verdict — both a
//! confidence hint and a cost signal for callers.
//!
//! ## Vocabulary
//!
//! Vendor tags are the canonical anti-bot vocabulary for the crate.
//! [`crate::captcha::CaptchaKind`] (DOM-side, post-render detection) is the
//! *other evidence source for the same vendors* — `cloudflare` here lines up
//! with `CaptchaKind::CloudflareChallenge`/`Turnstile`, `datadome` with
//! `DatadomeBlock`, and so on. Keep the two reconciled rather than forked.
//!
//! The signature patterns are first-party, authored against the vendor list in
//! the MIT-licensed `albinstman/antibot-print` corpus (see `docs/antibot.md`
//! and `crates/core/CORPUS.md`). We hand-pick the vendors we actually meet
//! rather than vendoring the full foreign corpus, so the ruleset stays small,
//! readable, and ours to test.

use std::sync::OnceLock;

use regex::{RegexSet, RegexSetBuilder};
use serde::{Deserialize, Serialize};

/// Identifier for the signature corpus this verdict was produced against.
///
/// Recorded alongside every verdict so a replay-grade archive can reproduce the
/// classification deterministically: a verdict is a **captured fact**, not a
/// value to recompute at replay time against a newer corpus. Bump on any
/// change to [`CORPUS_JSON`].
pub const CORPUS_VERSION: &str = "cl-2026.06.01";

/// Maximum body prefix (bytes) normalized into the `B:` tier. Challenge
/// interstitials and widget script tags sit near the top of the document, and
/// presence tells are header-side, so a bounded prefix catches the signal
/// without regex-scanning multi-megabyte pages on the hot path.
pub const BODY_PREFIX_LIMIT: usize = 64 * 1024;

/// Which normalization tier produced the verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Evidence {
    /// No signal matched.
    None,
    /// Matched on the status line and/or response headers (cheap, high signal).
    Headers,
    /// Matched only after including the bounded body prefix.
    Body,
}

/// The result of fingerprinting a response.
///
/// Non-fatal by construction: presence is telemetry/routing input, never an
/// error on its own. A challenged verdict means "this fetch is sitting behind
/// an active wall" — callers decide whether to rotate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AntibotVerdict {
    /// Canonical vendor tags detected, sorted and deduplicated.
    pub vendors:          Vec<String>,
    /// `true` when at least one *challenge* signal matched (active blocking),
    /// as opposed to mere presence.
    pub challenged:       bool,
    /// The vendor whose challenge signal fired, when [`Self::challenged`].
    pub challenge_vendor: Option<String>,
    /// Corpus the verdict was produced against — see [`CORPUS_VERSION`].
    pub corpus_version:   &'static str,
    /// Which tier produced the verdict.
    pub evidence:         Evidence,
}

impl AntibotVerdict {
    /// A verdict with no detected vendor.
    fn empty() -> Self {
        Self {
            vendors:          Vec::new(),
            challenged:       false,
            challenge_vendor: None,
            corpus_version:   CORPUS_VERSION,
            evidence:         Evidence::None,
        }
    }

    /// `true` when any vendor was detected (challenged or merely present).
    pub fn detected(&self) -> bool {
        !self.vendors.is_empty()
    }
}

/// One vendor's signatures, as deserialized from [`CORPUS_JSON`].
#[derive(Debug, Deserialize)]
struct VendorSig {
    vendor:    String,
    signals:   Vec<String>,
    #[serde(default)]
    challenge: Vec<String>,
}

/// Compiled corpus: a single [`RegexSet`] over every signal, plus parallel
/// metadata mapping each pattern index back to its vendor and whether it is a
/// challenge (vs. presence) signal.
struct Compiled {
    set:  RegexSet,
    meta: Vec<SignalMeta>,
}

struct SignalMeta {
    vendor:    String,
    challenge: bool,
}

/// Lazily compile and cache the corpus. A bad pattern is skipped (it simply
/// can't match) rather than panicking — the workspace forbids `unwrap`/`panic`,
/// and a corrupt signal must never take down the fetch path.
fn compiled() -> &'static Compiled {
    static COMPILED: OnceLock<Compiled> = OnceLock::new();
    COMPILED.get_or_init(|| {
        let vendors: Vec<VendorSig> = serde_json::from_str(CORPUS_JSON).unwrap_or_default();

        let mut patterns: Vec<String> = Vec::new();
        let mut meta: Vec<SignalMeta> = Vec::new();
        for v in vendors {
            for p in v.signals {
                patterns.push(section_anchor(&p));
                meta.push(SignalMeta { vendor: v.vendor.clone(), challenge: false });
            }
            for p in v.challenge {
                patterns.push(section_anchor(&p));
                meta.push(SignalMeta { vendor: v.vendor.clone(), challenge: true });
            }
        }

        // Case-insensitive, linear-time RE2-style matching. `size_limit` caps a
        // pathological pattern's compiled-DFA memory so a future corpus edit
        // can't blow up process start. On failure, fall back to an empty set
        // (detection degrades to "nothing detected", never an error).
        let set = RegexSetBuilder::new(&patterns)
            .case_insensitive(true)
            .size_limit(8 * 1024 * 1024)
            .build()
            .unwrap_or_else(|_| RegexSet::empty());

        Compiled { set, meta }
    })
}

/// Rewrite a corpus pattern so a `b:` (body) signal matches its marker
/// **anywhere** within the single-line body section, while header/status
/// patterns stay anchored at their section-line start. Authors write
/// `b:grecaptcha` ("grecaptcha somewhere in the body"); we expand it to
/// `b:.*grecaptcha`. Header patterns already encode position (`h:server: …`)
/// and pass through unchanged.
fn section_anchor(pattern: &str) -> String {
    match pattern.strip_prefix("b:") {
        Some(rest) => format!("b:.*{rest}"),
        None => pattern.to_string(),
    }
}

/// Collapse newlines/carriage returns to spaces so a section stays on one
/// logical line — keeps section scoping (`b:.*marker` can't bleed past the
/// body line into a header line) and lets `.` match within a section.
fn flatten(s: &str) -> String {
    s.replace(['\n', '\r'], " ")
}

/// Normalize a status line + headers into the `S:`/`H:` form signals match
/// against. Header names and values are lowercased and joined `name: value`.
fn normalize_head(status: u16, headers: &[(String, String)]) -> String {
    let mut out = String::with_capacity(64 + headers.len() * 48);
    out.push_str("S:");
    out.push_str(&status.to_string());
    for (name, value) in headers {
        out.push_str("\nH:");
        out.push_str(&flatten(&name.to_lowercase()));
        out.push_str(": ");
        out.push_str(&flatten(&value.to_lowercase()));
    }
    out
}

/// Run the corpus over `haystack`, returning detected vendors and the
/// challenge vendor (if any). `vendors` is sorted and deduplicated.
fn scan(haystack: &str) -> (Vec<String>, Option<String>) {
    let c = compiled();
    let mut vendors: Vec<String> = Vec::new();
    let mut challenge_vendor: Option<String> = None;
    for idx in c.set.matches(haystack) {
        let Some(m) = c.meta.get(idx) else { continue };
        if !vendors.iter().any(|v| v == &m.vendor) {
            vendors.push(m.vendor.clone());
        }
        if m.challenge && challenge_vendor.is_none() {
            challenge_vendor = Some(m.vendor.clone());
        }
    }
    vendors.sort();
    (vendors, challenge_vendor)
}

/// Fingerprint a response. Runs status + headers first; only normalizes the
/// (bounded) body when the head tier found no challenge, so the common case
/// never touches the body.
///
/// `body` may be the full document — only the first [`BODY_PREFIX_LIMIT`] bytes
/// are scanned.
pub fn classify(status: u16, headers: &[(String, String)], body: &str) -> AntibotVerdict {
    let head = normalize_head(status, headers);
    let (head_vendors, head_challenge) = scan(&head);

    // Header tier already proves an active challenge — stop, don't touch body.
    if head_challenge.is_some() {
        return AntibotVerdict {
            vendors:          head_vendors,
            challenged:       true,
            challenge_vendor: head_challenge,
            corpus_version:   CORPUS_VERSION,
            evidence:         Evidence::Headers,
        };
    }

    // Otherwise fall back to the body prefix for 200-cloaking challenges and
    // body-only presence tells.
    let prefix_end =
        body.char_indices().map(|(i, _)| i).nth(BODY_PREFIX_LIMIT).unwrap_or(body.len());
    let mut full = head;
    full.push_str("\nB:");
    full.push_str(&flatten(&body[..prefix_end].to_lowercase()));
    let (vendors, challenge_vendor) = scan(&full);

    if vendors.is_empty() {
        return AntibotVerdict::empty();
    }

    // Body tier only "earns" Body evidence if it found something the head tier
    // didn't; otherwise the head tier was sufficient (presence-only).
    let evidence = if vendors.len() > head_vendors.len() || challenge_vendor.is_some() {
        Evidence::Body
    } else {
        Evidence::Headers
    };

    AntibotVerdict {
        vendors,
        challenged: challenge_vendor.is_some(),
        challenge_vendor,
        corpus_version: CORPUS_VERSION,
        evidence,
    }
}

/// First-party signature corpus. Patterns are RE2-style (no backreferences /
/// lookaround) so they match under the linear-time [`regex`] engine and are
/// safe to run on attacker-controlled input. Matched case-insensitively
/// against the normalized `S:`/`H:`/`B:` form.
///
/// Bump [`CORPUS_VERSION`] on any edit here.
const CORPUS_JSON: &str = r#"
[
  {
    "vendor": "cloudflare",
    "signals": ["h:server: cloudflare", "h:cf-ray:", "b:cdn-cgi/"],
    "challenge": [
      "h:cf-mitigated: challenge",
      "b:just a moment\\.\\.\\.",
      "b:challenges\\.cloudflare\\.com/turnstile",
      "b:cf-turnstile",
      "b:/cdn-cgi/challenge-platform",
      "b:cf_chl_"
    ]
  },
  {
    "vendor": "datadome",
    "signals": ["h:x-datadome", "h:set-cookie: datadome=", "b:datadome"],
    "challenge": ["b:geo\\.captcha-delivery\\.com", "b:captcha-delivery\\.com", "h:x-dd-b:"]
  },
  {
    "vendor": "akamai",
    "signals": ["h:server: akamaighost", "h:x-akamai-transformed", "b:ak_bmsc", "b:_abck"],
    "challenge": ["b:reference #[0-9a-f]{2}\\.", "b:errors\\.edgesuite\\.net"]
  },
  {
    "vendor": "imperva",
    "signals": ["h:x-iinfo", "h:set-cookie: visid_incap", "h:x-cdn: incapsula"],
    "challenge": ["b:_incapsula_resource", "b:incident id"]
  },
  {
    "vendor": "perimeterx",
    "signals": ["h:set-cookie: _px", "b:window\\._pxappid", "b:px-cdn"],
    "challenge": ["b:px-captcha", "b:/px/captcha", "b:perimeterx"]
  },
  {
    "vendor": "kasada",
    "signals": ["h:x-kpsdk-ct", "h:x-kpsdk-cd", "b:kpsdk"],
    "challenge": ["b:/_kpsdk", "b:ips\\.js"]
  },
  {
    "vendor": "awswaf",
    "signals": ["h:x-amzn-waf-action", "b:awswaf"],
    "challenge": ["b:token\\.awswaf", "b:challenge\\.compact"]
  },
  {
    "vendor": "f5",
    "signals": ["h:set-cookie: bigipserver", "h:set-cookie: ts[0-9a-f]{6}", "h:server: big-?ip"],
    "challenge": ["b:the requested url was rejected", "b:support id is"]
  },
  {
    "vendor": "sucuri",
    "signals": ["h:server: sucuri", "h:x-sucuri-id"],
    "challenge": ["h:x-sucuri-block", "b:sucuri website firewall"]
  },
  {
    "vendor": "cloudfront",
    "signals": ["h:x-amz-cf-id", "h:via:.*cloudfront"],
    "challenge": ["b:generated by cloudfront"]
  },
  {
    "vendor": "recaptcha",
    "signals": [],
    "challenge": ["b:www\\.google\\.com/recaptcha", "b:grecaptcha", "b:g-recaptcha"]
  },
  {
    "vendor": "hcaptcha",
    "signals": [],
    "challenge": ["b:hcaptcha\\.com", "b:h-captcha"]
  }
]
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn h(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect()
    }

    #[test]
    fn corpus_compiles_nonempty() {
        // If a pattern were malformed the set would silently fall back to empty;
        // assert the corpus actually compiled so a bad edit fails CI.
        assert!(compiled().set.len() > 10, "corpus failed to compile");
    }

    #[test]
    fn cloudflare_presence_only_is_not_challenged() {
        // A plain Cloudflare-fronted 200 — present, but serving us fine.
        let v = classify(
            200,
            &h(&[("server", "cloudflare"), ("cf-ray", "8a1b2c3d4e5f")]),
            "<html>ok</html>",
        );
        assert!(v.vendors.contains(&"cloudflare".to_string()));
        assert!(!v.challenged, "mere presence must not count as a challenge");
        assert_eq!(v.evidence, Evidence::Headers);
        assert!(v.challenge_vendor.is_none());
    }

    #[test]
    fn cloudflare_turnstile_interstitial_is_challenged() {
        // The fortress.theplumber.dev shape: Cloudflare managed challenge.
        let body = "<title>Just a moment...</title><script src=\"https://challenges.cloudflare.com/turnstile/v0/api.js\"></script>";
        let v = classify(403, &h(&[("server", "cloudflare"), ("cf-mitigated", "challenge")]), body);
        assert!(v.challenged);
        assert_eq!(v.challenge_vendor.as_deref(), Some("cloudflare"));
        // Header tier (`cf-mitigated: challenge`) proves it without the body.
        assert_eq!(v.evidence, Evidence::Headers);
    }

    #[test]
    fn datadome_block_via_header() {
        let v = classify(
            403,
            &h(&[("x-datadome", "protected"), ("set-cookie", "datadome=abc; Path=/")]),
            "blocked",
        );
        assert!(v.vendors.contains(&"datadome".to_string()));
    }

    #[test]
    fn body_cloaked_recaptcha_uses_body_tier() {
        // 200 with no telling header — only the body reveals the wall.
        let body = "<div class=\"g-recaptcha\" data-sitekey=\"x\"></div>";
        let v = classify(200, &h(&[("content-type", "text/html")]), body);
        assert!(v.vendors.contains(&"recaptcha".to_string()));
        assert!(v.challenged);
        assert_eq!(v.evidence, Evidence::Body);
    }

    #[test]
    fn clean_page_detects_nothing() {
        let v = classify(
            200,
            &h(&[("server", "nginx"), ("content-type", "text/html")]),
            "<html><body>hello</body></html>",
        );
        assert!(!v.detected());
        assert!(!v.challenged);
        assert_eq!(v.evidence, Evidence::None);
        assert_eq!(v.corpus_version, CORPUS_VERSION);
    }

    #[test]
    fn body_prefix_is_bounded() {
        // A challenge marker past the limit must not be scanned.
        let mut body = "x".repeat(BODY_PREFIX_LIMIT + 1024);
        body.push_str("h-captcha");
        let v = classify(200, &h(&[]), &body);
        assert!(!v.detected(), "markers past BODY_PREFIX_LIMIT must not match");
    }
}
