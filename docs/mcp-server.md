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
| `pool_status` | Current pool config + open session count. |

### Stateful sessions

Open a session → navigate → operate → close. Each session is a dedicated Chrome instance with its own profile.

| Tool | Purpose |
|---|---|
| `session_open` | Launch dedicated Chrome. Returns `session_id`. Optional `profile_id`, `profile_pool`, or `user_data_dir` selects persistent state. |
| `session_navigate` | Navigate session to URL, wait for settle. |
| `session_content` | Return HTML, title, URL. |
| `session_snapshot` | Return a compact rendered-page snapshot of the current session page. |
| `session_close` | Tear down. |

Prefer `fetch_snapshot` for first-pass inspection of large pages, and
`session_snapshot` after clicking, pagination, login, or other stateful flows.
Use `session_ax_tree` for role/name targeting before `click_by_role`. Use
`fetch` and `session_content` only when the caller truly needs raw HTML.

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
| `network_capture` | Resource Timing entries (url, initiator type, transfer size, duration). |
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
