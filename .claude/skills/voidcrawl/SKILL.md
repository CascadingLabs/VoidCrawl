---
name: voidcrawl
description: Stealthy, concurrent headless browsing via the voidcrawl-mcp MCP server. Use for bulk/parallel web fetches, JS-rendered pages, bot-detection evasion (Cloudflare/Turnstile/fingerprinting), isolated per-task browser sessions, or durable element targeting through the accessibility tree. Runs headless and unattended — no visible browser needed.
---

# voidcrawl — stealthy, concurrent browsing

`voidcrawl-mcp` drives headless Chrome through a stealth-patched, recycled tab
pool exposed over the Model Context Protocol. The pool has real concurrency —
ten tabs in parallel is normal, not a stretch — and every tab is fingerprint-
and `navigator.webdriver`-patched.

This skill is harness-neutral: it works the same whether the MCP server is wired
into opencode, Codex, Claude Code, or a Yosoi pipeline. Nothing here assumes a
particular host agent.

## When to reach for voidcrawl

Reach for it when **any** of these hold:
- **Volume + latency** — more than one URL to fetch and you want parallel fan-out.
- **Bot walls** — Cloudflare, Turnstile, `navigator.webdriver` checks, TLS/JS fingerprinting.
- **JS rendering** — the data only exists after scripts run (SPAs, lazy lists).
- **Isolation** — you want a clean, cookieless, extension-free profile, or N parallel tasks that must not share state.
- **Unattended** — scripted work nobody needs to watch.

It is **not** the tool when you specifically need a *human's own authenticated,
visible browser* (logged-in session, a watched interaction, a particular
installed extension). In Claude Code that other tool is `claude-in-chrome`; in
most other hosts there's simply no such need and voidcrawl is the default for
anything touching third-party web content.

## Coming from Playwright / Chromium MCP

voidcrawl is a drop-in replacement for the Playwright MCP and Chrome DevTools
MCP servers, with **stealth and real pool concurrency** as the reasons to
switch — same headless-Chrome capabilities, but fingerprint-patched and built to
fan out. The mental model carries over directly:

| Playwright / Chromium MCP | voidcrawl | Note |
|---|---|---|
| `browser_navigate` | `session_navigate` (or `fetch` for one-shot) | |
| `browser_snapshot` (a11y tree) | `session_ax_tree` | Same accessibility-tree snapshot; compact by default. |
| `browser_click` (by ref/role) | `click_by_role` / `click` | Role+name *or* CSS. |
| `browser_type` | `type_text` | |
| `browser_take_screenshot` | `screenshot` | |
| `browser_evaluate` | `eval_js` | |
| `browser_network_requests` | `network_capture` | |
| *(no equivalent)* | `fetch_many` | Parallel fan-out over the pool. |
| *(no equivalent)* | `detect_captcha`, stealth patches | Bot-wall handling is built in. |

If a workflow worked against Playwright MCP, the same steps work here — reach for
`session_ax_tree` where you'd have called `browser_snapshot`, and you additionally
get stealth, `fetch_many` concurrency, and isolated profiles for free.

## Tool map

**Fetch (stateless, pool-recycled tabs):**
| Need | Tool | Notes |
|---|---|---|
| One page | `fetch` | `{ url, status_code, redirected, html, title, extracted? }`. |
| N pages in parallel | `fetch_many` | Array of `FetchArgs`; output preserves input order; per-request errors don't abort the batch. |
| Screenshot | `screenshot` | Returns a PNG as `image/png`. |

**Sessions (stateful, isolated Chrome — for login, pagination, multi-step flows):**
| Need | Tool | Notes |
|---|---|---|
| Lifecycle | `session_open` → `session_navigate` → … → `session_close` | Each session is a dedicated Chrome + profile. Cookies never leak across sessions. |
| Read page HTML/title/url | `session_content` | The whole document — large; prefer `extract`/AX below. |
| Perceive the page | `session_ax_tree` | Compact `role "name"` outline (or `mode:"raw"` for full CDP nodes). See the ladder below. |
| Click by selector | `click` | `{ session_id, selector }`. Real CDP click (hover + focus). |
| Click by role + name | `click_by_role` | `{ session_id, role, name, nth? }`. Durable across redesigns. |
| Click by pixel | `click_visual_coords` | `{ session_id, x, y }` CSS px. Last resort (see recipe). |
| Type | `type_text` | With `selector`: focus+type. Without: to whatever's focused. |
| Run JS | `eval_js` | Returns the expression value as JSON. |
| Pull element text | `extract` | `querySelectorAll` → text content per match. |
| Title only | `title` | Lightweight, no navigation. |
| Wait for idle | `wait_for_network_idle` | Event-driven; no sleeps. |
| What loaded | `network_capture` | Resource Timing entries (url, initiator, size, duration). |
| Captcha probe | `detect_captcha` | `recaptcha`/`hcaptcha`/`turnstile`/`cloudflare_challenge`/`datadome`/`null`. |
| Concurrency headroom | `pool_status` | Check before a big fan-out. |

## Working with a page: perceive → act → extract

There are only three things you ever do with a page. Each has a **preferred
tool and a fallback ladder — climb down a rung only when the one above it
fails.** Picking the right rung is mostly what separates a cheap, robust scrape
from a slow, brittle one.

### Perceive — "what's on this page?"
Default to the cheapest representation. **Raw HTML is a context bomb — don't dump
it to reason over a page.**
1. **`session_ax_tree`** (compact, default) — a pruned `role "name"` outline of
   everything meaningful and interactive. Semantic and typically 10–50× smaller
   than HTML. This is the right default for "show me the page."
   - **Trust signal:** compare `named_count` to `node_count`. A low ratio means a
     thin AX tree (a div-soup site with poor accessibility) — *don't over-trust
     it*; fall to a screenshot or a targeted HTML/`extract` pull.
