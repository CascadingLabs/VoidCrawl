"""Park a Docker headful page while an operator views the same browser.

This walkthrough depends on CAS-243's rootless on-demand noVNC viewer.
Start the rootless headful container and open its short-lived viewer:

    VIEWER_MODE=local ./docker/run-headful.sh -d
    ./docker/viewer.sh open --browser 1 --ttl 15m

Then set the selected browser's CDP WebSocket URL and run this example:

    VOIDCRAWL_WS_URL=ws://127.0.0.1:19222/devtools/browser/... \
        uv run python examples/deployment/docker_policy_interrupt.py

The browser remains in the interrupted state while the operator uses noVNC.
Type ``resolved`` into this process only after completing an authorized step.
No credential or CAPTCHA-solving behavior is implemented here.
"""

from __future__ import annotations

import asyncio
import os

from voidcrawl import BrowserConfig, BrowserSession, InterruptRequest


async def main() -> None:
    ws_url = os.environ["VOIDCRAWL_WS_URL"]
    async with BrowserSession(BrowserConfig(ws_url=ws_url)) as browser:
        page = await browser.new_page("https://example.com")
        interrupt = await browser.interrupt(
            page,
            InterruptRequest(
                code="policy.operator_review",
                summary="Complete an authorized step in the noVNC viewer.",
            ),
        )
        print(f"Interrupt {interrupt.interrupt_id} is parked on {interrupt.target_id}.")
        result = await asyncio.to_thread(input, "Type resolved to resume: ")
        if result.strip().lower() == "resolved":
            await browser.resume(interrupt.interrupt_id)
            print("Resumed the original target without replaying navigation.")
        else:
            await browser.release(interrupt.interrupt_id)
            print("Released the interrupt without replaying navigation.")


if __name__ == "__main__":
    asyncio.run(main())
