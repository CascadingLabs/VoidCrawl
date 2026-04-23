## 0.3.1 (2026-04-23)

### Feat

- `voidcrawl-mcp` now ships as a standalone PyPI + crates.io package at the same version as `voidcrawl`. Install via `uv tool install voidcrawl-mcp`, `pipx install voidcrawl-mcp`, or `cargo install voidcrawl-mcp`. Want the full bundle? `pip install 'voidcrawl[mcp]'`.
- Rust crate renamed on crates.io from `voidcrawl_mcp` to `voidcrawl-mcp` (matches the binary name). `use voidcrawl_mcp::…` Rust paths still resolve — cargo maps dashes to underscores for library names automatically.
- `void_crawl_core` published to crates.io so `cargo install voidcrawl-mcp` can resolve dependencies.

### Breaking

- The compiled `voidcrawl-mcp` binary is no longer bundled inside the `voidcrawl` Python wheel. If you were relying on `pip install voidcrawl` putting the binary on PATH, switch to `pip install 'voidcrawl[mcp]'` or install `voidcrawl-mcp` separately.
- `voidcrawl._mcp_launcher` has been removed.

## 0.3.0 (2026-04-22)

### Feat

- native Chrome profile leasing: `list_profiles`, `acquire_profile`, `with_profile` (Python async context manager) with cross-process advisory locking via `.voidcrawl.lock`
- typed exceptions: `ProfileBusy`, `ProfileLeaseExpired`, `ProfileNotFound`, `CaptchaDetected` (all subclassing `VoidCrawlError`)
- DOM-only captcha detection: `Page.detect_captcha()` / `detect_captcha` MCP tool — recaptcha, hcaptcha, Turnstile, Cloudflare interstitial, DataDome
- `Page.screenshot(path=..., bbox=...)` — optionally writes PNG to disk and/or crops to a region. Existing `Page.screenshot_png()` remains for backward compatibility.
- MCP server profile pinning: `voidcrawl-mcp --profile NAME` / `VOIDCRAWL_PROFILE` env var. Profile management is not exposed to MCP clients.
- new MCP tools: `click`, `click_visual_coords`, `type_text`, `eval_js`, `title`, `extract`, `wait_for_network_idle`, `network_capture`, `detect_captcha`
- MCP errors carry `data.exception` for typed dispatch (`CaptchaDetected`, `ProfileBusy`, etc.)
- updated Claude Code skill at `.claude/skills/voidcrawl/SKILL.md` with captcha/visual-click/profile guidance

### Docs

- `docs/profiles.md`, `docs/captcha-detection.md`, `docs/mcp-server.md`, `docs/experiments/profile-isolation.md`

### Breaking

- none. The API is strictly additive.

## 0.1.9 (2026-04-04)

### Fix

- include LICENSE.md in sdist to satisfy PyPI License-File validation

## v0.1.0 (2026-04-04)

### Feat

- added builtin and extensible actions system for VC

### Refactor

- improved API design w/ pydantic config objects
