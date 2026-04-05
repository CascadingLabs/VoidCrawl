"""Stress test: 30 tasks across 3 browser pools — extraction + live interaction.

Pool layout:

    Pool 0  headful   1 browser  10 tabs  -> news extraction
    Pool 1  headful   1 browser  10 tabs  -> scoretap extraction
    Pool 2  headful   1 browser  10 tabs  -> taxes interactive search

Pool 2 demonstrates concurrent interactions: each of its 10 tabs navigates
to the Eldoria Registry of Deeds, types a different search term, clicks
search, waits for results, then extracts structured records — all 10 running
simultaneously inside the same Chrome window.

All 30 tasks fire at once via asyncio.gather().  Each pool's internal
semaphore keeps its own 10 tabs saturated without touching the others.

Run::

    ./build.sh                              # build the Rust extension once
    uv run python examples/concurrent_pools.py

What you'll see:

    - 3 headful Chrome windows open side by side
    - Pools 0 and 1: tabs cycling through news articles and match scores
    - Pool 2: 10 tabs simultaneously typing different names into a search
      form and each pulling back a different set of property records
    - Results streaming in as tabs finish, with item counts and previews
    - Summary table: per-pool latency and per-schema item counts
"""

import asyncio
import time
from contextlib import AsyncExitStack
from dataclasses import dataclass, field
from typing import Any

import voidcrawl as vc
from voidcrawl import BrowserConfig, BrowserPool, PoolConfig
from voidcrawl.actions import QueryAll

# ── Schemas ───────────────────────────────────────────────────────────────


class NewsArticle(vc.Schema):
    headline: str = vc.Selector(".hn-feed-headline")
    category: str | None = vc.Selector(".hn-feed-cat")
    excerpt: str | None = vc.Selector(".hn-feed-excerpt", sanitize=vc.strip_tags)


class SportMatch(vc.Schema):
    team_a: str = vc.Selector(".st-team-right .st-team-name")
    team_b: str = vc.Selector(".st-team:not(.st-team-right) .st-team-name")
    score: str | None = vc.Selector(".st-score")
    status: str | None = vc.Selector(".st-live-badge, .st-final-badge")


class PropertyRecord(vc.Schema):
    """One row returned by the Eldoria Registry of Deeds search."""

    index: str | None = vc.Selector(".er-index-badge")
    status: str | None = vc.Selector(".er-status-badge")
    detail: str | None = vc.Selector(".er-cell-mono")
    info: str | None = vc.Selector(".er-cell-muted")


# ── URL / search-term sets ────────────────────────────────────────────────

# Pools 0 and 1 cycle their respective pages 10 times each.
_NEWS_URL = "https://qscrape.dev/l2/news"
_SCORETAP_URL = "https://qscrape.dev/l2/scoretap"
_TAXES_URL = "https://qscrape.dev/l2/taxes"

# 10 distinct names — one per interactive tab in pool 2.
SEARCH_TERMS = [
    "Smith",
    "Brown",
    "Jones",
    "Williams",
    "Taylor",
    "Davies",
    "Wilson",
    "Evans",
    "Thomas",
    "Roberts",
]

# ── URL -> (container selector, schema class) dispatch ───────────────────

_EXTRACTORS: dict[str, tuple[str, type[vc.Schema]]] = {
    "news": (".hn-feed-item", NewsArticle),
    "scoretap": (".st-match-row", SportMatch),
}


def _extractor_for(url: str) -> tuple[str, type[vc.Schema]] | None:
    for key, extractor in _EXTRACTORS.items():
        if key in url:
            return extractor
    return None


# ── Pool configurations ───────────────────────────────────────────────────

POOL_CONFIGS = [
    PoolConfig(
        browsers=1,
        tabs_per_browser=10,
        browser=BrowserConfig(headless=False),  # headful — news
    ),
    PoolConfig(
        browsers=1,
        tabs_per_browser=10,
        browser=BrowserConfig(headless=False),  # headful — scoretap
    ),
    PoolConfig(
        browsers=1,
        tabs_per_browser=10,
        browser=BrowserConfig(headless=False),  # headful — interactive taxes
    ),
]

# ── Result dataclass ──────────────────────────────────────────────────────


@dataclass
class Result:
    url: str
    pool_idx: int
    ok: bool
    elapsed_ms: float
    title: str | None = field(default=None)
    items: list[Any] = field(default_factory=list)
    search_term: str | None = field(default=None)
    error: str | None = field(default=None)


# ── Selector-level wait ───────────────────────────────────────────────────


