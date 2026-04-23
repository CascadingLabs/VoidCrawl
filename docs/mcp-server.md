# voidcrawl-mcp

Stdio MCP server exposing `voidcrawl` to Claude Code and other MCP-speaking agents.

## Install

If you cloned the repo:

```bash
cargo build --release -p voidcrawl_mcp
# binary at ./target/release/voidcrawl-mcp
```

Wire into Claude Code via `.mcp.json`:

```json
{
  "mcpServers": {
    "voidcrawl": {
      "command": "/path/to/target/release/voidcrawl-mcp"
    }
  }
}
```

## Pinning a profile

Launch with `--profile NAME` (or `VOIDCRAWL_PROFILE=NAME`) to bind the whole server to a warm Chrome profile at startup:

```bash
voidcrawl-mcp --profile "Default"
```

Profile management is **not** exposed to MCP clients. Agents can't list or acquire profiles — this is a pipeline-level concern. If a second `voidcrawl-mcp` tries to pin the same profile, startup fails with `ProfileBusy`.

## Tools

### Stateless

| Tool | Purpose |
|---|---|
| `fetch` | One URL → `{ url, status_code, redirected, html, title, extracted? }`. |
| `fetch_many` | Parallel fan-out over the pool. |
| `screenshot` | Load URL, return PNG. |
| `pool_status` | Current pool config + open session count. |

### Stateful sessions

Open a session → navigate → operate → close. Each session is a dedicated Chrome instance with its own profile.

| Tool | Purpose |
|---|---|
| `session_open` | Launch dedicated Chrome. Returns `session_id`. |
| `session_navigate` | Navigate session to URL, wait for settle. |
| `session_content` | Return HTML, title, URL. |
| `session_close` | Tear down. |

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
| `wait_for_network_idle` | Event-driven wait. |
| `network_capture` | Resource Timing entries (url, initiator type, transfer size, duration). |
| `detect_captcha` | DOM probe → `recaptcha` / `hcaptcha` / `turnstile` / `cloudflare_challenge` / `datadome` / `null`. |

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
