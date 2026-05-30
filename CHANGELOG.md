## Unreleased

### Feat

- file downloads with a built-in antivirus gate (CAS-86). New `download` MCP tool and `Page::download_to_dir` fetch a file through the stealth browser context (cookies/TLS fingerprint preserved), into a quarantine directory. A new `scanner` module (`scan_path` / `scan_bytes`) gates it: size cap, magic-byte type check (`infer`) to catch executables disguised as documents (the server's claimed `Content-Type` is fed into the check), and a `yara-x` signature scan (ships an EICAR rule). The file is moved into the output directory only if it comes back clean; a flagged file is deleted. Downloads are forced from inside the page (same-origin `fetch` → blob → download anchor) so `Content-Disposition: inline` resources like PDFs download instead of rendering in Chrome's viewer; the stream aborts past the size cap so a hostile server can't exhaust the tab, and the CDP download behavior is reset before the pooled tab is recycled. **Opt-in**: the tool is disabled unless the server runs with `VOIDCRAWL_ALLOW_DOWNLOADS=1`. A `clean` verdict means the file passed the size + content-type + bundled-signature checks, not that it is guaranteed malware-free (real signature-DB scanning via `clamd` is a planned opt-in). See `examples/download_and_scan.rs`.

## 0.3.2 (2026-05-24)

Packaging-only re-release of 0.3.1 — no source changes. The 0.3.1 release failed: every `Build MCP` job errored, yet the crates.io publish still ran, leaving void_crawl_core/voidcrawl-mcp 0.3.1 on crates.io while neither reached PyPI.

### Fix

- release CI: the `voidcrawl-mcp` wheel and sdist jobs passed both `working-directory: crates/mcp_server` and `--manifest-path crates/mcp_server/Cargo.toml`. maturin-action joins the two, so it looked for the manifest at `crates/mcp_server/crates/mcp_server/…` and every `Build MCP` job failed. The manifest path is now relative to the working directory (`Cargo.toml`).
- release CI: the MCP sdist's `[tool.maturin] include` reached the workspace LICENSE via `../../LICENSE.md`; maturin rejects `..` in include patterns. The crate now vendors its own `LICENSE.md`.
- release CI: `publish-crates-io` only depended on the Rust/Python checks, so a failed build still published to the immutable crates.io registry (as happened in 0.3.1). It now waits on the full wheel + sdist build matrix, matching the PyPI publish jobs.

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
