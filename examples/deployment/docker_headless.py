"""Use voidcrawl with Chrome running headless inside Docker.

This example connects to Chrome instances running in the VoidCrawl headless
Docker container (two Chrome processes on ports 9222 and 9223).

Setup (run this first in a separate terminal):

    docker compose -f docker/docker-compose.yml up

Then run this script:

    uv run python examples/deployment/docker_headless.py

Three connection patterns are shown:

1. ``PoolConfig.from_docker()``   — simplest; probes ports, raises if the
                                    container is not running.
2. ``PoolConfig.from_env()``      — reads CHROME_WS_URLS / SCALE_PROFILE from
                                    the environment, mirroring how the Docker
                                    entrypoint wires up the pool automatically.
3. ``SCALE_PROFILE`` via env      — auto-sizes Chrome to container memory and
                                    CPU limits without hard-coding URLs.

Environment variables read by from_env():

    CHROME_WS_URLS     comma-separated http://host:port URLs (set by entrypoint)
    BROWSER_COUNT      Chrome processes to launch (overridden by CHROME_WS_URLS)
    TABS_PER_BROWSER   concurrent tabs per process (default: 4)
    TAB_MAX_IDLE_SECS  idle eviction timeout in seconds (default: 60)
    CHROME_NO_SANDBOX  "1" to disable sandbox (required in Docker)
    CHROME_HEADLESS    "0" for headful, "1" for headless (default: 1)
    SCALE_PROFILE      minimal | balanced | advanced (skips above if set)
"""

import asyncio
import os

from voidcrawl import BrowserPool, PoolConfig

TARGET_URLS = [
    "https://qscrape.dev/l2/news",
    "https://qscrape.dev/l2/scoretap",
    "https://qscrape.dev/l2/eshop",
]


# ── Helper ────────────────────────────────────────────────────────────────


async def fetch(pool: BrowserPool, url: str) -> tuple[str, int]:
    """Navigate to *url* and return (title, html_length)."""
    async with pool.acquire() as tab:
        await tab.goto(url, timeout=30.0)
        title = await tab.title() or "(no title)"
        html = await tab.content()
    return title, len(html)


def _print_results(results: list[tuple[str, int]]) -> None:
    for url, (title, length) in zip(TARGET_URLS, results, strict=True):
        short = url.split("/")[-1]
        print(f"  {short:10s}  title={title[:40]!r}  len={length:,}")


# ── Pattern 1: PoolConfig.from_docker() ──────────────────────────────────


async def demo_from_docker() -> None:
    """Connect via the default Docker headless ports (9222/9223).

    from_docker() probes both ports before returning — you get an informative
    error instead of a cryptic connection failure if the container is down.
    """
    print("\n── Pattern 1: PoolConfig.from_docker() ──────────────────────")
    # headful=False (default) → ports 9222/9223
    # from_docker(headful=True) → ports 19222/19223 (headful container)
    pool_cfg = PoolConfig.from_docker(tabs_per_browser=4)
    print(f"  WS URLs  : {pool_cfg.chrome_ws_urls}")
    print(f"  Browsers : {pool_cfg.browsers}  tabs : {pool_cfg.tabs_per_browser}")

    async with BrowserPool(pool_cfg) as pool:
        results = await asyncio.gather(*[fetch(pool, url) for url in TARGET_URLS])

    _print_results(results)


# ── Pattern 2: PoolConfig.from_env() — mirror the Docker entrypoint ──────


