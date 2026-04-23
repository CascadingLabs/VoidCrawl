# Native Chrome Profiles

`voidcrawl` can boot Chrome against an existing native profile — the same one you use for logged-in sessions. This lets a scraper reuse cookies, localStorage, and extensions without going through a login flow.

## Quick start

```python
from voidcrawl import with_profile

async with with_profile("Default") as handle:
    page = await handle.new_page("https://linkedin.com/feed")
    html = await page.content()
```

`with_profile` is exclusive: only one voidcrawl process can hold a given profile at a time (see [isolation rationale](experiments/profile-isolation.md)).

## API

```python
from voidcrawl import (
    list_profiles,         # -> list[tuple[name, path]]
    acquire_profile,       # -> ProfileHandle (explicit use)
    with_profile,          # async context manager (preferred)
    ProfileBusy,           # another voidcrawl process holds the lock
    ProfileLeaseExpired,   # timed out waiting for the lock
    ProfileNotFound,       # no matching profile on disk
)
```

### `list_profiles()`

Returns `(name, path)` tuples for every Chrome profile discovered in the platform default dirs:

| OS      | Base dir                                                  |
|---------|-----------------------------------------------------------|
| Linux   | `~/.config/google-chrome/`, `~/.config/chromium/`         |
| macOS   | `~/Library/Application Support/Google/Chrome/`            |
| Windows | `%LOCALAPPDATA%\Google\Chrome\User Data\`                 |

Only directories containing a `Preferences` file are returned — this filters out `Crashpad`, `ShaderCache`, and other non-profile siblings.

### `acquire_profile(name, lease_timeout=300.0)`

Acquires an exclusive lease on the named profile, launches Chrome pointing at it, and returns a `ProfileHandle`. You must `await handle.release()` when done.

- `ProfileNotFound` — no matching profile in the platform default dirs.
- `ProfileBusy` — raised immediately if `lease_timeout=0` and the profile is locked.
- `ProfileLeaseExpired` — raised after polling for `lease_timeout` seconds.

### `with_profile(name, lease_timeout=300.0)`

Async context manager wrapping acquire + release.

### `ProfileHandle` methods

- `await handle.new_page(url)` — open a tab and navigate.
- `await handle.path()` — the on-disk profile directory.
- `await handle.release()` — close Chrome, drop the lock.

## Chrome's own lock

Chrome itself locks a profile via `SingletonLock` when it runs. If your **real** Chrome is currently using the profile, `acquire_profile` will launch a second Chrome that fails at startup. Close the user's Chrome first, or use a profile dedicated to voidcrawl.

`voidcrawl`'s own lock (`.voidcrawl.lock` in the profile dir) only arbitrates between voidcrawl processes.

## MCP server pinning

`voidcrawl-mcp` accepts `--profile NAME` (or `VOIDCRAWL_PROFILE=NAME` env) to bind the whole server to one profile at startup. Profile management is **not** exposed to MCP clients — agents don't acquire profiles, pipelines do.

```bash
voidcrawl-mcp --profile "Default"
```
