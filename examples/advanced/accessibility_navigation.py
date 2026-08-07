"""Full use case: drive a page through its accessibility tree.

Unlike CSS selectors (which break when markup is refactored), addressing by
accessibility *role* + computed *name* targets what assistive tech perceives —
far more durable across redesigns. This example:

  1. Renders a JS-heavy page.
  2. Reads the AX tree and lists the actionable controls (buttons / links).
  3. Clicks one *by role + name* — no CSS selector involved.
  4. Confirms the navigation took effect.

AX addressing is powerful but not infallible (ambiguous or localized names,
state-dependent text), so treat it as one tool among CSS, coordinates, and
text — not the only one. Here we fall back to reporting cleanly if no
actionable control is found.
"""

import asyncio

from voidcrawl import BrowserPool, PoolConfig

TARGET_URL = "https://qscrape.dev/l2/news"

# Roles a user can actually act on — what we'd want to drive a page by.
ACTIONABLE = {"button", "link", "checkbox", "tab", "menuitem"}


def role_of(node: dict) -> str:
    role = node.get("role")
    return "" if role is None else str(role.get("value", ""))


def name_of(node: dict) -> str:
    name = node.get("name")
    return "" if name is None else str(name.get("value", ""))


async def main() -> None:
    async with BrowserPool(PoolConfig()) as pool, pool.acquire() as tab:
        await tab.goto(TARGET_URL)
        start_url = await tab.url()

        # 1. Find the actionable controls straight from the AX tree.
        nodes = await tab.get_full_ax_tree()
        controls = [
            (role_of(n), name_of(n))
            for n in nodes
            if not n.get("ignored", False) and role_of(n) in ACTIONABLE and name_of(n)
        ]

        print(f"Found {len(controls)} actionable controls. First few:")
        for role, name in controls[:8]:
            print(f"  {role:<8} {name!r}")

        if not controls:
            print("\nNo named, actionable controls — this page's AX tree is thin.")
            print("Fall back to CSS/visual addressing here.")
            return

        # 2. Cross-check with a targeted query (the resolver click_by_role uses).
        target_role, target_name = controls[0]
        matches = await tab.query_ax_tree(role=target_role, name=target_name)
        print(
            f"\nqueryAXTree(role={target_role!r}, name={target_name!r}) "
            f"→ {len(matches)} match(es)"
        )

        # 3. Click it by role + name — no selector.
        print(f"Clicking {target_role} {target_name!r} …")
        await tab.click_by_role(target_role, target_name)

        # 4. Confirm something changed (URL navigation or new title).
        await tab.wait_for_network_idle(timeout=10.0)
        end_url = await tab.url()
        print(f"\nbefore: {start_url}")
        print(f"after:  {end_url}")
        print(
            "navigated ✓"
            if end_url != start_url
            else "same URL (in-page update or no-op)"
        )


if __name__ == "__main__":
    asyncio.run(main())