async def demo_from_env() -> None:
    """Build a pool from environment variables.

    The Docker entrypoint writes CHROME_WS_URLS, BROWSER_COUNT, etc. into the
    container environment.  from_env() reads exactly those variables so your
    application code doesn't need to know whether it's running in Docker or on
    bare metal.

    To simulate locally, export the same variables before running::

        export CHROME_WS_URLS="http://localhost:9222,http://localhost:9223"
        export CHROME_NO_SANDBOX=1
        uv run python examples/deployment/docker_headless.py
    """
    print("\n── Pattern 2: PoolConfig.from_env() ─────────────────────────")
    # Simulate what the Docker entrypoint sets up.
    env_overrides = {
        "CHROME_WS_URLS": "http://localhost:9222,http://localhost:9223",
        "TABS_PER_BROWSER": "4",
        "TAB_MAX_IDLE_SECS": "60",
        "CHROME_NO_SANDBOX": "1",
    }
    original = {k: os.environ.get(k) for k in env_overrides}
    os.environ.update(env_overrides)

    try:
        pool_cfg = PoolConfig.from_env()
        print(f"  WS URLs    : {pool_cfg.chrome_ws_urls}")
        print(f"  Browsers   : {pool_cfg.browsers}  tabs : {pool_cfg.tabs_per_browser}")
        print(
            f"  Headless   : {pool_cfg.browser.headless}"
            f"  no_sandbox : {pool_cfg.browser.no_sandbox}"
        )

        async with BrowserPool(pool_cfg) as pool:
            results = await asyncio.gather(*[fetch(pool, url) for url in TARGET_URLS])

        _print_results(results)
    finally:
        # Restore original environment
        for k, v in original.items():
            if v is None:
                os.environ.pop(k, None)
            else:
                os.environ[k] = v


# ── Pattern 3: SCALE_PROFILE — auto-size Chrome to container resources ────


async def demo_scale_profile() -> None:
    """Use SCALE_PROFILE so the pool is auto-sized to container limits.

    When SCALE_PROFILE is set, from_env() delegates to compute_scale() which
    reads /proc, cgroup memory limits, and FD limits, then returns a PoolConfig
    sized accordingly.  This is what the Docker entrypoint does when
    CHROME_WS_URLS is not pre-set.

    The entrypoint generates a supervisord.conf that launches the right number
    of Chrome processes, then sets CHROME_WS_URLS.  Your application calls
    from_env() to pick up the result.
    """
    print("\n── Pattern 3: SCALE_PROFILE=balanced via from_env() ─────────")
    original_profile = os.environ.get("SCALE_PROFILE")
    original_ws = os.environ.get("CHROME_WS_URLS")
    # Clear CHROME_WS_URLS so from_env() takes the SCALE_PROFILE path.
    os.environ.pop("CHROME_WS_URLS", None)
    os.environ["SCALE_PROFILE"] = "balanced"

    try:
        from voidcrawl.scale import compute_scale  # noqa: PLC0415

        report = compute_scale("balanced")
        print(f"  Detected env  : {report.detected_env}")
        print(f"  Profile       : {report.profile}")
        print(f"  Browsers      : {report.browsers}")
        print(f"  Tabs/browser  : {report.tabs_per_browser}")
        print(f"  Total tabs    : {report.total_tabs}")
        print(f"  Headless      : {report.headless}")
        for w in report.warnings:
            print(f"  Warning       : {w}")

        # In production: from_env() calls compute_scale for you, then pass
        # the result straight to BrowserPool.
        pool_cfg = PoolConfig.from_env()
        print(
            f"\n  PoolConfig : browsers={pool_cfg.browsers}"
            f" tabs={pool_cfg.tabs_per_browser}"
        )
        # pool_cfg.chrome_ws_urls is empty here — no container is connected.
        # In the real Docker flow, CHROME_WS_URLS is set by the entrypoint
        # before your application starts.
        print("  (skipping browser launch — no container connected in this demo)")
    finally:
        if original_profile is None:
            os.environ.pop("SCALE_PROFILE", None)
        else:
            os.environ["SCALE_PROFILE"] = original_profile
        if original_ws is not None:
            os.environ["CHROME_WS_URLS"] = original_ws


# ── main ──────────────────────────────────────────────────────────────────


async def main() -> None:
    print("VoidCrawl Docker headless examples")
    print("Requires: docker compose -f docker/docker-compose.yml up")

    await demo_from_docker()
    await demo_from_env()
    await demo_scale_profile()

    print("\nDone.")
    print("Stop the container with:")
    print("  docker compose -f docker/docker-compose.yml down")


if __name__ == "__main__":
    asyncio.run(main())
