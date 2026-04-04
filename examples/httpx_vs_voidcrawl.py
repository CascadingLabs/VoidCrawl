"""httpx vs voidcrawl — ScoreTap (qscrape.dev/l2/scoretap).

Plain HTTP cannot see JS-rendered scores — and walks straight into a
honeypot trap planted in the static HTML to poison naive scrapers.
voidcrawl executes the page's JavaScript and extracts the real match data.
"""

import asyncio
import re

import httpx

import voidcrawl as vc
from voidcrawl import BrowserConfig, BrowserSession
from voidcrawl.actions import QueryAll

TARGET_URL = "https://qscrape.dev/l2/scoretap"


class Match(vc.Schema):
    team_a: str = vc.Selector(".st-team-right .st-team-name")
    team_b: str = vc.Selector(".st-team:not(.st-team-right) .st-team-name")
    score: str | None = vc.Selector(".st-score")
    status: str | None = vc.Selector(".st-live-badge, .st-final-badge")
    event: str | None = vc.Selector(".st-match-event")
    game: str | None = vc.Selector(".st-game-tag")


# ── 1. httpx (static HTML only) ─────────────────────────────────────────


def scrape_with_httpx() -> None:
    print("=" * 64)
    print("httpx  (plain HTTP GET)")
    print("=" * 64)

    r = httpx.get(TARGET_URL, follow_redirects=True)
    print(f"  Status : {r.status_code}")
    print(f"  Size   : {len(r.text):,} chars  (Astro shell — JS islands not executed)")

    match_rows = re.findall(r'class="st-match-row"', r.text)
    print(f"  Matches found : {len(match_rows)}  ← zero real data")

    # The static HTML contains a hidden honeypot to poison naive scrapers.
    if 'data-qs-trap="scoretap"' in r.text:
        trap_teams = re.findall(r'data-team="([^"]+)"', r.text)
        trap_scores = re.findall(r'data-score-[ab]="(\d+)"', r.text)
        print(
            f"\n  [!] Honeypot trap (data-qs-trap) — fake data planted for bots:\n"
            f"      Teams  : {trap_teams}\n"
            f"      Scores : {trap_scores}"
        )

    print()


# ── 2. voidcrawl (JS-rendered via CDP) ──────────────────────────────────


async def scrape_with_voidcrawl() -> None:
    print("=" * 64)
    print("voidcrawl  (JS-rendered via CDP)")
    print("=" * 64)

    async with BrowserSession(BrowserConfig(headless=True)) as browser:
        page = await browser.new_page(TARGET_URL)
        await page.wait_for_network_idle(timeout=15.0)

        html = await page.content()
        print(f"  Size   : {len(html):,} chars  (fully hydrated DOM)")

        matches: list[Match] = await QueryAll(".st-match-row", Match).run(page)
        print(f"  Matches found : {len(matches)}\n")

        col = "{:<8} {:>7}  {:<8}  {:<18}  {:<12}  {}"
        print("  " + col.format("Team A", "Score", "Team B", "Status", "Game", "Event"))
        print("  " + "-" * 62)
        for m in matches:
            score = (m.score or "").replace("\xa0", " ")
            status = (m.status or "").replace("●", "").strip()
            print(
                "  "
                + col.format(
                    m.team_a,
                    score,
                    m.team_b,
                    status,
                    m.game or "",
                    m.event or "",
                )
            )

    print()


# ── Main ─────────────────────────────────────────────────────────────────


async def main() -> None:
    scrape_with_httpx()
    await scrape_with_voidcrawl()


if __name__ == "__main__":
    asyncio.run(main())
