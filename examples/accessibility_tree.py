"""Fetch the browser-computed accessibility (AX) tree for a rendered page.

The AX tree is the semantic view assistive tech sees: implicit roles resolved,
accessible names computed, and hidden or presentational nodes pruned out. It
comes back as a *flat* list of nodes linked by ``childIds`` — this example also
reconstructs the hierarchy for a readable dump.
"""

import asyncio
import json

from voidcrawl import BrowserPool, PoolConfig

TARGET_URL = "https://qscrape.dev/l2/news"


def _axval(node: dict, key: str) -> str:
    """Pull the inner ``.value`` out of an AXValue-wrapped field, or ''."""
    v = node.get(key)
    return "" if v is None else str(v.get("value", ""))


def print_tree(nodes: list[dict]) -> None:
    """Render the flat AX node list as an indented role/name tree."""
    by_id = {n["nodeId"]: n for n in nodes}
    children: dict[str, list[str]] = {n["nodeId"]: n.get("childIds", []) for n in nodes}
    roots = [n["nodeId"] for n in nodes if n.get("parentId") not in by_id]

    def walk(node_id: str, depth: int) -> None:
        node = by_id[node_id]
        if not node.get("ignored", False):
            role = _axval(node, "role") or "—"
            name = _axval(node, "name")
            label = f"{role}" + (f"  {name!r}" if name else "")
            print(f"{'  ' * depth}{label}")
        for child_id in children.get(node_id, []):
            if child_id in by_id:
                walk(child_id, depth + 1)

    for root_id in roots:
        walk(root_id, 0)


async def main() -> None:
    async with BrowserPool(PoolConfig()) as pool, pool.acquire() as tab:
        await tab.goto(TARGET_URL)

        # One CDP round-trip; the page must already be rendered (goto waits).
        ax_nodes = await tab.get_full_ax_tree()

        print(f"AX nodes: {len(ax_nodes)}")
        print("\n── role/name tree ──")
        print_tree(ax_nodes)

        print("\n── first 3 raw nodes ──")
        print(json.dumps(ax_nodes[:3], indent=2))


if __name__ == "__main__":
    asyncio.run(main())