async def _wait_for_items(tab: Any, selector: str, timeout: float = 15.0) -> bool:
    """Poll via evaluate_js until at least one element matching *selector* exists.

    goto() fires on network-idle but Astro/Vue/Svelte client-side rendering
    may still be in flight.  Polling for the actual container element is more
    reliable than waiting for DOM size to stabilise.

    Returns ``True`` if elements appeared within *timeout*, ``False`` if the
    deadline expired (e.g. search returns no results).
    """
    deadline = time.monotonic() + timeout
    js = f"document.querySelectorAll('{selector}').length"
    while time.monotonic() < deadline:
        count = await tab.evaluate_js(js)
        if isinstance(count, int) and count > 0:
            return True
        await asyncio.sleep(0.25)
    return False


# ── Extraction fetch (pools 0 and 1) ─────────────────────────────────────


async def fetch(pool: BrowserPool, pool_idx: int, url: str) -> Result:
    t0 = time.monotonic()
    try:
        async with pool.acquire() as tab:
            await tab.goto(url, timeout=30.0)
            title = await tab.title()

            items: list[Any] = []
            extractor = _extractor_for(url)
            if extractor is not None:
                container_sel, schema_cls = extractor
                if await _wait_for_items(tab, container_sel):
                    items = await QueryAll(container_sel, schema_cls).run(tab)

        return Result(
            url=url,
            pool_idx=pool_idx,
            ok=True,
            elapsed_ms=(time.monotonic() - t0) * 1000,
            title=title,
            items=items,
        )
    except Exception as exc:
        return Result(
            url=url,
            pool_idx=pool_idx,
            ok=False,
            elapsed_ms=(time.monotonic() - t0) * 1000,
            error=str(exc),
        )


# ── Interactive fetch (pool 2) ────────────────────────────────────────────


async def fetch_interactive(
    pool: BrowserPool, pool_idx: int, url: str, term: str
) -> Result:
    """Navigate to *url*, type *term* into the search form, submit, extract rows."""
    t0 = time.monotonic()
    try:
        async with pool.acquire() as tab:
            await tab.goto(url, timeout=30.0)

            # Wait for Astro island (search form) to mount.
            await _wait_for_items(tab, ".er-input")

            # Type the search term and submit.
            await tab.type_into(".er-input", term)
            await tab.evaluate_js("document.querySelector('.er-btn-primary').click()")

            # Poll directly for result rows — avoids wait_for_network_idle stalling
            # when all 10 tabs fire XHR simultaneously (never reaches "idle").
            # 8s is generous; results for matching names appear within ~3s.
            # Returns False if the name has no records — accepted as 0 items.
            has_rows = await _wait_for_items(tab, ".er-row", timeout=8.0)

            title = await tab.title()
            items = (
                await QueryAll(".er-row", PropertyRecord).run(tab) if has_rows else []
            )

        return Result(
            url=url,
            pool_idx=pool_idx,
            ok=True,
            elapsed_ms=(time.monotonic() - t0) * 1000,
            title=title,
            items=items,
            search_term=term,
        )
    except Exception as exc:
        return Result(
            url=url,
            pool_idx=pool_idx,
            ok=False,
            elapsed_ms=(time.monotonic() - t0) * 1000,
            search_term=term,
            error=str(exc),
        )


# ── Summary helpers ───────────────────────────────────────────────────────


def _preview(r: Result) -> str:  # noqa: PLR0911
    """One-line preview of extracted data for the live result row."""
    if not r.ok:
        return r.error or "(error)"
    if not r.items:
        if r.search_term:
            return f'0 records — "{r.search_term}" not in registry'
        return "(no items)"
    first = r.items[0]
    if isinstance(first, NewsArticle):
        cat = f"[{first.category}] " if first.category else ""
        return f"{len(r.items)} articles — {cat}{first.headline[:40]}"
    if isinstance(first, SportMatch):
        score = (first.score or "?").replace("\xa0", " ").strip()
        return f"{len(r.items)} matches — {first.team_a} {score} {first.team_b}"
    if isinstance(first, PropertyRecord):
        term = f'"{r.search_term}" → ' if r.search_term else ""
        detail = (first.detail or first.index or "?")[:30]
        status = first.status or "?"
        return f"{len(r.items)} records — {term}{detail} [{status}]"
    return f"{len(r.items)} items"


# ── Summary printing ─────────────────────────────────────────────────────


