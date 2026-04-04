"""Demonstrate the actions framework: prebaked, custom, and composed flows.

Run modes controlled by ``MODE`` at the top of the file:

- ``"debug"``  — interactive step debugger (headful, breakpoints, back/forward)
- ``"replay"`` — automatic headful replay with delay between steps
- ``"fast"``   — headless, no instrumentation (default for CI)
"""

import asyncio

from void_crawl import BrowserConfig, BrowserSession
from void_crawl.actions import (
    ActionNode,
    CdpClick,
    ClickElement,
    Flow,
    GetText,
    JsActionNode,
    ScrollTo,
    SetInputValue,
    Tab,
    inline_js,
)
from void_crawl.debug import DebugSession, vd_breakpoint

# ── Run configuration ────────────────────────────────────────────────────
MODE = "debug"  # "debug" | "replay" | "fast"
STEP_DELAY = 1.0  # Seconds between steps in replay mode

DEMO_PAGE = """data:text/html,
<html>
<body>
  <h1 id="title">Actions Demo</h1>
  <input id="name" type="text" placeholder="Your name" />
  <button id="greet" onclick="
    document.getElementById('title').textContent =
      'Hello, ' + document.getElementById('name').value + '!';
  ">Greet</button>
  <div id="output" style="margin-top:20px;"></div>
</body>
</html>
"""


# ── Custom JS action ─────────────────────────────────────────────────────


class AppendToOutput(JsActionNode):
    """Custom action: append text to the #output div."""

    js = inline_js("""\
const el = document.querySelector(__params.selector);
el.innerHTML += '<p>' + __params.text + '</p>';
return el.children.length;
""")

    def __init__(self, text: str, selector: str = "#output") -> None:
        self.text = text
        self.selector = selector


# ── Custom CDP action ────────────────────────────────────────────────────


class CdpDoubleClick(ActionNode):
    """Custom action: double-click at coordinates via CDP."""

    def __init__(self, x: float, y: float) -> None:
        self.x = x
        self.y = y

    async def run(self, tab: Tab) -> None:
        for _ in range(2):
            await tab.dispatch_mouse_event(
                "mousePressed", self.x, self.y, click_count=2
            )
            await tab.dispatch_mouse_event(
                "mouseReleased", self.x, self.y, click_count=2
            )


# ── Mark an action as a breakpoint (pauses even in continue mode) ────────


@vd_breakpoint
class BreakpointClick(JsActionNode):
    """Example breakpointed action: click via JS and always pause in debugger."""

    js = inline_js("""\
const el = document.querySelector(__params.selector);
if (!el) throw new Error('Element not found: ' + __params.selector);
el.click();
return null;
""")

    def __init__(self, selector: str) -> None:
        self.selector = selector


# ── Main ─────────────────────────────────────────────────────────────────


async def main() -> None:
    headless = MODE == "fast"

    async with BrowserSession(BrowserConfig(headless=headless)) as browser:
        page = await browser.new_page(DEMO_PAGE)

        if MODE == "debug":
            # Interactive debugger — step, back, continue, breakpoints
            dbg = DebugSession(
                page,
                start_url=DEMO_PAGE,
                stepping=True,
                highlight=True,
            )
            dbg.add(SetInputValue("#name", "World"))
            dbg.add(ClickElement("#greet"))
            dbg.add(GetText("#title"))
            dbg.add(AppendToOutput("First line"))
            dbg.add(ScrollTo(0, 0))
            dbg.add(AppendToOutput("Added via flow"))
            dbg.add(GetText("#output"))
            # This one is decorated with @vd_breakpoint — pauses even after "c"
            dbg.add(BreakpointClick("#greet"))
            dbg.add(CdpClick(100.0, 50.0))

            result = await dbg.start()
            print(f"\nFinal results: {result.results}")

        elif MODE == "replay":
            # Automatic replay — headful with delay, no interaction
            dbg = DebugSession(
                page,
                start_url=DEMO_PAGE,
                stepping=False,
                step_delay=STEP_DELAY,
                highlight=True,
            )
            dbg.add(SetInputValue("#name", "World"))
            dbg.add(ClickElement("#greet"))
            dbg.add(GetText("#title"))
            dbg.add(AppendToOutput("First line"))
            dbg.add(ScrollTo(0, 0))
            dbg.add(AppendToOutput("Added via flow"))
            dbg.add(GetText("#output"))
            dbg.add(CdpClick(100.0, 50.0))

            result = await dbg.start()
            print(f"\nFinal results: {result.results}")

        else:
            # Fast mode — headless, no instrumentation
            await SetInputValue("#name", "World").run(page)
            await ClickElement("#greet").run(page)
            title = await GetText("#title").run(page)
            print(f"Title: {title}")

            count = await AppendToOutput("First line").run(page)
            print(f"Output children: {count}")

            flow = Flow(
                [
                    ScrollTo(0, 0),
                    AppendToOutput("Added via flow"),
                    GetText("#output"),
                ]
            )
            result = await flow.run(page)
            print(f"Flow results: {result.results}")

            await CdpClick(100.0, 50.0).run(page)
            print("CDP click dispatched")

        await page.close()


if __name__ == "__main__":
    asyncio.run(main())
