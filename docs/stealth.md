# Stealth & Anti-Detection

void_crawl uses a **minimal-footprint** stealth strategy inspired by [zendriver](https://github.com/nicegamer7/zendriver) / [nodriver](https://github.com/nicegamer7/nodriver) — the async successor to undetected-chromedriver. Stealth is **enabled by default**.

## Philosophy: Less Is More

Most browser automation tools try to _spoof_ every fingerprint signal — fake plugins, fake WebGL, fake user-agent strings. This backfires against modern WAFs (Akamai, Cloudflare, PerimeterX) because:

1. **Spoofed values are inconsistent.** A hardcoded `Chrome/131` user-agent on a system running Chromium 146 is an instant flag. Fake WebGL renderer strings that don't match the actual GPU are trivially detected.

2. **The spoofing itself is detectable.** Each `Page.addScriptToEvaluateOnNewDocument` CDP call is a fingerprint. Overriding `navigator.plugins` with a Proxy/getter behaves differently from the real `PluginArray` prototype — detection scripts check for exactly this.

3. **The automation signal isn't in JS — it's in Chrome's launch flags.** chromiumoxide's default flags include `--enable-automation`, which tells every WAF "I'm automated" before any page loads.

void_crawl's approach: **don't fake anything** except the one property Chrome explicitly sets for automation (`navigator.webdriver`). Instead, launch Chrome with clean flags that don't advertise automation.

## What Changed from chromiumoxide Defaults

### Removed (toxic flags)

| Flag | Why it's bad |
|---|---|
| `--enable-automation` | Literally opts in to automation detection |
| `--disable-extensions` | Normal Chrome always has extensions support |
| `--enable-blink-features=IdleDetection` | Unusual feature that fingerprints automation |

### Added (zendriver flags)

| Flag | Purpose |
|---|---|
| `--disable-blink-features=AutomationControlled` | Removes Chrome's automation-controlled blink feature |
| `--disable-features=IsolateOrigins,site-per-process` | Disables site isolation that WAFs use for fingerprinting |
| `--no-pings` | Suppresses background pings |
| `--disable-component-update` | Prevents background update checks |
| `--disable-session-crashed-bubble` | Suppresses crash UI |
| `--disable-search-engine-choice-screen` | Suppresses search engine prompt |
| `--homepage=about:blank` | Clean startup page |

### Kept (safe defaults)

Standard flags like `--disable-background-networking`, `--disable-breakpad`, `--disable-dev-shm-usage`, `--no-first-run`, etc. are retained — they reduce noise without being automation signals.

## What the JS Stealth Layer Does

Only two patches are injected via `addScriptToEvaluateOnNewDocument`:

### 1. `navigator.webdriver` Removal

Chrome explicitly sets `navigator.webdriver = true` when connected via CDP. We delete it from the prototype chain and redefine it as `undefined`:

```javascript
delete Object.getPrototypeOf(navigator).webdriver;
Object.defineProperty(navigator, 'webdriver', {
    get: () => undefined,
    configurable: true,
});
```

### 2. Force-Open Shadow DOMs

Cloudflare Turnstile and similar WAF challenges render inside closed shadow roots. We force all `attachShadow` calls to use `mode: 'open'` so the automation layer can interact with challenge elements:

```javascript
Element.prototype._attachShadow = Element.prototype.attachShadow;
Element.prototype.attachShadow = function(init) {
    return this._attachShadow({ ...init, mode: 'open' });
};
```

### What We Don't Patch (and why)

| Signal | Why we leave it alone |
|---|---|
| `navigator.plugins` | Real Chrome already populates this. Faking it creates detectable inconsistencies. |
| `navigator.userAgent` | We use Chrome's real UA. Hardcoding a version creates a mismatch with the actual browser. |
| WebGL vendor/renderer | The real GPU info from the system is more convincing than any fake string. |
| `window.chrome.runtime` | Real Chrome already has this. |
| `navigator.permissions` | The default behavior is already correct. |
| Canvas fingerprint | Can't be spoofed reliably without introducing detectable noise. |

## Headful vs Headless

For WAF-protected sites (Akamai, Cloudflare), **headful mode is required**. Headless Chrome has fundamental differences that sophisticated WAFs detect regardless of JS patches:

- Different rendering pipeline (no compositing)
- Missing screen/display properties
- HTTP/2 TLS fingerprint differences

```python
import os
from void_crawl import BrowserPool

# For WAF-protected sites — use headful (set CHROME_HEADLESS=0)
os.environ["CHROME_HEADLESS"] = "0"
async with BrowserPool.from_env() as pool:
    async with pool.acquire() as tab:
        await tab.navigate("https://waf-protected-site.com")
        await tab.wait_for_network_idle(timeout=15.0)
        html = await tab.content()

# For unprotected sites — headless is fine and faster
async with BrowserPool.from_env() as pool:
    async with pool.acquire() as tab:
        await tab.navigate("https://example.com")
        html = await tab.content()
```

## Waiting for readiness (event-driven)

JS-heavy sites and WAF challenge pages don't have their content ready at page load. void_crawl exposes two event-driven waits — no polling, no sleeps.

```python
async with pool.acquire() as tab:
    await tab.navigate(url)

    # Option 1 — wait for Chrome's networkIdle lifecycle event.
    # Returns the event name on success, None on timeout.
    await tab.wait_for_network_idle(timeout=15.0)

    # Option 2 — wait for a specific CSS selector to appear.
    # Driven by an in-page MutationObserver; resolves the moment the
    # element is inserted (or rejects with a timeout).
    await tab.wait_for_selector("#results", timeout=15.0)
```

## Disabling Stealth

```python
# Via BrowserSession
async with BrowserSession(stealth=False) as session:
    page = await session.new_page("https://trusted-site.com")

# Stealth is always on for BrowserPool (via vc.pool())
# The minimal patches have negligible overhead
```

## A Note on CDP `networkIdle` Events

Chrome's CDP exposes `Page.lifecycleEvent` events that fire in sequence during page load:

```
DOMContentLoaded → load → networkAlmostIdle → networkIdle
```

`networkIdle` fires when the browser has had **zero in-flight network requests for 500ms**. This sounds like the perfect "page is ready" signal, but it is **unreliable for SPAs and modern web apps** because:

- **WebSocket connections** (chat, real-time data) keep the network permanently active.
- **Server-Sent Events (SSE)** and long-polling hold open persistent HTTP connections.
- **Analytics and telemetry** (Google Analytics, Segment, etc.) fire periodic beacons that reset the 500ms idle window.
- **Lazy-loaded content** triggers new requests as the page renders, creating a moving target.

On these sites, `networkIdle` may **never fire** — or fire only after an unpredictable delay. Prefer `wait_for_selector("<the element you actually care about>")`: it's driven by an in-page `MutationObserver` and fires the moment that element is inserted, regardless of whether the network ever settles.

## Real-World Results

Tested against Akamai WAF (BusinessWire) — the same site that blocks Playwright, Selenium, and even chromiumoxide with heavy stealth patches:

| Approach | Result |
|---|---|
| chromiumoxide defaults (`--enable-automation`) | 403 Access Denied |
| chromiumoxide + heavy JS spoofing + fake UA | 403 Access Denied |
| chromiumoxide + `disable_default_args` + clean flags + no UA override | **Success (600K chars)** |
| zendriver (reference) | Success |
| Plain curl | 403 Access Denied |

The lesson: **the flags matter more than the JS patches.**
