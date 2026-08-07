# voidcrawl-mcp

Stdio MCP server exposing `voidcrawl` to Claude Code and other MCP-speaking agents.

## Install

```bash
# Prebuilt binary wheel (recommended for Claude Code users):
uv tool install voidcrawl-mcp
# or: pipx install voidcrawl-mcp

# Python lib + MCP server together:
pip install 'voidcrawl[mcp]'

# From source via cargo:
cargo install voidcrawl-mcp
# or directly from git HEAD:
cargo install --git https://github.com/CascadingLabs/VoidCrawl voidcrawl-mcp

# Dev / repo clone:
cargo build --release -p voidcrawl-mcp
# binary at ./target/release/voidcrawl-mcp
```

Wire into Claude Code via `.mcp.json`:

```json
{
  "mcpServers": {
    "voidcrawl": {
      "command": "voidcrawl-mcp"
    }
  }
}
```

## Profiles

There are two profile paths:

- **Pinned native Chrome profile**: launch with `--profile NAME` or `VOIDCRAWL_PROFILE=NAME` to bind the whole server to one existing Chrome profile at startup.
- **VoidCrawl-managed profiles**: create standalone profiles under `VOIDCRAWL_PROFILE_ROOT` and lease one per `session_open` with `profile_id` or `profile_pool`.

```bash
voidcrawl-mcp --profile "Default"
```

Native profile discovery and leasing stay outside the MCP tool surface. MCP clients cannot enumerate your daily Chrome profiles. Managed profile tools expose only VoidCrawl-owned metadata under the managed root; cookies, local storage, and saved passwords are never returned.

## Tools

### Stateless

| Tool | Purpose |
|---|---|
| `fetch` | One URL → `{ url, status_code, redirected, html, title, extracted? }`. |
| `fetch_snapshot` | One URL → compact rendered-page snapshot with headings, text blocks, links, controls, forms, metadata, and truncation stats. |
| `fetch_many` | Parallel fan-out over the pool. |
| `screenshot` | Load URL, return PNG. |
| `record` | Load URL, record it, write frames/video to disk and return paths. |
| `pool_status` | Current pool config + open session count. |

### Stateful sessions

Open a session → navigate → operate → close. Each session is a dedicated Chrome instance with its own profile.

| Tool | Purpose |
|---|---|
| `session_open` | Launch dedicated Chrome. Returns `session_id`. Optional `profile_id`, `profile_pool`, or `user_data_dir` selects persistent state. |
| `session_navigate` | Navigate session to URL, wait for settle. |
| `session_content` | Return HTML, title, URL. |
| `session_record_start` / `session_record_stop` | Record the session while you drive it; writes frames/video to disk. |
| `session_snapshot` | Return a compact rendered-page snapshot of the current session page. |
| `session_screenshot` | Return a PNG of the current session page exactly as it stands — no navigation, no URL change. |
| `session_close` | Tear down. |

Prefer `fetch_snapshot` for first-pass inspection of large pages, and
`session_snapshot` after clicking, pagination, login, or other stateful flows.
Use `session_ax_tree` for role/name targeting before `click_by_role`. Use
`fetch` and `session_content` only when the caller truly needs raw HTML.

**Choosing a perception tool for a stateful session:**
- `session_ax_tree` — default. Cheapest, semantic, best for locating clickable
  targets by role/name. Check `named_count` vs `node_count`; a low ratio means
  a thin accessibility tree.
- `session_snapshot` — structured headings/text/links/controls/forms when you
  need more page content than the AX tree surfaces, without the cost of raw HTML.
- `session_screenshot` — reach for this when you need *pixels*: layout, visual
  state, a challenge/CAPTCHA rendering, or a thin AX tree/snapshot that doesn't
  explain what's on screen. It's the visual fallback before `click_visual_coords`
  and the only one of the three that can show authenticated, post-click,
  paginated, or challenge state without disturbing it — `screenshot` (stateless)
  always navigates first, so it can't see state that only exists inside an
  already-open session. Unknown or closed `session_id`s fail explicitly with
  `invalid_params`.

#### Recording

`record` and `session_record_start` / `session_record_stop` capture a page as a
sequence of frames rather than a single image. Unlike the screenshot tools they
never return frames inline — a recording is hundreds of images, which would
swamp an agent's context — so frames and any encoded artifact are written to
disk and the response carries the output directory, per-region paths, and
counts.

