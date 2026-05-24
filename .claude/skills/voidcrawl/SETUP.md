# voidcrawl skill — setup (one-time)

Goal: a stealth headless-scraping skill that drops in as a replacement for the
Playwright MCP / Chromium MCP server in Claude Code, opencode, and Codex.

## 1. Install the server binary (once)

Put `voidcrawl-mcp` on your `PATH`. Pick whichever matches your toolchain — both
pull published releases, so the skill is drop-in on any machine:

```bash
# Prebuilt binary, no Rust toolchain needed (recommended):
uvx voidcrawl-mcp --help          # run on demand, or:
pipx install voidcrawl-mcp        # install to PATH

# Or build from source (needs a Rust toolchain):
cargo install voidcrawl-mcp       # → ~/.cargo/bin (must be on PATH)
```

Requires Chrome/Chromium installed. Verify:

```bash
voidcrawl-mcp --help   # or: which voidcrawl-mcp
```

(From a clone instead of a release: `cargo install --path crates/mcp_server`,
or after `./build.sh` use the absolute `target/release/voidcrawl-mcp` path in
the configs below.)

## 2. Wire it into your host

All three hosts launch the same `voidcrawl-mcp` binary. The usage guide
(`SKILL.md`) is loaded automatically by Claude Code and opencode (both read
`.claude/skills/`); Codex has no skills mechanism, so it's pointed to from
`AGENTS.md`.

There are two ways to wire each host up: a one-liner via the host's own
`mcp add` CLI (**Option A**, fastest), or editing the host's config file
directly (**Option B**, needed for committed project scope, or when the host's
CLI isn't on PATH). A future `voidcrawl-mcp install` wrapper (§4) will do both
for all three hosts at once.

### Option A — the host's native `mcp add` CLI

```bash
# Claude Code — honors scope (user = every repo; project = committed .mcp.json).
claude mcp add --scope user voidcrawl \
  --env BROWSER_COUNT=1 --env TABS_PER_BROWSER=5 --env CHROME_HEADLESS=1 \
  -- voidcrawl-mcp

# Codex — writes ~/.codex/config.toml (GLOBAL only; the CLI has no scope flag).
codex mcp add voidcrawl \
  --env BROWSER_COUNT=1 --env TABS_PER_BROWSER=5 --env CHROME_HEADLESS=1 \
  -- voidcrawl-mcp

# opencode — INTERACTIVE prompt (no name/command/env flags); follow the
# prompts (name: voidcrawl, command: voidcrawl-mcp), or use Option B to script it.
opencode mcp add
```

Caveats: Codex's CLI can only write the **global** config — for a committed,
project-scoped Codex server use Option B. opencode's `mcp add` is interactive
and can't be scripted. Verify with `claude mcp list` / `codex mcp list` /
`opencode mcp list`.

### Option B — edit the config file directly (committed / project scope)

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
target repo, then wire the host — run the §2 Option A `mcp add` one-liners
(or, for committed config, copy the §2 Option B blocks). opencode/Claude will
pick up the skill from `.claude/skills/`; for Codex add the `AGENTS.md` pointer.

## 4. One-shot install — `voidcrawl-mcp install`

The alternative to wiring each host by hand (§2): a single self-install
subcommand on the binary itself.

```bash
voidcrawl-mcp install [--scope user|project] [--tool claude|codex|opencode]
voidcrawl-mcp uninstall [...same flags]
voidcrawl-mcp install --status      # report where it's wired
voidcrawl-mcp install --dry-run     # show resulting config/command, write nothing
voidcrawl-mcp install --help
```

Default scope is `user`; with no `--tool` it targets all three hosts. It is a
hybrid that reuses each host's own machinery where it can, and falls back to
the config file (or a paste-this block) where it can't:

- **Claude** → shells out to `claude mcp add --scope <user|project>`. If the
  `claude` CLI isn't on PATH, it prints the `.mcp.json` / `~/.claude.json` block.
- **Codex** → shells out to `codex mcp add` (user scope). Its CLI has no scope
  flag, so for `--scope project` it prints the `.codex/config.toml` block to
  paste instead (likewise if the `codex` CLI is missing).
- **opencode** → merges `opencode.json` directly (its `mcp add` is interactive,
  not scriptable), backing up the prior file to `.bak`. If `opencode` isn't on
  PATH it prints the block.

It resolves its own absolute path via `env::current_exe()` and writes that as
the launch command for every host, so the wiring never depends on PATH. JSON
merges are idempotent — re-running updates in place and preserves other servers.

## Tuning knobs (env)

- `BROWSER_COUNT` × `TABS_PER_BROWSER` = max parallel tabs. Raise both for
  bigger `fetch_many` fan-outs (watch RAM — each browser is ~hundreds of MB).
- `CHROME_HEADLESS=0` to watch a run; `CDP_PORT_BASE=<n>` to pin debug ports.
- `VOIDCRAWL_PROFILE=NAME` (or `--profile NAME`) to pin a warm logged-in profile.
