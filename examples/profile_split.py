"""Run two Chrome instances from the same managed-profile baseline.

The one-line magic is ``registry.split_profile(...)``. While entering that
context, VoidCrawl leases the source once and copies it into unique temporary
``user_data_dir`` roots. Chrome sees distinct directories, so each instance can
own its own ``SingletonLock`` even though cookies, storage, extensions, and
profile identity started from the same source.

This is intentionally copy-on-start, not live synchronization. Changes made by
one Chrome diverge from the other and are not merged back into the source.
"""

import asyncio
import tempfile
from pathlib import Path

from voidcrawl import BrowserConfig, BrowserSession
from voidcrawl.profiles import ProfileRegistry


async def main() -> None:
    with tempfile.TemporaryDirectory(prefix="voidcrawl-profile-demo-") as root:
        registry = ProfileRegistry(root)
        registry.create_profile("work", description="Shared starting profile")

        # One source lease covers both copies, giving them one consistent
        # baseline rather than two snapshots taken at different moments.
        async with registry.split_profile("work", copies=2) as split:
            first_path, second_path = split.paths
            print("different profile directories:", first_path != second_path)

            first = BrowserSession(BrowserConfig(user_data_dir=first_path))
            second = BrowserSession(BrowserConfig(user_data_dir=second_path))
            async with first, second:
                first_ws, second_ws = await asyncio.gather(
                    first.websocket_url(), second.websocket_url()
                )
                print("different Chrome instances:", first_ws != second_ws)

                first_page, second_page = await asyncio.gather(
                    first.new_page("data:text/html,<title>Worker one</title>"),
                    second.new_page("data:text/html,<title>Worker two</title>"),
                )
                print("titles:", await first_page.title(), await second_page.title())
                await asyncio.gather(first_page.close(), second_page.close())

            paths = [Path(first_path), Path(second_path)]
            print(
                "copies exist inside split context:",
                all(path.exists() for path in paths),
            )

        print("copies cleaned after context:", all(not path.exists() for path in paths))


if __name__ == "__main__":
    asyncio.run(main())
