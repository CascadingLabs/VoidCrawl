"""Anti-bot / CDN vendor detection (CAS-139) — real-world behavior.

Every ``goto`` now returns a signature-based fingerprint of the response on
``PageResponse.antibot``: *which* vendor is in front of the page and whether it
is **actively challenging** us (a wall) or merely **present** (a CDN serving us
fine). That single distinction is what lets a pipeline route deterministically
instead of retrying blind.

For each target this prints the status, the verdict (vendors / challenged /
which **evidence tier** matched), a few provenance-grade headers we now capture,
the routing decision the verdict would drive, and the exact JSON record a
replay-grade pipeline (Yosoi) would persist alongside the capture.

Honesty notes baked in, because the live web doesn't read our annotations:
  * ``evidence=body`` means the wall was a 200-cloaked challenge the cheap
    header tier missed and the body-prefix fallback caught — lower-confidence,
    higher-cost than a header tell.
  * "no vendor detected" is **not** proof we got through. A cloaked block can
    serve a decoy 200 with no telling header and look identical to a clean pass;
    a response fingerprinter cannot tell them apart (that's the DOM detector's
    job — see docs/antibot.md).

This fetches live third-party sites from your IP. Hammering known walls
(fortress, DataDome storefronts) repeatedly can flag your IP — run sparingly.
The routing table below is illustrative; the shipped policy lives in
docs/antibot.md, not here.

Run:  python examples/antibot_detection.py
"""

import asyncio
import json
from dataclasses import dataclass

from voidcrawl import (
    AntibotChallenge,
    AntibotVerdict,
    BrowserPool,
    PageResponse,
    PoolConfig,
)


@dataclass
class Target:
    url: str
    why: str  # what we expect to learn from this one


TARGETS = [
    Target(
        "https://news.ycombinator.com/",
        "no major anti-bot vendor — expect a clean verdict (the control)",
    ),
    Target(
        "https://example.com/",
        "Cloudflare-fronted but serves fine — the 'present, don't rotate' win",
    ),
    Target(
        "https://fortress.theplumber.dev/",
        "Cloudflare managed challenge — an active wall (also: a challenge here "
        "can hint our outbound stealth leaked; bot.sannysoft.com is the real "
        "outbound canary)",
    ),
    Target(
        "https://www.hermes.com/us/en/",
        "DataDome storefront — may serve fine, cloak, or wall a cold headless "
        "IP; a clean verdict here is not proof of success",
    ),
]


# ── Routing policy (illustrative; the shipped policy lives in docs/antibot.md) ─


def decide_route(verdict: AntibotVerdict | None) -> str:
    """Map a verdict to the action a pipeline would take.

    The quiet win over today's blind retry is the *no-op* row: a CDN that is
    merely present and serving real content must NOT cost a rotation.
    """
    if verdict is None or not verdict.vendors:
        return "use as-is — no vendor detected (NB: not proof we got through)"

    present = ", ".join(verdict.vendors)
    if not verdict.challenged:
        return f"use as-is — {present} present but not challenging (no rotation)"

    match verdict.challenge_vendor:
        case "cloudflare":
            return "challenged by cloudflare -> headful + warm profile (Turnstile)"
        case "datadome":
            return "challenged by datadome -> rotate residential proxy"
        case "perimeterx" | "kasada":
            return f"challenged by {verdict.challenge_vendor} -> headful + slow + warm"
        case other:
            return f"challenged by {other} -> rotate proxy/profile and retry"


# ── Reporting & persistence ─────────────────────────────────────────────

PROVENANCE_HEADERS = ("server", "cf-ray", "x-cache", "x-datadome", "via")


def provenance(resp: PageResponse) -> dict[str, str]:
    """Headers worth recording with a capture for replay-grade provenance."""
    return {k: resp.headers[k] for k in PROVENANCE_HEADERS if k in resp.headers}


def to_capture_record(target: Target, resp: PageResponse) -> dict[str, object]:
    """The exact shape a replay-grade pipeline persists next to the capture.

    The verdict is stored as a captured fact — including ``corpus_version`` —
    never recomputed at replay time against a newer corpus.
    """
    v = resp.antibot
    return {
        "url": target.url,
        "status_code": resp.status_code,
        "provenance_headers": provenance(resp),
        "antibot": None
        if v is None
        else {
            "vendors": v.vendors,
            "challenged": v.challenged,
            "challenge_vendor": v.challenge_vendor,
            "evidence": v.evidence,
            "corpus_version": v.corpus_version,
        },
    }


def report(target: Target, resp: PageResponse) -> None:
    v = resp.antibot
    pairs = provenance(resp)
    prov = ", ".join(f"{k}={val}" for k, val in pairs.items()) or "(none of note)"
    print(f"\n{'-' * 72}")
    print(f"  {target.url}")
    print(f"  expect: {target.why}")
    print(f"  status: {resp.status_code}   headers: {prov}")
    if v is None or not v.vendors:
        print("  verdict: no vendor detected")
    else:
        print(
            f"  verdict: vendors={v.vendors} challenged={v.challenged} "
            f"challenge_vendor={v.challenge_vendor!r} evidence={v.evidence} "
            f"(corpus {v.corpus_version})"
        )
    print(f"  route -> {decide_route(v)}")
    print(f"  record: {json.dumps(to_capture_record(target, resp))}")


async def fetch_and_report(pool: BrowserPool, target: Target) -> None:
    """Fetch one target and report; keep going on any single failure."""
    try:
        async with pool.acquire() as tab:
            resp = await tab.goto(target.url)
        report(target, resp)
    except AntibotChallenge as exc:
        # Opt-in hard-wall raise (not the default fetch path) — report it as a
        # verdict, not an anonymous failure, so the distinction stays visible.
        print(f"\n{'-' * 72}")
        print(f"  {target.url}\n  AntibotChallenge (opt-in raise): {exc}")
    except Exception as exc:  # demo: keep going on any other single failure
        print(f"\n{'-' * 72}")
        print(f"  {target.url}\n  fetch failed: {type(exc).__name__}: {exc}")


async def main() -> None:
    print("Anti-bot vendor detection — one pass per fetch, no extra round-trips.")
    # Headless so the comparison is honest: this is the path a bot wall sees.
    async with BrowserPool(PoolConfig()) as pool:
        for target in TARGETS:
            await fetch_and_report(pool, target)

    print(f"\n{'-' * 72}")
    print("Takeaway: 'present' is telemetry; 'challenged' is the rotate signal.")
    print("evidence=body = a 200-cloaked wall the header tier missed (see fortress).")
    print("The 'record' line is what you persist with the capture for replay.")


if __name__ == "__main__":
    asyncio.run(main())
