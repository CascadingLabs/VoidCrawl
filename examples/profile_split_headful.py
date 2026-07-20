"""Visually prove one profile baseline can run in two headful Chrome instances.

With no arguments this forks your installed native Chrome ``Default`` profile:

    uv run python examples/profile_split_headful.py

Select another discovered profile name or an explicit profile-directory path
with ``--source``. Close regular Chrome before running so its on-disk databases
have a consistent state to copy.

Both windows start from isolated copies of the selected profile. They initially
contain the same browser state, but changes made after launch do not synchronize
or merge back into the source.
"""

import argparse
import asyncio
import math
import tempfile
from pathlib import Path
from urllib.parse import quote

from voidcrawl import BrowserConfig, BrowserSession, ChromeProfileBusy
from voidcrawl.profiles import ProfileRegistry


def demo_url(worker: str, color: str) -> str:
    html = f"""<!doctype html>
    <title>VoidCrawl profile split: {worker}</title>
    <body style="background:{color};color:white;font:32px sans-serif;padding:48px">
      <h1>Same profile baseline</h1>
      <p>{worker}</p>
      <p>This window is a separate Chrome instance.</p>
    </body>"""
    return f"data:text/html,{quote(html)}"


async def run_demo(registry: ProfileRegistry, source: str, hold_seconds: float) -> None:
    # This single context is the profile-forking operation: one source lease,
    # two consistent copies, two unique user_data_dir paths, automatic cleanup.
    async with registry.fork_profile(source, copies=2) as split:
        first_path, second_path = split.paths
        print("first user_data_dir: ", first_path)
        print("second user_data_dir:", second_path)

        first = BrowserSession(
            BrowserConfig(
                headless=False,
                user_data_dir=first_path,
                extra_args=[
                    "--class=voidcrawl-profile-one",
                    "--window-position=0,0",
                    "--window-size=900,800",
                ],
            )
        )
        second = BrowserSession(
            BrowserConfig(
                headless=False,
                user_data_dir=second_path,
                extra_args=[
                    "--class=voidcrawl-profile-two",
                    "--window-position=920,0",
                    "--window-size=900,800",
                ],
            )
        )

        async with first, second:
            first_ws, second_ws = await asyncio.gather(
                first.websocket_url(), second.websocket_url()
            )
            print("separate Chrome endpoints:", first_ws != second_ws)

            first_page, second_page = await asyncio.gather(
                first.new_page(demo_url("Worker one", "#6b3030")),
                second.new_page(demo_url("Worker two", "#303b6b")),
            )
            print(f"Holding both windows open for {hold_seconds:g} seconds...")
            await asyncio.sleep(hold_seconds)
            await asyncio.gather(first_page.close(), second_page.close())


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--source",
        default="Default",
        help="installed Chrome profile name or explicit profile-directory path",
    )
    parser.add_argument(
        "--snapshot-root",
        type=Path,
        help="temporary-copy parent; omitted uses an auto-cleaned temp directory",
    )
    parser.add_argument("--hold-seconds", type=float, default=20.0)
    return parser.parse_args()


async def main() -> None:
    args = parse_args()
    if not math.isfinite(args.hold_seconds) or args.hold_seconds < 0:
        raise ValueError("--hold-seconds must be finite and non-negative")

    try:
        if args.snapshot_root is not None:
            registry = ProfileRegistry(str(args.snapshot_root.expanduser()))
            await run_demo(registry, args.source, args.hold_seconds)
            return

        with tempfile.TemporaryDirectory(prefix="voidcrawl-headful-demo-") as root:
            await run_demo(ProfileRegistry(root), args.source, args.hold_seconds)
    except ChromeProfileBusy as error:
        raise SystemExit(
            f"Cannot fork {args.source!r} while Chrome is using its data root. "
            f"Close regular Chrome and retry.\n{error}"
        ) from None


if __name__ == "__main__":
    asyncio.run(main())