Two behaviors to expect: `fps` is a **ceiling**, not a floor (Chrome emits
frames when it paints, so a static page yields very few — check
`effective_fps`), and `selectors` is a **list**, producing one cropped region
per selector from a single recording. Recordings are capped at 120s, and a
session recording holds that browser's capture lock, so screenshots on its
sibling tabs wait until it stops. See [recording.md](recording.md) for the
full model, including how to record concurrently.

### Managed profiles

| Tool | Purpose |
|---|---|
| `profile_list` / `profile_describe` | Inspect VoidCrawl-managed profile metadata. |
| `profile_create` | Create a standalone managed profile under the registry root. |
| `profile_clone` | Clone a managed profile id or explicit `user_data_dir` path into a new managed profile. |
| `profile_delete` | Delete an unlocked managed profile. |
| `profile_pool_list` / `profile_pool_describe` | Inspect named ordered profile pools. |
| `profile_pool_create` | Create or replace a round-robin pool used by `session_open.profile_pool`. |

Set `VOIDCRAWL_PROFILE_ROOT` to pin the registry location. The default is the platform data directory, for example `~/.local/share/voidcrawl/profiles` on Linux. These are standalone Chrome `user_data_dir` roots, not subprofiles inside your daily Chrome directory.

### Interaction primitives

All take `session_id` plus tool-specific args.

| Tool | Purpose |
|---|---|
| `click` | Click a CSS selector. |
| `click_visual_coords` | Click pixel coords (for React forms that ignore `dispatchEvent`-style clicks). |
| `type_text` | Type into a selector, or into the focused element. |
| `eval_js` | Evaluate an expression, return JSON. |
| `title` | Read `<title>`. |
| `extract` | `querySelectorAll(selector).map(textContent)`. |
| `session_ax_tree` | Compact or raw accessibility tree for role/name inspection. |
| `wait_for_network_idle` | Event-driven wait. |
| `network_capture` | Resource Timing entries (url, initiator type, transfer size, duration). No headers or bodies. |
| `network_capture_arm` | Arm real CDP `Network.*` capture for named URL globs before the triggering action. Returns request headers, response headers, status, opt-in body. Credential header values are `<redacted>` unless `include_sensitive_headers` **and** `VOIDCRAWL_ALLOW_CREDENTIAL_CAPTURE=1`. |
| `network_capture_wait` | Wait for the armed patterns and return the captures. `timeout_secs` is measured from this call, not from `arm`. |
| `cookie_lease_open` | Fork the cookies reachable by one replay origin into a revocable, in-memory lease; returns the lease id plus **value-free** provenance per cookie (incl. `top_level_site`, the CHIPS partition key). Values never cross the wire. Scope-bound; dies with its session. Reports facts, does not classify replay eligibility. |
| `cookie_lease_revoke` | Revoke a lease, scrubbing held values; fails later use closed with a recorded reason. Idempotent. |
| `session_cookies` | Every cookie CDP can see, including HttpOnly/Secure, with **raw values**. OPT-IN: needs `VOIDCRAWL_ALLOW_CREDENTIAL_CAPTURE=1`. Prefer `cookie_lease_open` unless a human needs the literal value. |
| `detect_captcha` | DOM probe → `recaptcha` / `hcaptcha` / `turnstile` / `cloudflare_challenge` / `datadome` / `null`. |
| `capture_challenge` | Capture an active challenge event with anti-bot evidence, DOM captcha info, same-tab CDP attach coordinates, and VNC/noVNC links. |
| `mark_challenge_resolved` | Mark a challenge cleared by `manual_vnc` or a future resolver. |
| `mark_challenge_failed` | Mark a challenge failed so the caller can rotate identity or stop with evidence. |
| `wait_for_challenge_resolution` | Wait for resolution/failure and optionally re-probe the DOM before resuming. |

For the manual operator loop, see [Challenge Escalation With VNC and noVNC](challenge-escalation.md).

## Typed errors

Errors carry `data.exception` for machine dispatch:

- `CaptchaDetected` → `data: { exception, kind }`
- `ProfileBusy` → `data: { exception, name }`
- `ProfileLeaseExpired` → `data: { exception, name, timeout_secs }`
- `ProfileNotFound` → `data: { exception, name, searched }`

Other errors surface as plain `invalid_params` (bad selector/URL/JS) or `internal_error` (timeout, browser crash).

## HiDPI note

`click_visual_coords` takes CSS pixels. On HiDPI displays the screenshot dimensions are `devicePixelRatio × CSS pixels`. Divide by the DPR before passing coords:

```javascript
eval_js("window.devicePixelRatio")  // -> 2.0 on many Macs
```
