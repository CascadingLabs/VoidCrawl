# Experiment Log: Profile Isolation (D2)

**Decision:** one Chrome process per leased profile, with a separate `--user-data-dir`. Recorded 2026-04-22.

## Options considered

### A. One Chrome per profile (chosen)

Each `acquire_profile(name)` launches a brand new Chrome process with `--user-data-dir=<profile path>`. Chrome's own `SingletonLock` prevents a second process from attaching.

**Pro:**
- Matches the existing `BrowserSessionBuilder::user_data_dir` plumbing — no invasive rewiring.
- Per-profile crash isolation: one profile crashing doesn't take down others.
- Simple mental model: 1 handle = 1 Chrome.

**Con:**
- ~200MB RAM per active profile.
- Slow cold start (~2–5s per Chrome boot on a warm profile).

### B. Single Chrome with `--profile-directory`

Boot one Chrome at startup and switch profiles via `chrome.windows.create({ profile: ... })` equivalents.

**Pro:**
- One Chrome process; lower total RAM if multiple profiles active.
- Faster profile-switch (no boot).

**Con:**
- CDP semantics around profile switching are underdocumented and flaky.
- `chromiumoxide` doesn't expose a clean profile-switching API — we'd be reaching past it.
- Cross-process exclusivity becomes harder: Chrome's `SingletonLock` no longer arbitrates per-profile.
- One Chrome crash loses every profile at once.

## Verdict

Pick A. The extra RAM is not a real constraint for the pipeline use case (a handful of profiles active at once), and the operational simplicity is worth more than the theoretical efficiency of B. Revisit if we ever need 20+ concurrent profiles.

## Advisory lock (D5)

`voidcrawl`'s own `.voidcrawl.lock` is held via `fs2::try_lock_exclusive` on a file inside the profile directory. This:

- Releases automatically when the holding process exits (OS-level, via `close(2)` on the fd).
- Arbitrates voidcrawl-vs-voidcrawl only — Chrome's own `SingletonLock` still applies on top.
- Works on Linux + macOS + Windows via `fs2`'s platform shims.
