"""Interactive step debugger for browser actions.

``DebugSession`` wraps a sequence of actions and pauses before each one so
you can inspect state, replay steps, or skip ahead without re-running the
whole script.

Key concepts
------------
* **stepping=True** (default) — pause before *every* action.
* **stepping=False** — run freely and only pause at ``@vc_breakpoint`` actions.
* **highlight=True** (default) — flash a red CSS outline on the target element
  before the action runs, so you can see exactly what will be clicked/typed.
* **start_url** — required if you want to use the back (b) or restart (r)
  commands; the debugger navigates back to this URL and replays.

Debugger key bindings
---------------------
n / Enter   execute current action and advance
c           continue running until the next breakpoint
b           rewind one step (re-navigates and replays)
r           restart from the beginning
l           list all queued actions with position marker
h           show history of executed actions and results
q           quit the session early

Run this example
----------------
    # Requires a visible browser window so you can see the highlights.
    python examples/debug_session.py

Uses qscrape.dev/l2/taxes (Eldoria Registry of Deeds) — a JS-rendered
search form whose DOM is only available after hydration.
"""

import asyncio

from voidcrawl import BrowserConfig, BrowserSession
from voidcrawl.actions import (
    ClickElement,
    Flow,
    GetText,
    JsActionNode,
    SetInputValue,
    WaitForSelector,
    inline_js,
)
from voidcrawl.debug import DebugSession, vc_breakpoint

TARGET_URL = "https://qscrape.dev/l2/taxes"


# ---------------------------------------------------------------------------
# Custom action decorated with @vc_breakpoint.
#
# When stepping=False, the debugger runs all actions without pausing — except
# for actions whose class is decorated with @vc_breakpoint.  This mirrors how
# debugger breakpoints work in an IDE: mark the lines that matter, then press
# "continue" and the debugger stops only where you asked.
# ---------------------------------------------------------------------------
@vc_breakpoint
class ViewFirstRecord(JsActionNode):
    """Click the first property record's view button — marked as a breakpoint."""

    js = inline_js("document.querySelector(__params.s).click();")

    def __init__(self, s: str) -> None:
        self.s = s


async def demo_stepping() -> None:
    """Part 1: full step-through mode (pause before every action)."""
    print("\n=== Part 1: stepping mode (pause before every action) ===\n")

    # headless=False is strongly recommended while debugging so you can see
    # the red highlight flash on each targeted element.
    async with BrowserSession(BrowserConfig(headless=False)) as browser:
        page = await browser.new_page(TARGET_URL)

        dbg = DebugSession(
            page,
            start_url=TARGET_URL,  # enables b (back) and r (restart) commands
            stepping=True,  # pause before every action
            highlight=True,  # flash red outline on targeted elements
            step_delay=0.3,  # delay between auto-run steps (when not paused)
        )

        # Wait for the Astro island to hydrate, fill the search, and read results.
        dbg.add(WaitForSelector(".er-input"))  # wait for form to appear
        dbg.add(SetInputValue(".er-input", "Smith"))  # fill the owner-name field
        dbg.add(ClickElement(".er-btn-primary"))  # submit the search
        dbg.add(WaitForSelector(".er-row"))  # wait for result rows
        dbg.add(GetText(".er-row"))  # read the first result row

        result = await dbg.start()
        print(f"\nCollected results: {result.results}")


async def demo_breakpoints() -> None:
    """Part 2: run freely, pause only at @vc_breakpoint actions."""
    print("\n=== Part 2: breakpoint-only mode (stepping=False) ===\n")

    async with BrowserSession(BrowserConfig(headless=False)) as browser:
        page = await browser.new_page(TARGET_URL)

        dbg = DebugSession(
            page,
            start_url=TARGET_URL,
            stepping=False,  # do NOT pause at every step …
            highlight=True,
        )

        # These three run without pausing …
        dbg.add(WaitForSelector(".er-input"))
        dbg.add(SetInputValue(".er-input", "Smith"))
        dbg.add(ClickElement(".er-btn-primary"))
        dbg.add(WaitForSelector(".er-row"))

        # … but this one is decorated with @vc_breakpoint, so the debugger
        # will stop here even though stepping=False.
        dbg.add(ViewFirstRecord(".er-view-btn"))

        dbg.add(
            GetText(".er-back-btn")
        )  # resumes after you press n/c at the breakpoint

        result = await dbg.start()
        print(f"\nCollected results: {result.results}")


async def demo_flow() -> None:
    """Part 3: load a pre-built Flow into the debugger with add_flow()."""
    print("\n=== Part 3: debugging a Flow object ===\n")

    async with BrowserSession(BrowserConfig(headless=False)) as browser:
        page = await browser.new_page(TARGET_URL)

        # Build a normal Flow first …
        flow = (
            Flow()
            .add(WaitForSelector(".er-input"))
            .add(SetInputValue(".er-input", "Jones"))
            .add(ClickElement(".er-btn-primary"))
            .add(WaitForSelector(".er-row"))
            .add(GetText(".er-row"))
        )

        # … then hand the whole flow to a DebugSession via add_flow().
        # This is handy when you already have a production Flow and just want
        # to wrap it in the debugger without rewriting it action-by-action.
        dbg = DebugSession(page, start_url=TARGET_URL, stepping=True)
        dbg.add_flow(flow)

        result = await dbg.start()
        print(f"\nCollected results: {result.results}")


async def main() -> None:
    await demo_stepping()
    await demo_breakpoints()
    await demo_flow()


if __name__ == "__main__":
    asyncio.run(main())
