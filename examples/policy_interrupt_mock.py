"""Pause the same tab for explicit operator review.

QScrape is an authorized, non-transactional test target. The ``payment``
scenario opens its VaultMart L2 page; no cart or checkout action is sent.
QScrape deliberately has no authentication or CAPTCHA implementation, so
``login`` and ``recaptcha`` instead display self-contained, interactive policy
mocks. They do not detect, solve, or bypass either kind of challenge.

Run after ``uv run maturin develop``::

    uv run python examples/policy_interrupt_mock.py --scenario payment
    uv run python examples/policy_interrupt_mock.py --scenario login
    uv run python examples/policy_interrupt_mock.py --scenario recaptcha

Set ``QSCRAPE_URL`` to use a controlled local QScrape deployment. The default
is its public L2 e-shop test page. The browser opens headfully so an operator
can inspect the retained tab.
"""

from __future__ import annotations

import argparse
import asyncio
import os
from dataclasses import dataclass
from urllib.parse import quote

from voidcrawl import (
    BrowserConfig,
    BrowserSession,
    InterruptRequest,
    SessionInterrupted,
)

QSCRAPE_URL = os.environ.get("QSCRAPE_URL", "https://qscrape.dev/l2/eshop/")


def mock_page_url(*, title: str, content: str) -> str:
    """Return a visible, local-only fixture without a network request."""
    html = f"""<!doctype html>
<title>{title}</title>
<style>
  body {{ background: #111827; color: #e5e7eb; font: 18px system-ui; }}
  main {{ max-width: 520px; margin: 12vh auto; padding: 32px; }}
  .card {{ background: #fff; border-radius: 4px; color: #202124; padding: 24px; }}
  label, input {{ display: block; margin: 12px 0; }}
  input {{ font: inherit; padding: 8px; width: 95%; }}
  .captcha {{
    align-items: center; border: 1px solid #d5d9dd; display: flex; padding: 16px;
  }}
  .captcha input {{ height: 28px; margin-right: 16px; width: 28px; }}
  button {{
    background: #2563eb; border: 0; border-radius: 4px; color: white;
    padding: 10px 16px;
  }}
  small {{ color: #6b7280; display: block; margin-top: 24px; }}
</style>
<main><h1>{title}</h1><div class="card">{content}</div></main>"""
    return f"data:text/html,{quote(html)}"


def scenario_url(scenario_name: str) -> str:
    if scenario_name == "payment":
        return QSCRAPE_URL
    if scenario_name == "login":
        return mock_page_url(
            title="Mock login review",
            content="""<p>Operator-controlled fixture; credentials stay in this tab.</p>
<label>Username <input autocomplete="username"></label>
<label>Password <input type="password" autocomplete="current-password"></label>
<button onclick="document.querySelector('p').textContent =
'Mock login completed by operator.'">Sign in</button>
<small>No authentication request is made.</small>""",
        )
    return mock_page_url(
        title="Mock reCAPTCHA review",
        content="""<p>Operator-controlled CAPTCHA fixture.</p>
<div class="captcha"><input id="human" type="checkbox">
<label for="human">I'm not a robot</label></div>
<small>This is not Google reCAPTCHA and no verification request is made.</small>""",
    )


@dataclass(frozen=True)
class Scenario:
    """Caller-declared policy metadata; VoidCrawl does not infer it."""

    code: str
    summary: str


SCENARIOS = {
    "payment": Scenario(
        code="policy.payment.confirmation",
        summary="Review the VaultMart checkout boundary; do not submit payment.",
    ),
    "login": Scenario(
        code="policy.login.credentials",
        summary="Mock login boundary: an operator must handle credentials in this tab.",
    ),
    "recaptcha": Scenario(
        code="policy.captcha.operator",
        summary=(
            "Mock CAPTCHA boundary: an operator must perform authorized verification."
        ),
    ),
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scenario", choices=SCENARIOS, default="payment")
    return parser.parse_args()


async def prompt() -> bool:
    answer = await asyncio.to_thread(
        input, "Type resolved to resume, anything else to release: "
    )
    return answer.strip().lower() == "resolved"


async def main(scenario_name: str) -> None:
    scenario = SCENARIOS[scenario_name]
    async with BrowserSession(BrowserConfig(headless=False)) as browser:
        page_url = scenario_url(scenario_name)
        page = await browser.new_page(page_url)
        original_url = await page.url()
        original_target_id = await page.target_id()
        print(f"Opened {await page.title()!r} in target {original_target_id}.")

        interrupt = await browser.interrupt(
            page,
            InterruptRequest(code=scenario.code, summary=scenario.summary),
        )
        print(
            f"Parked {scenario_name} review: target={interrupt.target_id} "
            f"interrupt={interrupt.interrupt_id}"
        )
        print(
            "The operator may inspect the visible tab; VoidCrawl mutations are paused."
        )

        try:
            await page.navigate(page_url)
        except SessionInterrupted:
            print(
                "Verified: a navigation attempt was rejected while the tab is parked."
            )
        else:
            raise RuntimeError("expected the interrupted page to reject navigation")

        if await prompt():
            result = await browser.resume(interrupt.interrupt_id)
            assert result.target_id == original_target_id
            assert await page.target_id() == original_target_id
            assert await page.url() == original_url
            print(
                "Resumed the original target without replaying the rejected navigation."
            )
        else:
            result = await browser.release(interrupt.interrupt_id)
            print("Released the interrupt without replaying the rejected navigation.")

        print(f"Interrupt is now {result.state}.")


if __name__ == "__main__":
    asyncio.run(main(parse_args().scenario))
