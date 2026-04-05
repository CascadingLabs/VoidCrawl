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
  <a href="https://pypi.python.org/pypi/voidcrawl"><img src="https://img.shields.io/pypi/v/voidcrawl?labelColor=120a24&color=b07adf" alt="PyPI"></a>
  <a href="https://pypi.python.org/pypi/voidcrawl"><img src="https://img.shields.io/pypi/pyversions/voidcrawl?labelColor=120a24&color=b07adf" alt="Python versions"></a>
  <a href="https://cascadinglabs.com/voidcrawl/"><img src="https://img.shields.io/badge/docs-cascadinglabs.com%2Fvoidcrawl-b07adf?labelColor=120a24" alt="docs"></a>
</p>

# VoidCrawl

**CDP browser automation for Python** — a Rust-native Chrome DevTools Protocol client exposed to Python via PyO3.

`void_crawl` replaces Playwright/Selenium with a permissively-licensed (Apache-2.0) stack for rendering JavaScript-heavy pages. Built on [chromiumoxide](https://github.com/mattsse/chromiumoxide) with a shared Tokio runtime.

> **Used by [Yosoi](https://github.com/CascadingLabs/Yosoi)** — an AI-powered selector discovery tool for resilient web scraping.

## Requirements

- **Rust** ≥ 1.86 (edition 2024)
- **Python** ≥ 3.10
- **Chrome/Chromium** installed on the system
- **maturin** ≥ 1.7 (`cargo install maturin`)

## Installation

```bash
# Build and install into your venv
./build.sh

# Or manually:
maturin develop --release --manifest-path crates/pyo3_bindings/Cargo.toml
```

## Quick Start

### BrowserPool (recommended)

Tabs are recycled, not closed — near-instant reuse across requests.

```python
import asyncio
from void_crawl import BrowserPool

async def main():
    async with BrowserPool.from_env() as pool:
        async with pool.acquire() as tab:
            await tab.navigate("https://example.com")
            print(await tab.title())
            print(len(await tab.content()))

asyncio.run(main())
```

### BrowserSession (low-level)

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

## Docker

```bash
docker compose -f docker/docker-compose.yml up -d

export CHROME_WS_URLS="http://localhost:9222,http://localhost:9223"
python examples/basic_navigation.py
```

For headful Docker with GPU + VNC, see [docs/docker-headful.md](docs/docker-headful.md).

## Testing

```bash
# Rust integration tests (serial — Chrome singleton lock)
cargo test -p void_crawl_core -- --test-threads=1

# Python integration tests (requires built extension + Chrome)
uv run pytest tests/ -v
```

## Documentation

- [Full API reference](docs/api-reference.md)
- [Examples](examples/)

## License

Apache-2.0
