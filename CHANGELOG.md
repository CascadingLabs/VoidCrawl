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
