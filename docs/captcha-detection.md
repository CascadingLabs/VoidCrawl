# Captcha / Bot-Wall Detection

`voidcrawl` surfaces captcha and interstitial bot-walls as a typed signal so pipelines can rotate upstream (swap proxy, swap profile) instead of banging on a blocked URL.

## Scope

**DOM-only** in v0.3. One JS probe runs against the live document and returns the first matching marker.

## What's detected

| Marker | Detected via | Returned kind |
|---|---|---|
| reCAPTCHA v2/v3 | `iframe[src*="google.com/recaptcha"]`, `.g-recaptcha` | `recaptcha` |
| hCaptcha | `iframe[src*="hcaptcha.com"]`, `.h-captcha`, `[data-hcaptcha-widget-id]` | `hcaptcha` |
| Cloudflare Turnstile | `iframe[src*="challenges.cloudflare.com/turnstile"]`, `.cf-turnstile` | `turnstile` |
| Cloudflare interstitial | `#cf-challenge-running`, `#cf-chl-widget`, `.cf-browser-verification`, title "Just a moment" / "Attention required" | `cloudflare_challenge` |
| DataDome | `#datadome-captcha`, `[id^="dd_"]` | `datadome` |
| PerimeterX | `#px-captcha` | `unknown:perimeterx` |

## Known gaps

- **Visual-only challenges** (image puzzles without DOM markers) are missed.
- **Shadow-root captchas**: this DOM *probe* doesn't pierce closed shadow roots.
  To *interact* with a widget that lives in one (e.g. Cloudflare Turnstile's
  checkbox in a closed shadow root inside a cross-origin frame), use the
  accessibility-tree locators — `ax_outline_in_frame` / `ax_box_in_frame` /
  `click_ax_in_frame` — which the browser-computed AX tree lets descend into
  closed roots. See [cross-origin-frames.md](cross-origin-frames.md).
- **Delayed injection**: if the challenge renders after DOM ready, `detect_captcha` must be called *after* the page settles (use `wait_for_network_idle` first).
- **DataDome**: some deployments use rotating class names; we only catch the ID-based fingerprint.

Visual diffing against known captcha fingerprints is not in scope for v0.3. Open an issue if you hit a real-world false negative — we'll add DOM markers as they surface.

## API

### Python

```python
from voidcrawl import BrowserSession, CaptchaDetected

async with BrowserSession() as b:
    page = await b.new_page("https://protected.example.com")
    kind = await page.detect_captcha()
    if kind is not None:
        # caller decides: retry with a different proxy, a different
        # profile, or surface the failure upstream
        raise CaptchaDetected(f"blocked by {kind}")
```

### MCP

The `detect_captcha` tool returns `{ "kind": "recaptcha" | ... | null }`. When a captcha is raised as an error mid-operation, `ErrorData.data.exception == "CaptchaDetected"` and `data.kind` carries the tag.

## Philosophy

`voidcrawl` **never solves** captchas. Solving is a cat-and-mouse game and a legal gray area. The project's stance is: rotate around them (different egress IP, different warm profile), or surface the failure so a human can decide.