2. **`screenshot`** — when layout, visual state, or pixel position matters, or
   when the AX tree came back thin.
3. **`session_content` / raw HTML** — last resort. If you only need a few fields,
   reach for `extract` instead of reading the whole document.

### Act — "click / type here"
1. **`click`** (CSS selector) — when you have a stable selector.
2. **`click_by_role`** (accessibility role + accessible name) — when selectors are
   brittle or the markup churns between deploys. It targets *what assistive tech
   sees*, so it survives refactors like `<div role="button" aria-label="Save">`
   → `<button>Save</button>` that shatter CSS selectors. Read the exact role/name
   from `session_ax_tree` first; use `nth` to disambiguate duplicates.
3. **`click_visual_coords`** (pixel x, y) — last resort for React/compositor-only
   forms that ignore synthetic clicks (see recipe).
- **Type** with `type_text`: with a `selector` it focuses then types; without, it
  dispatches to whatever currently has focus (pair with a click first).

### Extract — "give me the data"
1. **`extract` / `eval_js`** with a JS expression — pull exactly the fields you
   want, returned as JSON. Cheapest and most precise; favor this over reading HTML.
2. **`session_ax_tree`** — when the thing you want is keyed by role/name (e.g.
   "every `button` whose name is a headline") rather than a clean selector.

### `wait_for` knob (shared by fetch / screenshot / session_navigate)
- `"networkidle"` (default) — Chrome's network-idle lifecycle event.
- `"selector:<css>"` — an in-page `MutationObserver`.

Both are event-driven — no Rust-side polling, no sleep fallbacks. A selector that
never appears yields a clean `Timeout` error, not a spin loop.

### `extract` knob on `fetch` / `fetch_many`
Pass a JS expression evaluated after the wait; its return comes back as
`extracted`, e.g. `"document.querySelector('h1').innerText"`. Use it to bring back
*data*, not markup.

## Subagent fan-out & token economy

The pool's biggest win is parallel isolation: N subagents each hold their own
`session_id` and run concurrently; the semaphore arbitrates automatically.

```
main agent
├── Agent("scrape vendor A") → session_open → navigate → perceive/extract → close → summary
├── Agent("scrape vendor B") → session_open → navigate → perceive/extract → close → summary
└── Agent("scrape vendor C") → session_open → navigate → perceive/extract → close → summary
```

Each subagent returns a **distilled finding**, never raw DOM. The main agent's
context stays clean. Two rules that keep token cost sane:
- Perceive with the **AX outline**, not HTML.
- Extract **data** with `extract`/`eval_js`, don't ship the page back.

## Operational notes
- **Always `session_close`** when done — an open session keeps its Chrome alive until the server exits.
- **Pool sizing** comes from server env (`BROWSER_COUNT`, `TABS_PER_BROWSER`, `TAB_MAX_USES`, `TAB_MAX_IDLE_SECS`). If `pool_status` shows `max_tabs` too low for your fan-out, ask the user to raise them in the MCP config.
- **Stateless vs. stateful:** `fetch*` reuses recycled (non-isolated) pool tabs — right for one-shot grabs. Use **sessions** when isolation or multi-step state matters; don't open a session just to read one URL.
- **Errors:** `invalid_params` ≈ bad selector/URL/JS. `internal_error` ≈ a timeout or a browser crash — retrying once is reasonable. Typed errors (captcha, profile lease) carry `data.exception` — dispatch on it.
- **Ports:** the MCP transport is stdio (no port). Launched Chromes bind ephemeral loopback ports. For a pinned port (firewall/container), set `CDP_PORT_BASE=<n>`; browser *i* binds `n+i`.

## `click_visual_coords` recipe
Some React forms bind only to real compositor input; a selector `click`
(dispatchEvent-style) fires but the handler never runs. Drive it through pixels:
1. `screenshot` → identify the target's pixel coordinates.
2. `click_visual_coords { x, y }` → sends `mousePressed`+`mouseReleased`; the handler fires.
3. `type_text { text }` (no selector) → keys go to the now-focused element.
4. Repeat for submit.

Coordinates are **CSS pixels, pre-DPR**. On HiDPI the screenshot may be 2× — divide by `devicePixelRatio` (`eval_js "window.devicePixelRatio"`) before passing them.

## Captcha contract
`detect_captcha` and navigation-time detection are **DOM-only** — false negatives
happen (visual-only captchas, novel markers). Default agent contract:
- On a `CaptchaDetected` error (check `data.exception`), **surface the failure**.
  Don't re-hit the same URL — upstream rotation (proxy/profile) must happen first.
- **Don't try to solve.** voidcrawl rotates *around* captchas by design.
- `capture_captcha` / `solve_captcha` / `inject_captcha_token` exist for
  pipeline flows that integrate an external solver — they are not the default
  path and shouldn't be reached for during ordinary scraping.

## Bot-wall hygiene
- Don't hammer identical URLs — one block makes the next likelier.
- Reuse a single session for same-origin work (cookies, realistic pacing).
- Space requests: back-to-back `fetch_many` against a bot-managed domain is a fast way to taint the IP. Throttle and, if available, rotate proxies/profiles.

## Profile pinning (pipeline use only)
`voidcrawl-mcp --profile NAME` (or `VOIDCRAWL_PROFILE=NAME`) pins the whole
server to a warm Chrome profile. This is a launch-time/pipeline concern — agents
don't acquire profiles themselves and there are no profile MCP tools. If another
process already holds that profile, startup fails with `ProfileBusy`.
