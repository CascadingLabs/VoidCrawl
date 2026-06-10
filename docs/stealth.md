# Stealth & Anti-Detection

void_crawl uses a **minimal-footprint** stealth strategy inspired by
[zendriver](https://github.com/nicegamer7/zendriver) /
[nodriver](https://github.com/nicegamer7/nodriver). Stealth is **enabled by
default**. The guiding rule: *present a real browser consistently — don't fake
things.*

## TL;DR (at a glance)

What voidcrawl does, and why each piece exists:

| Layer | What we do | Why |
|---|---|---|
| **Launch flags** | Drop chromiumoxide's `--enable-automation`/`--disable-extensions`; add `--disable-blink-features=AutomationControlled` + zendriver flags | The automation signal lives in the flags, not JS. `AutomationControlled` is what makes `navigator.webdriver` **`false`** (a native value — we do **not** patch it in JS). |
| **No JS injection** | `addScriptToEvaluateOnNewDocument` is **empty** | Each injected script is itself a fingerprint. We patch nothing in page-world JS. |
| **UA / Client Hints** | Real UA (Headless stripped), with `navigator.platform` + `userAgentData` (Sec-CH-UA) **derived from that UA** so they agree | A Linux UA with `platform === "Win32"` or empty `brands` is a bot tell. |
| **GPU** | `--headless=new` + ANGLE + `--disable-gpu-sandbox` → **hardware** WebGL | Legacy headless renders WebGL with **SwiftShader** (software) — a strong bot signal. |

**Managed Cloudflare Turnstile** (the hard case):

| Mode | Result |
|---|---|
| **Headful** | ✅ Passes managed Turnstile non-interactively (verified server-side: `siteverify` `success:true, interactive:false`). |
| **Headless** | ❌ Still gated — the challenge stalls at `before-interactive`. Use headful for Turnstile-walled targets. |

All defaults are **overridable** by the caller (see [Overriding the defaults](#overriding-the-defaults)).

## Philosophy: less is more

Most automation tools try to *spoof* every fingerprint — fake plugins, fake
WebGL, fake UA. This backfires against modern WAFs (Akamai, Cloudflare,
PerimeterX) because:

1. **Spoofed values are inconsistent.** A hardcoded `Chrome/131` UA on a
   Chromium 148 build is an instant flag. A fake WebGL renderer that doesn't
   match the real GPU is trivially caught.
2. **The spoofing itself is detectable.** Every
   `Page.addScriptToEvaluateOnNewDocument` call is a fingerprint. Overriding
   `navigator.plugins` with a Proxy behaves differently from the real
   `PluginArray` prototype — and detectors check for exactly that.
3. **The automation signal is in the launch flags, not JS.** chromiumoxide's
   defaults include `--enable-automation`, which tells every WAF "I'm
   automated" before a page loads.

void_crawl's approach: **don't fake anything.** Launch with clean flags, let
Chrome report its real values, and only ensure those values are *internally
consistent*.

> **Lesson learned the hard way:** voidcrawl used to inject two JS patches —
> deleting `navigator.webdriver` and force-opening shadow DOMs. Both were
> removed. Deleting `navigator.webdriver` made it `undefined` (real Chrome
> reports `false` — `undefined` is the tell). Force-opening shadow DOMs broke
> **Cloudflare Turnstile**, which renders its challenge in a *closed* shadow
> root and tamper-checks it: forcing it open failed the challenge with
> `ERROR 600010`. We inject **zero** page-world JS today. To *reach into* a
> closed shadow root without tampering, use the AX-tree locators
> (`ax_box_in_frame` / `click_ax_in_frame`) — the browser-computed accessibility
> tree descends into closed roots, so a trusted compositor click can drive the
> widget with no shadow patch. See
> [cross-origin-frames.md](cross-origin-frames.md).

## The automation signal is in the launch flags

After `disable_default_args()` (which strips chromiumoxide's toxic defaults) we
re-add a curated set. Flags are stored **without** the leading `--`
(chromiumoxide prepends it; a literal `--` would produce the inert `----flag` —
a bug we fixed, which had silently disabled the whole list).

### Removed (toxic defaults)

| Flag | Why it's bad |
|---|---|
| `--enable-automation` | Literally opts in to automation detection |
| `--disable-extensions` | Real Chrome always has extension support |

### Anti-automation flags we add

| Flag | Purpose |
|---|---|
| `--disable-blink-features=AutomationControlled` | Removes the automation-controlled blink feature → `navigator.webdriver` is a native `false` |
| `--disable-features=IsolateOrigins,site-per-process,TranslateUI` | Disables isolation/UI WAFs fingerprint on |
| `--no-pings`, `--disable-component-update`, `--disable-session-crashed-bubble`, `--disable-search-engine-choice-screen`, `--homepage=about:blank` | Suppress automation-ish background behavior + UI |

Plus the safe noise-reducers (`--disable-background-networking`,
`--disable-breakpad`, `--disable-dev-shm-usage`, `--no-first-run`, …).

## UA / platform / Client-Hints consistency

This is the only "override" we apply, via CDP `Emulation.setUserAgentOverride`
(not page-world JS) in `Page::apply_stealth`. We probe the browser's **real**
UA, strip any `Headless` token, and from that one string derive a coherent
identity so UA, `navigator.platform`, and `navigator.userAgentData` all agree:

| Signal | Value (for the real Linux/Chrome UA) |
|---|---|
| `navigator.userAgent` | real build, `HeadlessChrome` → `Chrome` |
| `navigator.platform` | `Linux x86_64` (Win32 / MacIntel for those UAs) |
| `Sec-CH-UA-Platform` | `Linux` / `Windows` / `macOS` |
| `userAgentData.brands` / `fullVersionList` | `Chromium`/`Google Chrome` at the UA's major/full version + a GREASE entry |

A mismatch here (e.g. the old hardcoded `platform: "Win32"` on a Linux UA, or
empty `brands`) is itself a bot signal — both flagged by the
[rebrowser bot-detector](https://bot-detector.rebrowser.net/), now green.

## GPU acceleration (hardware WebGL)

A headless browser that renders WebGL with **SwiftShader** (Chrome's software
fallback) advertises itself: `WEBGL_debug_renderer_info` returns
`"ANGLE (… SwiftShader …)"`, which Cloudflare and others weight as "no real
GPU → likely a bot/VM."

void_crawl forces **hardware** rendering:

- **`--headless=new`** — the legacy `--headless` forces SwiftShader; the new
  mode runs the full browser stack and can use a real GPU.
- **`--use-angle=vulkan` + `--enable-gpu` + `--ignore-gpu-blocklist`** — route
  WebGL through ANGLE on the real GPU.
- **`--disable-gpu-sandbox`** — lets the GPU process reach the DRM render node.
  *This is the lever* — on a host with a working driver it's usually all you
  need (no `VK_DRIVER_FILES` juggling).

Verified on AMD (RADV): renderer becomes
`ANGLE (AMD, Vulkan … (AMD Radeon … RADV …)), radv)` — hardware, not
SwiftShader. The defaults are **vendor-generic** (ANGLE uses whatever
Intel/AMD/NVIDIA driver the machine has); nothing is hardcoded per vendor.

**In Docker**, hardware GPU additionally needs Mesa drivers in the image +
`/dev/dri` passthrough — see [docker-mcp.md](docker-mcp.md) and
[docker-headful.md](docker-headful.md). Without a GPU passed through, the
container falls back to SwiftShader.

To force software rendering (or pick a different backend), override
`--use-angle` (see below).

## What we don't touch (and why)

We inject **no** page-world JS, and we leave these alone:

| Signal | Why |
|---|---|
| `navigator.webdriver` | The launch flag already yields a native `false`. A JS patch (deleting it → `undefined`, or a redefined getter) is itself detectable. |
| `navigator.plugins` | Real Chrome populates it; faking creates inconsistencies. |
| `navigator.userAgent` | We use the real UA (Headless stripped) — no version mismatch. |
| WebGL vendor/renderer | The real GPU string (once hardware-accelerated) beats any fake. |
| `window.chrome.runtime`, `navigator.permissions`, canvas | Default behavior is already correct; spoofing adds detectable noise. |
| Shadow DOM mode | We do **not** force-open it (it broke Turnstile). Interacting with a challenge widget works via real compositor clicks at pixel coordinates regardless of shadow mode — locate the target inside a closed root with `ax_box_in_frame` / `click_ax_in_frame` (see [cross-origin-frames.md](cross-origin-frames.md)). |

## Headful vs headless (and managed Turnstile)

For the toughest WAFs, **headful is required** — headless has detectable
differences that survive every JS patch:

- Different rendering/compositing pipeline.
- Missing / non-default screen, media, and input-related properties.
- The managed-challenge score is simply lower.

Concretely, against **managed Cloudflare Turnstile** with a real sitekey
(verified server-side via `siteverify`):

| Mode | Outcome |
|---|---|
| Headful | **Pass**, non-interactive (`success:true, interactive:false`) |
| Headless | Stalls at `before-interactive`; no token |

```python
import os
from voidcrawl import BrowserPool

# WAF / managed-Turnstile targets — headful:
os.environ["CHROME_HEADLESS"] = "0"
async with BrowserPool.from_env() as pool:
    async with pool.acquire() as tab:
        await tab.navigate("https://waf-protected-site.com")
        await tab.wait_for_network_idle(timeout=15.0)
        html = await tab.content()

# Unprotected / bulk targets — headless is fine and faster (default).
```

For a headless *farm* that still needs to clear Turnstile, run the **headful
GPU container** ([docker-headful.md](docker-headful.md)) rather than headless.

## Overriding the defaults

Every default flag is overridable by the caller — useful to force a GPU
backend, disable acceleration, add a proxy bypass, etc. Caller args are merged
by **switch key**, so a caller value *replaces* the matching default (we don't
emit duplicate switches — Chrome's per-switch precedence is inconsistent):

```python
from voidcrawl import BrowserConfig

# Force software rendering (e.g. to compare, or on a GPU-less box):
cfg = BrowserConfig(extra_args=["--use-angle=swiftshader"])

# Disable the GPU entirely:
cfg = BrowserConfig(extra_args=["--disable-gpu"])
```

The same applies through the MCP server / pool config (`extra_args`).

## Waiting for readiness (event-driven)

JS-heavy sites and challenge pages aren't ready at page load. Two event-driven
waits — no polling, no sleeps:

```python
async with pool.acquire() as tab:
    await tab.navigate(url)
    # Chrome's networkIdle lifecycle event (returns event name, or None on timeout):
    await tab.wait_for_network_idle(timeout=15.0)
    # …or an in-page MutationObserver for a specific selector:
    await tab.wait_for_selector("#results", timeout=15.0)
```

### Why `networkIdle` is unreliable for SPAs

`networkIdle` fires after **zero in-flight requests for 500ms** — but
WebSockets, SSE/long-polling, analytics beacons, and lazy-loading keep the
network active, so on many modern apps it **never fires**. Prefer
`wait_for_selector("<the element you actually care about>")`: it resolves the
moment that element is inserted, regardless of network.

## Real-world results

| Target | Approach | Result |
|---|---|---|
| Akamai WAF (BusinessWire) | chromiumoxide defaults (`--enable-automation`) | 403 |
| Akamai WAF (BusinessWire) | + heavy JS spoofing + fake UA | 403 |
| Akamai WAF (BusinessWire) | `disable_default_args` + clean flags + real UA | **Success** |
| Managed Cloudflare Turnstile (real sitekey) | headful, hardware GPU, consistent UA, no JS injection | **Pass** (`siteverify success:true`) |
| Managed Cloudflare Turnstile | headless | Gated (`before-interactive`) |

The lesson, twice over: **the flags + a consistent real browser matter more
than JS patches — and a wrong JS patch is worse than none.**
