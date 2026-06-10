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

## Ephemeral vs. persistent profiles

A session can run against either:

- **Ephemeral** (default): a fresh, cookieless `TempDir`, deleted on session
  close. Maximum isolation — no shared cookies, history, or fingerprint state.
  Right for parallel fan-out where tasks must not see each other.
- **Persistent** (`user_data_dir` / `with_profile`): a directory that survives
  across sessions, so cookies, `localStorage`, and a logged-in state carry
  over. Right for authenticated scraping or banking a warm browsing history.

```python
from voidcrawl import BrowserConfig, BrowserSession

# ephemeral (default)
async with BrowserSession() as s:
    ...

# persistent — reuse a dedicated voidcrawl profile dir
async with BrowserSession() as s:
    page = await s.builder().user_data_dir("~/.config/voidcrawl-acme").new_page(url)

# persistent — declarative, via BrowserConfig
cfg = BrowserConfig(user_data_dir="~/.config/voidcrawl-acme")
async with BrowserSession(cfg) as s:
    ...
```

> Pick a directory **dedicated** to voidcrawl. Chrome locks a profile while
> running, so pointing at your live daily-driver profile while normal Chrome is
> open fails with a `SingletonLock` conflict.

> `BrowserConfig.user_data_dir` is a single-process profile, so it only applies
> to a **single launched browser**. A `BrowserPool` rejects it when
> `browsers != 1` or when `chrome_ws_urls` is set (sharing one locked profile
> across browsers, or pushing it to a remote Chrome, can't work) — both raise a
> `ValueError` at start.

## Profile & proxy rotation (bot-wall hygiene)

Hitting a bot-managed domain repeatedly from one identity raises its risk
score — one block makes the next likelier. The levers, in order of impact:

1. **Residential / rotating proxy** — the single biggest lever for IP
   reputation. Pass `proxy` per session and rotate the exit per task or per N
   requests:

   ```python
   cfg = BrowserConfig(proxy="http://user:pass@residential-pool:port")
   ```

2. **Rotate the profile** — give each "identity" its own persistent
   `user_data_dir` and round-robin across a small pool, so cookies/history
   don't all accrue against one fingerprint. Don't fan a single profile across
   concurrent sessions (Chrome locks it).
3. **Pace and reuse** — reuse one session for same-origin work (realistic
   cookies + pacing); space `fetch_many` batches against managed domains rather
   than firing them back-to-back.

There is no built-in rotator — rotation is a caller/pipeline pattern: keep a
pool of `(proxy, user_data_dir)` identities and pick one per task.

## Warm profiles & Cloudflare `cf_clearance` — what they do and don't fix

A "warm" profile (one that's browsed a Cloudflare-fronted site) carries a
`cf_clearance` cookie. Useful to understand precisely what that buys you,
because it's easy to over-rely on:

- ✅ **`cf_clearance` satisfies the Cloudflare *edge* challenge** — the
  interstitial "checking your browser" gate for a domain. A warm profile skips
  that on revisits.
- ❌ **It does NOT satisfy an inline managed-Turnstile widget.** A site's
  embedded Turnstile issues a *separate* `cf-turnstile-response` token, scored
  **fresh per request** on live fingerprint/IP/behavior. A banked
  `cf_clearance` cookie does nothing for it.

So a warm profile helps you *reach* a page behind Cloudflare, but to clear an
on-page Turnstile you still need a good live score — which means **headful +
hardware GPU + consistent UA** (see [stealth.md](stealth.md)), and, if the IP
is flagged, a cleaner exit (proxy). `cf_clearance` is also bound to the UA, so a
warm profile only helps if the session presents the *same* UA that earned it.

To persist `cf_clearance` across runs, mount a persistent `user_data_dir` (or,
in Docker, a volume over the profile dir — see [docker-mcp.md](docker-mcp.md)).
