<p align="center">
  <a href="https://cascadinglabs.com/voidcrawl/">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="media/logo-dark.svg">
      <source media="(prefers-color-scheme: light)" srcset="media/logo-light.svg">
      <img src="media/logo-dark.svg" alt="VoidCrawl" width="200">
    </picture>
  </a>
</p>

<p align="center">
  <a href="https://discord.gg/ftykDhmAQN"><img src="https://img.shields.io/badge/Discord-Join-b07adf?labelColor=120a24&logo=discord&logoColor=white" alt="Discord"></a>
  <a href="https://opensource.org/licenses/Apache-2.0"><img src="https://img.shields.io/badge/License-Apache_2.0-b07adf?labelColor=120a24" alt="License"></a>
  <a href="https://github.com/CascadingLabs/VoidCrawl/actions"><img src="https://img.shields.io/github/actions/workflow/status/CascadingLabs/VoidCrawl/CI.yaml?label=CI&labelColor=120a24&color=b07adf" alt="CI"></a>
  <a href="https://pypi.python.org/pypi/void-crawl"><img src="https://img.shields.io/pypi/v/void-crawl?labelColor=120a24&color=b07adf" alt="PyPI"></a>
  <a href="https://pypi.python.org/pypi/void-crawl"><img src="https://img.shields.io/pypi/pyversions/void-crawl?labelColor=120a24&color=b07adf" alt="Python versions"></a>
  <a href="https://cascadinglabs.com/voidcrawl/"><img src="https://img.shields.io/badge/docs-cascadinglabs.com%2Fvoidcrawl-b07adf?labelColor=120a24" alt="docs"></a>
</p>

> [!WARNING]
> **VoidCrawl is currently in Alpha.** The API is expected to change significantly. We do not expect a stable API until we are out of Beta.

# VoidCrawl

**CDP browser automation for Python** — a Rust-native Chrome DevTools Protocol client exposed to Python via PyO3.

