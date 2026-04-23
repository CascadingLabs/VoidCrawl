---
name: voidcrawl
description: Stealthy headless browsing and concurrent scraping via the voidcrawl-mcp MCP server. Use when a task needs bulk/parallel web fetches, a clean browser profile, bot-detection evasion (Cloudflare/Turnstile/fingerprinting), or per-subagent isolated sessions. Prefer this over claude-in-chrome whenever the work does NOT need to use the user's own visible browser.
---

# voidcrawl — stealthy, concurrent browsing

This skill exposes the `voidcrawl-mcp` MCP server, which drives headless Chrome through a stealth-patched tab pool. The pool has real concurrency: ten tabs in parallel is the norm, not a stretch.

## When to pick voidcrawl vs. claude-in-chrome

Pick **voidcrawl** when any of these are true:
- You need to scrape more than one URL and latency matters (parallel fan-out over the pool).
- The target has bot detection: Cloudflare, Turnstile, `navigator.webdriver` checks, fingerprinting.
- You're fanning out across subagents, each doing its own scrape — each subagent should hold its own `session_id`.
- You want a clean, isolated browser profile (no user cookies, no extensions, ephemeral storage).
- The work is scripted/unattended; the user doesn't need to watch it happen.

Pick **claude-in-chrome** when:
- The user's own authenticated browser matters (they are logged into the site).
- The user wants to visually watch the interaction in their own Chrome.
- You need a specific installed Chrome extension.
- The task is a single quick page and you can already see their active tab.

Default: if neither side of the decision fits exactly, voidcrawl is the safer pick for anything touching third-party web content.

## Tool map

| Need | Tool | Notes |
|---|---|---|
| Grab one page | `fetch` | Returns `{ url, status_code, redirected, html, title, extracted? }`. |
| Grab N pages in parallel | `fetch_many` | Input is an array of `FetchArgs`; output preserves order. Per-request errors don't abort the batch. |
| Screenshot a page | `screenshot` | Returns a PNG as `image/png` content. |
| Login / click through pagination / keep state | `session_open` → `session_navigate` → `session_content` → `session_close` | Each session is a dedicated Chrome with its own profile. Cookies never leak between sessions. |
| Click a CSS selector in a session | `click` | `{ session_id, selector }`. Real CDP click — handles hover + focus. |
| Click at pixel coordinates | `click_visual_coords` | `{ session_id, x, y }` in CSS pixels. Use when `click` fires but the target React handler doesn't respond. |
| Type text | `type_text` | With `selector`: focuses then types. Without: dispatches to the currently-focused element. |
| Run JS in the page | `eval_js` | Returns the expression value as JSON. |
| Read the `<title>` | `title` | Lightweight — no navigation. |
| Pull text of matching elements | `extract` | Runs `querySelectorAll` and returns text content per match. |
| Wait for network-idle mid-session | `wait_for_network_idle` | Event-driven; no sleeps. |
| See what the page loaded | `network_capture` | Returns Resource Timing entries (url, initiator type, transfer size, duration). |
| Detect a captcha / bot wall | `detect_captcha` | Returns `recaptcha` / `hcaptcha` / `turnstile` / `cloudflare_challenge` / `datadome` / `null`. |
| Check concurrency headroom | `pool_status` | Useful before kicking off a big fan-out. |

### `wait_for` knob (shared by fetch/screenshot/session_navigate)
- `"networkidle"` — default; driven by Chrome's network-idle lifecycle event.
- `"selector:<css>"` — driven by an in-page `MutationObserver`.

Both are event-driven — zero Rust-side polling, zero sleep-based fallbacks. If a selector never appears within `timeout_secs`, you get a proper `Timeout` error, not a tight loop.

### `extract` knob on `fetch`/`fetch_many`
Pass a JS expression evaluated in the page after the wait. Its return value comes back as `extracted`. Example: `"document.querySelector('h1').innerText"`.

## Subagent fan-out pattern

voidcrawl's big win over claude-in-chrome is that N subagents can each hold their own `session_id` and operate in parallel. The pool's semaphore arbitrates concurrency automatically.

```
main agent
├── Agent("scrape vendor A") → session_open → navigate → content → close → summary
├── Agent("scrape vendor B") → session_open → navigate → content → close → summary
└── Agent("scrape vendor C") → session_open → navigate → content → close → summary
```

Each subagent returns a distilled finding; the main agent never sees the raw DOM. Keep main-agent context clean.

## Operational notes

- **Always `session_close`** when a subagent is done. Otherwise the Chrome stays alive until the server exits.
- **Pool sizing** is read from env by the server (`BROWSER_COUNT`, `TABS_PER_BROWSER`, `TAB_MAX_USES`, `TAB_MAX_IDLE_SECS`). If `pool_status` shows `max_tabs` too low for your fan-out, tell the user to raise those in the MCP config.
- **Debug ports**: the MCP itself is stdio — no network port. Launched Chromes default to an OS-assigned ephemeral port on loopback, which never conflicts. If the user needs a pinned port (firewall / container mapping), set `CDP_PORT_BASE=<n>` in the MCP env; browser *i* binds `n + i`. For the Docker container, override with `CDP_PORT_BASE` / `CDP_PORTS` — 9222/9223 aren't privileged, so any free port works.
- **Stateless vs. stateful**: `fetch*` reuses pool tabs (recycled, not isolated). Use sessions when isolation matters; don't use sessions just to scrape one URL.
- **Errors**: `invalid_params` usually means a bad selector, URL, or JS expression. `internal_error` usually means a timeout or the browser crashed — retrying once is reasonable. Typed errors (captcha, profile lease) carry `data.exception` — dispatch on it.

## `click_visual_coords` recipe

Some React-rendered forms bind only to real browser input events; a `click` on a CSS selector (dispatchEvent-style) fires but the React handler never runs. The fix is to drive the form through the compositor:

1. `screenshot` — returns a PNG. Identify the pixel coordinates of the target (e.g. the submit button).
2. `click_visual_coords { x, y }` — sends `mousePressed` + `mouseReleased` at those coords with left button. The React handler fires.
3. `type_text { text }` — with no selector, dispatches keys to the currently-focused element.
4. Repeat for the submit button.

Coordinates are **CSS pixels, pre-DPR**. On HiDPI displays the screenshot may be 2× the CSS-pixel size — divide by `devicePixelRatio` (call `eval_js "window.devicePixelRatio"`) before passing to `click_visual_coords`.

## Captcha behavior

`detect_captcha` and navigation-time captcha detection are **DOM-only**. False negatives are possible (visual-only captchas, unusual markers). The contract for agents:

- If you see `CaptchaDetected` in an error (check `data.exception`), **surface the failure**. Don't retry the same URL — upstream rotation (proxy/profile) should happen before the next attempt.
- Don't try to solve. voidcrawl rotates around captchas, never solves them.

## Bot-wall hygiene

- Don't hammer identical URLs — if a site blocks one request, the next one is likelier to as well.
- Reuse a single session for same-origin work (cookies, realistic request pacing).
- Space requests: back-to-back `fetch_many` on a bot-managed domain is a fast way to get the IP tainted.

## Profile pinning (pipeline use only)

`voidcrawl-mcp` can be launched with `--profile NAME` (or `VOIDCRAWL_PROFILE=NAME` env) to pin the whole server to a warm Chrome profile. This is a pipeline-level concern — agents don't acquire profiles themselves, and there are no MCP tools for profile management. If the server was pinned at startup and another voidcrawl process is already holding the same profile, startup fails with `ProfileBusy`.
