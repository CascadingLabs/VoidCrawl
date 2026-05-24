# void_crawl — Rust CDP Browser Bindings

## What this is
A Rust workspace providing CDP (Chrome DevTools Protocol) browser automation, exposed to Python via PyO3. Replaces zendriver/Playwright with a permissively-licensed (MIT/Apache-2.0) stack.

## Using voidcrawl to scrape (drop-in for Playwright / Chromium MCP)
For any task that needs to browse, scrape, or drive a web page — stealthily,
in parallel, or against bot-walled sites — use the **`voidcrawl` MCP server**
instead of Playwright/Chromium MCP. The full usage guide (tool map, the
perceive→act→extract decision ladder, captcha contract, fan-out pattern) lives
in `.claude/skills/voidcrawl/SKILL.md`. Hosts with a skills mechanism
(Claude Code, opencode) load it automatically; Codex should read that file
directly before a browser task. One-time setup + per-host wiring:
`.claude/skills/voidcrawl/SETUP.md`.

## Architecture
```
void_crawl/
├── crates/
│   ├── core/              # Pure Rust async CDP wrapper (chromiumoxide)
│   │   └── src/
│   │       ├── lib.rs     # Re-exports
│   │       ├── session.rs # BrowserSession — launch/connect/close
│   │       ├── page.rs    # Page — navigate, content, JS eval, screenshot, DOM queries
│   │       ├── stealth.rs # StealthConfig — anti-detection patches
│   │       └── error.rs   # VoidCrawlError enum
│   └── pyo3_bindings/     # PyO3 extension module (cdylib)
│       └── src/lib.rs     # PyBrowserSession + PyPage pyclass structs
├── pyproject.toml         # maturin build config
└── build.sh               # Quick build: maturin develop --release
```

## Key commands
- `cargo check` — type-check the Rust code
- `cargo test -p void_crawl_core -- --test-threads=1` — run Rust integration tests (serial, requires Chromium)
- `./build.sh` — build and install the Python extension into the current venv
- `uv run pytest tests/ -v` — Python integration tests

## Python type stubs
- Type stubs at `void_crawl.pyi`
- See `examples/` for usage patterns

## Dependencies (all MIT/Apache-2.0)
- `chromiumoxide` 0.9 — CDP client
- `tokio` — async runtime
- `pyo3` 0.28 + `pyo3-async-runtimes` 0.28 — Python bridge
- `thiserror` — error types
- `reqwest` — HTTP (for WebSocket URL resolution)
- Rust edition 2024 / MSRV 1.86

## Pool architecture
- Chrome is a long-lived daemon — never launch per-request
- Pool lives in Rust (crates/core/src/pool.rs), not Python
- Semaphore lives in Rust (tokio::sync::Semaphore)
- Tab recycling: navigate to about:blank, never close+reopen
- Hard recycle after TAB_MAX_USES (default 50)
- Idle eviction after TAB_MAX_IDLE_SECS (default 60)

## PyO3 rules
- Never use std::sync::Mutex — always tokio::sync::Mutex
- Never acquire GIL inside a tokio::spawn — deadlock risk
- Python::with_gil() only for constructing return values (bytes, etc.)
- All pool operations must cross the PyO3 boundary exactly once per acquire/release

## Python targets
- Primary dev: 3.11
- Support range: 3.10–3.13
- Do NOT add 3.14 specific APIs
- Never use time.sleep() — always asyncio.sleep() or tenacity
- Never use unittest — always pytest + pytest-asyncio