def _print_summary(
    results: list[Result],
    pool_labels: list[str],
    total_ms: float,
) -> None:
    total_items = sum(len(r.items) for r in results)
    print(f"\n{'─' * 64}")
    print(f"  Total wall time  : {total_ms:.0f} ms  ({total_ms / 1000:.1f} s)")
    print(f"  Tasks            : {len(results)}")
    print(f"  Succeeded        : {sum(r.ok for r in results)}")
    print(f"  Failed           : {sum(not r.ok for r in results)}")
    print(f"  Items extracted  : {total_items}")

    print(
        f"\n  {'pool':^10}  {'ok':>3}  {'items':>5}"
        f"  {'avg ms':>7}  {'min ms':>7}  {'max ms':>7}"
    )
    print("  " + "-" * 52)
    for idx, label in enumerate(pool_labels):
        ok_bucket = [r for r in results if r.pool_idx == idx and r.ok]
        pool_items = sum(len(r.items) for r in ok_bucket)
        if ok_bucket:
            avg = sum(r.elapsed_ms for r in ok_bucket) / len(ok_bucket)
            mn = min(r.elapsed_ms for r in ok_bucket)
            mx = max(r.elapsed_ms for r in ok_bucket)
            print(
                f"  {label:^10}  {len(ok_bucket):>3}  {pool_items:>5}"
                f"  {avg:>7.0f}  {mn:>7.0f}  {mx:>7.0f}"
            )
        else:
            print(f"  {label:^10}  {'0':>3}  {'0':>5}  {'—':>7}  {'—':>7}  {'—':>7}")

    print(f"\n  {'page type':^12}  {'fetches':>7}  {'total items':>11}  {'avg':>7}")
    print("  " + "-" * 44)
    for key in ("news", "scoretap", "taxes"):
        page_results = [r for r in results if key in r.url and r.ok]
        if page_results:
            total = sum(len(r.items) for r in page_results)
            avg_items = total / len(page_results)
            print(
                f"  {key:^12}  {len(page_results):>7}  {total:>11}  {avg_items:>7.1f}"
            )

    tax_results = [r for r in results if "taxes" in r.url and r.ok and r.search_term]
    if tax_results:
        print(f"\n  {'search term':^14}  {'records':>7}  sample record")
        print("  " + "-" * 56)
        for r in sorted(tax_results, key=lambda x: x.search_term or ""):
            sample = ""
            if r.items and isinstance(r.items[0], PropertyRecord):
                first = r.items[0]
                detail = (first.detail or "?")[:20]
                sample = f"{detail} [{first.status or '?'}]"
            print(f"  {r.search_term or '':^14}  {len(r.items):>7}  {sample}")


# ── Main ──────────────────────────────────────────────────────────────────


async def main() -> None:
    pool_labels = ["headful-0", "headful-1", "interact"]

    async with AsyncExitStack() as stack:
        # ── Launch all 3 pools ────────────────────────────────────────
        print("Launching 3 Chrome pools...")
        pools: list[BrowserPool] = [
            await stack.enter_async_context(BrowserPool(cfg)) for cfg in POOL_CONFIGS
        ]
        print(f"  Pool 0  {pool_labels[0]:12s}  tabs=10  news articles")
        print(f"  Pool 1  {pool_labels[1]:12s}  tabs=10  sport scores")
        print(f"  Pool 2  {pool_labels[2]:12s}  tabs=10  interactive search")

        # ── Warmup — pre-open all 30 tabs ─────────────────────────────
        print("\nWarming up (pre-opening tabs)...")
        t_warmup = time.monotonic()
        await asyncio.gather(*[p.warmup() for p in pools])
        print(f"  done in {(time.monotonic() - t_warmup) * 1000:.0f} ms")

        # ── Build task list ───────────────────────────────────────────
        # Pool 0: 10x news  |  Pool 1: 10x scoretap  |  Pool 2: 10x taxes (interactive)
        tasks: list[asyncio.Task[Result]] = []
        term_iter = iter(SEARCH_TERMS)
        for i in range(30):
            pool_idx = i % 3
            if pool_idx == 0:
                tasks.append(asyncio.create_task(fetch(pools[0], 0, _NEWS_URL)))
            elif pool_idx == 1:
                tasks.append(asyncio.create_task(fetch(pools[1], 1, _SCORETAP_URL)))
            else:
                term = next(term_iter)
                tasks.append(
                    asyncio.create_task(
                        fetch_interactive(pools[2], 2, _TAXES_URL, term)
                    )
                )

        # ── Dispatch ──────────────────────────────────────────────────
        print(f"\nDispatching {len(tasks)} tasks (all concurrent)...")
        results: list[Result] = []
        t_start = time.monotonic()

        header = f"  {'#':>3}  {'pool':^10}  {'ms':>6}  {'items':>5}  preview"
        print(header)
        print("  " + "-" * (len(header) - 2))

        for n, done in enumerate(asyncio.as_completed(tasks), start=1):
            r: Result = await done
            results.append(r)
            status_items = f"{len(r.items):>5}" if r.ok else " ERR "
            pool_label = pool_labels[r.pool_idx]
            preview = _preview(r)[:62]
            print(
                f"  {n:>3}  {pool_label:^10}  {r.elapsed_ms:>6.0f}"
                f"  {status_items}  {preview}"
            )

        total_ms = (time.monotonic() - t_start) * 1000

    _print_summary(results, pool_labels, total_ms)


if __name__ == "__main__":
    asyncio.run(main())