`void_crawl` replaces Playwright/Selenium with a permissively-licensed (Apache-2.0) stack for rendering JavaScript-heavy pages. Built on [chromiumoxide](https://github.com/mattsse/chromiumoxide) with a shared Tokio runtime.

> **Used by [Yosoi](https://github.com/CascadingLabs/Yosoi)** — an AI-powered selector discovery tool for resilient web scraping.

## Architecture

```
void_crawl/
├── crates/
│   ├── core/              # Pure Rust async CDP wrapper (chromiumoxide 0.9)
│   │   └── src/
│   │       ├── session.rs # BrowserSession — launch / connect / close
│   │       ├── page.rs    # Page — navigate, content, JS eval, screenshot, DOM
│   │       ├── pool.rs    # BrowserPool — tab reuse with semaphore + eviction
│   │       ├── stealth.rs # StealthConfig — anti-detection patches
│   │       └── error.rs   # YosoiError enum
│   └── pyo3_bindings/     # PyO3 0.28 extension module (cdylib)
│       └── src/lib.rs     # BrowserPool, BrowserSession, Page → Python classes
├── pyproject.toml         # maturin build config
├── build.sh               # Quick build: maturin develop --release
└── void_crawl.pyi         # Python type stubs
```

### How it works

1. **Rust core** (`void_crawl_core`) wraps [chromiumoxide](https://github.com/mattsse/chromiumoxide) into a clean async API: `BrowserPool` manages a pool of reusable tabs, `BrowserSession` manages individual browser lifecycle, `Page` wraps tabs with navigation, JS evaluation, screenshots, and DOM queries.

2. **PyO3 bindings** bridge Rust async → Python asyncio via [`pyo3-async-runtimes`](https://github.com/PyO3/pyo3-async-runtimes). A shared Tokio runtime handles all CDP I/O; `future_into_py` converts each Rust future into a Python awaitable.

### Anti-detection (Stealth)

`StealthConfig` applies multiple layers to avoid bot detection:

| Layer | What it does |
|---|---|
| chromiumoxide `enable_stealth_mode()` | Patches `navigator.webdriver`, `navigator.plugins`, Chrome runtime checks |
| `Page.addScriptToEvaluateOnNewDocument` | Custom JS injection before every page load |
| `Emulation.setUserAgentOverride` | Realistic UA + platform + Accept-Language |
| `Emulation.setDeviceMetricsOverride` | 1920×1080 viewport, device scale factor 1.0 |
| `Page.setBypassCSP` | Allows injected JS to run despite CSP |
| Chrome flags | `--disable-blink-features=AutomationControlled`, `--disable-infobars` |

Use `StealthConfig::chrome_like()` (default) for a realistic Chrome fingerprint, or `StealthConfig::none()` for raw headless.

## Requirements

- **Rust** ≥ 1.86 (edition 2024)
- **Python** ≥ 3.10
- **Chrome/Chromium** installed on the system
- **maturin** ≥ 1.7 (`cargo install maturin`)

## Quick start

```bash
# Build and install into your venv
./build.sh

# Or manually:
maturin develop --release --manifest-path crates/pyo3_bindings/Cargo.toml
```

### Python — BrowserPool (recommended)

```python
import asyncio
from void_crawl import BrowserPool

async def main():
    async with BrowserPool.from_env() as pool:
        # Tabs are recycled, not closed — near-instant reuse
        async with pool.acquire() as tab:
            await tab.navigate("https://example.com")
            print(await tab.title())
            print(len(await tab.content()))

asyncio.run(main())
```

### Python — Parallel fetch

```python
import asyncio
from void_crawl import BrowserPool

async def main():
    async with BrowserPool.from_env() as pool:
        async def fetch(url: str) -> str:
            async with pool.acquire() as tab:
                await tab.navigate(url)
                return await tab.content()

        urls = ["https://example.com"] * 4
        results = await asyncio.gather(*[fetch(u) for u in urls])
        for html in results:
            print(f"  {len(html)} chars")

asyncio.run(main())
```

### Python — BrowserSession (low-level)

```python
import asyncio
from void_crawl import BrowserSession

async def main():
    async with BrowserSession(headless=True) as browser:
        page = await browser.new_page("https://example.com")
        print(await page.title())
        await page.close()

asyncio.run(main())
```

### Rust usage

```rust
use void_crawl_core::{BrowserPool, PoolConfig, BrowserSession};

#[tokio::main]
async fn main() -> void_crawl_core::Result<()> {
    // Pool-based (recommended)
    let pool = BrowserPool::from_env().await?;
    pool.warmup().await?;

    let tab = pool.acquire().await?;
    tab.page.navigate("https://example.com").await?;
    println!("{}", tab.page.content().await?);
    pool.release(tab).await?;
    pool.close().await?;

    // Or low-level session
    let session = BrowserSession::launch_headless().await?;
    let page = session.new_page("https://example.com").await?;
    println!("{}", page.content().await?);
    page.close().await?;
    session.close().await?;
    Ok(())
}
```

### Docker (headless)

```bash
docker compose -f docker/docker-compose.yml up -d

# Pool auto-connects to Chrome via CHROME_WS_URLS
export CHROME_WS_URLS="http://localhost:9222,http://localhost:9223"
python examples/basic_navigation.py
```

### Docker headful (GPU + VNC)

Run Chrome with a real GUI inside Docker. Uses Sway as a Wayland compositor
and wayvnc to stream the display. Your GPU is passed through for hardware
rendering. Connect a VNC client to `localhost:5900` to watch Chrome work.

```bash
# One-click — auto-detects GPU (AMD/Intel/NVIDIA)
./docker/run-headful.sh

# Then connect to the Docker Chrome from Python:
CHROME_WS_URLS="http://localhost:19222,http://localhost:19223" \
  python examples/docker_headful.py

# Watch Chrome live in your browser:
#   Open http://localhost:6080 and click Connect
#   (Or use a VNC client on localhost:5900)
```

See [docs/docker-headful.md](docs/docker-headful.md) for the full guide.

## API Reference

### `BrowserPool` (Python)

```python
pool = await BrowserPool.from_env()  # reads env vars
```

| Env Variable | Default | Description |
|---|---|---|
| `CHROME_WS_URLS` | — | Comma-separated URLs (connect mode) |
| `BROWSER_COUNT` | `1` | Chrome processes to launch |
| `TABS_PER_BROWSER` | `4` | Tabs per browser |
| `TAB_MAX_USES` | `50` | Hard-recycle threshold |
| `TAB_MAX_IDLE_SECS` | `60` | Idle eviction timeout |
| `CHROME_NO_SANDBOX` | — | Set `"1"` for containers |

**Methods** (all async):
- `warmup()` — Pre-open tabs (called by `async with`)
- `acquire() -> PooledTab` — Check out a tab (blocks if all busy)
- `release(tab)` — Return a tab to the pool

### `PooledTab` (Python)

Same methods as `Page` (navigate, content, title, url, evaluate_js, screenshot_png, query_selector, etc.) plus:
- `use_count: int` — How many times this tab has been used

Use as async context manager for auto-release: `async with pool.acquire() as tab:`

### `BrowserSession` (Python)

```python
BrowserSession(
    *,
    headless: bool = True,
    ws_url: str | None = None,
    stealth: bool = True,
    no_sandbox: bool = False,
    proxy: str | None = None,
    chrome_executable: str | None = None,
    extra_args: list[str] | None = None,
)
```

**Methods** (all async):
- `launch()` — Launch browser (called automatically by `async with`)
- `new_page(url: str) -> Page` — Open a new tab and navigate
- `version() -> str` — Browser version string
- `close()` — Shut down the browser

### `Page` (Python)

**Methods** (all async):
- `goto(url: str, timeout: float = 30.0) -> str | None` — Navigate + wait for network idle in one shot. Returns `"networkIdle"`, `"networkAlmostIdle"`, or `None` on timeout
- `navigate(url: str)` — Navigate to a new URL (no waiting)
- `wait_for_navigation()` — Wait for current navigation to complete
- `wait_for_network_idle(timeout: float = 30.0) -> str | None` — Event-driven wait for network idle
- `content() -> str` — Full page HTML
- `title() -> str | None` — Page title
- `url() -> str | None` — Current URL
- `evaluate_js(expression: str) -> object` — Evaluate JS, returns native Python types (dict/list/str/int/float/bool/None)
- `screenshot_png() -> bytes` — Full-page PNG screenshot
- `pdf_bytes() -> bytes` — PDF of the page
- `query_selector(selector: str) -> str | None` — Inner HTML of first match
- `query_selector_all(selector: str) -> list[str]` — Inner HTML of all matches
- `click_element(selector: str)` — Click first matching element
- `type_into(selector: str, text: str)` — Type text into first matching element
- `set_headers(headers: dict[str, str])` — Set extra HTTP headers
- `close()` — Close this tab

See [full API reference](docs/api-reference.md) for detailed docs.

## Examples

See the [`examples/`](examples/) directory:

| Script | Description |
|---|---|
| `basic_navigation.py` | Launch headless, navigate, read title/content |
| `connect_to_existing_chrome.py` | Connect via WebSocket URL to a running Chrome |
| `custom_headers_and_proxy.py` | Set custom HTTP headers and proxy |
| `dom_and_interaction.py` | `query_selector`, `click_element`, `type_into` |
| `javascript_eval.py` | Evaluate JS expressions, modify DOM |
| `multi_page.py` | Multiple pages in one session |
| `screenshot_and_pdf.py` | Capture PNG screenshots and PDFs |
| `stealth_mode.py` | Compare fingerprints: stealth on vs off |
| `docker_headful.py` | Connect to Docker Chrome with GPU + VNC |

## Testing

```bash
# Rust integration tests (serial due to Chrome singleton lock)
cargo test -p void_crawl_core -- --test-threads=1

# Python integration tests (requires void_crawl built and Chrome installed)
python -m pytest tests/ -v
```

## Dependencies

All dependencies are MIT or Apache-2.0 licensed — no AGPL exposure.

| Crate | Version | Purpose |
|---|---|---|
| `chromiumoxide` | 0.9 | CDP client, browser management |
| `tokio` | 1.x | Async runtime |
| `pyo3` | 0.28 | Rust ↔ Python bindings |
| `pyo3-async-runtimes` | 0.28 | Tokio ↔ asyncio bridge |
| `thiserror` | 2.x | Error derive macros |
| `serde` / `serde_json` | 1.x | Serialization |
| `reqwest` | 0.12 | HTTP (WebSocket URL resolution) |
| `futures` | 0.3 | Stream utilities (Handler loop) |

## License

Apache-2.0
