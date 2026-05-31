# voidcrawl-mcp

Rust-native MCP server that exposes [`void_crawl_core`](../core)'s
stealth-patched Chrome pool to Claude Code. Headless by default,
concurrent by construction — ten tabs in parallel is the norm.

## Why not claude-in-chrome?

- `claude-in-chrome` drives the user's single visible Chrome tab.
- `voidcrawl-mcp` drives a headless pool with real concurrency and
  per-session isolation. Pick it for bulk scraping, bot-detection
  targets, and subagent fan-out. See [`../../.claude/skills/voidcrawl/SKILL.md`](../../.claude/skills/voidcrawl/SKILL.md) for the full decision rubric.

## Tools

| Tool              | Kind      | Summary                                                     |
| ----------------- | --------- | ----------------------------------------------------------- |
| `fetch`           | stateless | Single URL → `{ html, title, status_code, ... }`.           |
| `fetch_many`      | stateless | N URLs in parallel; pool semaphore caps concurrency.        |
| `screenshot`      | stateless | Full-page PNG as `image/png` content.                       |
| `session_open`    | stateful  | Launches a dedicated Chrome, returns `session_id`.          |
| `session_navigate`| stateful  | Goto URL within a session and wait for settle.              |
| `session_content` | stateful  | HTML + title + URL of the session's current page.           |
| `session_close`   | stateful  | Shut down the session's Chrome.                             |
| `pool_status`     | diag      | Pool config + live session count.                           |
| `download`        | stateless | Download a file by URL through stealth Chrome and scan it with the built-in antivirus gate. **Opt-in** (`VOIDCRAWL_ALLOW_DOWNLOADS=1`). |
| `download_arm` / `download_wait` | stateful | Capture a download started by a page action (button with no stable URL), then scan it. **Opt-in** (`VOIDCRAWL_ALLOW_DOWNLOADS=1`). |

This is an abbreviated list; the perceive/act tools (`click`, `click_by_role`,
`type_text`, `eval_js`, `extract`, `session_ax_tree`, `detect_captcha`, …) are
documented in the published MCP and File Downloads guides.

Each tool accepts a `wait_for` string: `"networkidle"` (default),
`"selector:<css>"`, or `"ms:<n>"`.

## Install

### Repo-local (dev)

The repo ships an [`../../.mcp.json`](../../.mcp.json) at the root that
runs the server via `cargo run`. Claude Code will pick it up
automatically when launched from the repo.

### Global

```
cargo install --path .
claude mcp add voidcrawl voidcrawl-mcp -e BROWSER_COUNT=1 -e TABS_PER_BROWSER=5
```

## Configuration (environment)

All knobs are read by `BrowserPool::from_env()` in `void_crawl_core`:

| Env var                 | Default | Meaning                                                |
| ----------------------- | ------- | ------------------------------------------------------ |
| `BROWSER_COUNT`         | 1       | Number of Chrome processes to launch.                  |
| `TABS_PER_BROWSER`      | 4       | Tabs per Chrome. `max_tabs = BROWSER_COUNT * this`.    |
| `TAB_MAX_USES`          | 50      | Hard-recycle a tab after this many `acquire`s.         |
| `TAB_MAX_IDLE_SECS`     | 60      | Evict a tab after this many seconds idle.              |
| `ACQUIRE_TIMEOUT_SECS`  | 30      | Max wait for a free tab. `0` = infinite.               |
| `CHROME_HEADLESS`       | 1       | `1` headless, `0` headful.                             |
| `CHROME_NO_SANDBOX`     | 0       | `1` to pass `--no-sandbox` (Docker/CI).                |
| `CHROME_WS_URLS`        | —       | Comma-separated CDP WS endpoints to connect to instead of launching. |
| `VIEWPORT_WIDTH`        | 1920    | Stealth viewport width.                                |
| `VIEWPORT_HEIGHT`       | 1080    | Stealth viewport height.                               |
| `CDP_PORT_BASE`         | —       | Pin Chrome's `--remote-debugging-port` for launched browsers (browser *i* gets `base + i`). Unset = OS picks a free ephemeral port, which can't conflict. Set when a firewall only exposes specific ports. |

One capability knob is read separately from `from_env()`:

| Env var                    | Default | Meaning                                                |
| -------------------------- | ------- | ------------------------------------------------------ |
| `VOIDCRAWL_ALLOW_DOWNLOADS` | unset (off) | `1` exposes the `download` / `download_arm` / `download_wait` tools. Off by default: downloads pull untrusted bytes to disk over a live auth session, so they're opt-in. |

Logging goes to stderr (`RUST_LOG=voidcrawl_mcp=debug` for more). stdout
is reserved for the MCP protocol.

## Protocol

Transport: **stdio**. Protocol version: whatever `rmcp` 1.4 defaults to.
