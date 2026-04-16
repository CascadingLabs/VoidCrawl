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
- **Stateless vs. stateful**: `fetch*` reuses pool tabs (recycled, not isolated). Use sessions when isolation matters; don't use sessions just to scrape one URL.
- **Errors**: `invalid_params` usually means a bad selector, URL, or JS expression. `internal_error` usually means a timeout or the browser crashed — retrying once is reasonable.
