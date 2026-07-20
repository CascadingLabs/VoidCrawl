"""Exercise four live Ahrefs free checkers concurrently.

This mirrors Nimbal's Ahrefs use case: each checker owns one tab and becomes
ready when its meaningful ``/v4/`` response arrives. It deliberately does not
wait for network idle and never prints or persists captured response bodies.

Ahrefs may present a CAPTCHA or rate limit. The example runs headfully by
default, matching Nimbal; set ``AHREFS_HEADLESS=1`` for headless diagnostics.
Keep this to occasional manual testing; do not hammer the free service.
"""

import asyncio
import os
import time
from typing import Any, NamedTuple
from urllib.parse import urlencode

from voidcrawl import BrowserConfig, BrowserSession, CapturedResponse

TARGET_DOMAIN = os.getenv("AHREFS_TARGET_DOMAIN", "example.com")
TARGET_URL = os.getenv("AHREFS_TARGET_URL", "https://example.com")
TARGET_KEYWORD = os.getenv("AHREFS_TARGET_KEYWORD", "voidcrawl")
HEADLESS = os.getenv("AHREFS_HEADLESS", "0") != "0"
TIMEOUT = float(os.getenv("AHREFS_TIMEOUT", "45"))


class CheckerJob(NamedTuple):
    name: str
    page_url: str
    routes: dict[str, str]


class CheckerResult(NamedTuple):
    name: str
    elapsed: float
    responses: dict[str, str]
    error: str | None = None


def checker_url(base_url: str, value: str) -> str:
    return f"{base_url}?{urlencode({'input': value})}"


JOBS = (
    CheckerJob(
        "authority",
        checker_url("https://ahrefs.com/website-authority-checker/", TARGET_DOMAIN),
        {"overview": "**/v4/stGetFreeWebsiteOverview"},
    ),
    CheckerJob(
        "backlinks",
        checker_url("https://ahrefs.com/backlink-checker/", TARGET_URL),
        {
            "overview": "**/v4/stGetFreeBacklinksOverview",
            "list": "**/v4/stGetFreeBacklinksList",
        },
    ),
    CheckerJob(
        "traffic",
        checker_url("https://ahrefs.com/traffic-checker/", TARGET_URL),
        {"overview": "**/v4/stGetFreeTrafficOverview"},
    ),
    CheckerJob(
        "serp",
        checker_url("https://ahrefs.com/serp-checker/", TARGET_KEYWORD),
        {"serp": "**/v4/*[sS][eE][rR][pP]*"},
    ),
)


def summarize_payload(payload: Any) -> str:
    """Describe payload shape without logging Ahrefs response data."""
    if isinstance(payload, list):
        tag = payload[0] if payload and isinstance(payload[0], str) else None
        value_type = type(payload[1]).__name__ if len(payload) > 1 else "missing"
        return f"list(tag={tag!r}, value_type={value_type})"
    if isinstance(payload, dict):
        return f"object(keys={len(payload)})"
    return type(payload).__name__


async def summarize_response(response: CapturedResponse) -> str:
    if response.status != 200:
        return f"HTTP {response.status}"
    if response.body_state == "unavailable":
        return f"body unavailable: {response.body_error}"
    payload = await response.json()
    suffix = " (truncated)" if response.truncated else ""
    return f"HTTP 200 {summarize_payload(payload)}{suffix}"


async def run_checker(
    browser: BrowserSession,
    job: CheckerJob,
    started: float,
) -> CheckerResult:
    try:
        async with browser.page() as page:
            # DOM readiness is sufficient to find the form. The /v4/ routes,
            # not unrelated background traffic, determine when extraction is done.
            await page.navigate(job.page_url)
            await page.wait_for_selector('form button[type="submit"]', timeout=TIMEOUT)

            async with page.expect_responses(
                job.routes,
                timeout=TIMEOUT,
                max_response_bytes=2_000_000,
                max_total_bytes=4_000_000,
            ) as pending:
                await page.click_element('form button[type="submit"]')

            captured = await pending.value
            summaries = {
                name: await summarize_response(response)
                for name, response in captured.items()
            }
            return CheckerResult(job.name, time.monotonic() - started, summaries)
    except Exception as error:
        return CheckerResult(
            job.name,
            time.monotonic() - started,
            {},
            f"{type(error).__name__}: {error}",
        )


async def main() -> None:
    print(
        "Launching four concurrent Ahrefs checker tabs "
        f"(headless={HEADLESS}, target={TARGET_DOMAIN!r})"
    )
    async with BrowserSession(BrowserConfig(headless=HEADLESS)) as browser:
        started = time.monotonic()
        tasks = [
            asyncio.create_task(run_checker(browser, job, started), name=job.name)
            for job in JOBS
        ]

        for completed in asyncio.as_completed(tasks):
            result = await completed
            if result.error is not None:
                print(f"{result.elapsed:0.2f}s {result.name:10} FAILED {result.error}")
            else:
                print(f"{result.elapsed:0.2f}s {result.name:10} {result.responses}")


if __name__ == "__main__":
    asyncio.run(main())
