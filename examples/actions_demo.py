"""Demonstrate the actions framework: prebaked, custom, and composed flows."""

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


async def main() -> None:
    # Toggle headless=False to watch the browser visually
    async with BrowserSession(BrowserConfig(headless=False)) as browser:
        page = await browser.new_page(DEMO_PAGE)

        # 1. Use individual prebaked actions
        print("--- Individual actions ---")
        await SetInputValue("#name", "World").run(page)
        await ClickElement("#greet").run(page)
        title = await GetText("#title").run(page)
        print(f"Title after greet: {title}")

        # 2. Use a custom JS action (no params() override needed)
        print("\n--- Custom JS action ---")
        count = await AppendToOutput("First line").run(page)
        print(f"Output children after append: {count}")

        # 3. Compose actions into a flow
        print("\n--- Flow ---")
        flow = Flow(
            [
                ScrollTo(0, 0),
                AppendToOutput("Added via flow"),
                GetText("#output"),
            ]
        )
        result = await flow.run(page)
        print(f"Flow results: {result.results}")
        print(f"Last result (output text): {result.last}")

        # 4. CDP-level action
        print("\n--- CDP click ---")
        await CdpClick(100.0, 50.0).run(page)
        print("CDP click dispatched at (100, 50)")

        await page.close()


if __name__ == "__main__":
    asyncio.run(main())
