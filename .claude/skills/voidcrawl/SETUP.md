# voidcrawl skill — setup (one-time)

Goal: a stealth headless-scraping skill that drops in as a replacement for the
Playwright MCP / Chromium MCP server in Claude Code, opencode, and Codex.

## 1. Install the server binary (once)

Put `voidcrawl-mcp` on your `PATH`:

```bash
cargo install --path crates/mcp_server
# → installs `voidcrawl-mcp` to ~/.cargo/bin (must be on PATH)
```

Requires Chrome/Chromium installed. Verify:

```bash
voidcrawl-mcp --help   # or: which voidcrawl-mcp
```

(Alternatively, after `./build.sh` the binary is at
`target/release/voidcrawl-mcp` — use that absolute path in the configs below
if you'd rather not `cargo install`.)

## 2. Wire it into your host

All three hosts launch the same `voidcrawl-mcp` binary. The usage guide
(`SKILL.md`) is loaded automatically by Claude Code and opencode (both read
`.claude/skills/`); Codex has no skills mechanism, so it's pointed to from
`AGENTS.md`.

**Claude Code** — `.mcp.json` (already in this repo):
```json
{ "mcpServers": { "voidcrawl": {
  "command": "voidcrawl-mcp", "args": [],
  "env": { "BROWSER_COUNT": "1", "TABS_PER_BROWSER": "5", "CHROME_HEADLESS": "1" }
} } }
```

**opencode** — `opencode.json` (already in this repo):
```json
{ "mcp": { "voidcrawl": {
  "type": "local", "command": ["voidcrawl-mcp"], "enabled": true,
  "environment": { "BROWSER_COUNT": "1", "TABS_PER_BROWSER": "5", "CHROME_HEADLESS": "1" }
} } }
```

**Codex** — `.codex/config.toml` (project-scoped, already in this repo; or add
the same block to `~/.codex/config.toml` to enable it everywhere):
```toml
[mcp_servers.voidcrawl]
command = "voidcrawl-mcp"
args = []
[mcp_servers.voidcrawl.env]
BROWSER_COUNT = "1"
TABS_PER_BROWSER = "5"
CHROME_HEADLESS = "1"
```

## 3. Use it in another repo (e.g. Yosoi)

The binary is global once installed, so only the per-host config + the skill
file travel. Copy `.claude/skills/voidcrawl/` (SKILL.md + this file) into the
target repo and add the host config block(s) above. opencode/Claude will pick
up the skill from `.claude/skills/`; for Codex add the `AGENTS.md` pointer.

## Tuning knobs (env)

- `BROWSER_COUNT` × `TABS_PER_BROWSER` = max parallel tabs. Raise both for
  bigger `fetch_many` fan-outs (watch RAM — each browser is ~hundreds of MB).
- `CHROME_HEADLESS=0` to watch a run; `CDP_PORT_BASE=<n>` to pin debug ports.
- `VOIDCRAWL_PROFILE=NAME` (or `--profile NAME`) to pin a warm logged-in profile.
