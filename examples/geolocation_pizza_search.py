"""Same query, four cities: how "pizza near me" changes as you teleport.

Builds on ``geolocation_teleport.py``. For each stop we override geolocation +
timezone + locale (CDP ``setGeolocationOverride``, which also *grants* the
geolocation permission), then run the identical query ``pizza near me`` on
DuckDuckGo and Google and read back each engine's local pack — the real list of
nearby pizzerias, not generic web pages.

Three behaviours this example has to work around, all found by experiment:

  1. **Location stickiness.** Each engine writes the *first* location it resolves
     into a cookie and reuses it for the whole session — so reusing one tab leaks
     city N's results into city N+1. Fix: a fresh ``BrowserSession`` per zone, so
     every city starts with a clean cookie jar.

  2. **One-zone lag.** Even within a fresh session, an engine resolves location on
     load and only applies it on the *next* request — a single load shows the
     IP-based location, not the GPS spoof. Fix: load twice — a throwaway "prime"
     load, then the real read.

  3. **No stable selectors on DDG's local pack.** Its rows carry rotating,
     obfuscated class names. We anchor instead on the stable ``Results near …``
     text and pull the numbered place lines out of the module's innerText.

Why not a DuckDuckGo API client (e.g. ``ddgs``) instead of the browser? Because
those have no maps/local endpoint — their ``region`` param is country-level
(us-en, it-it), so they can't tell NYC from Chicago and return franchise
homepages for local-intent queries. Precise local results need the browser GPS
spoof, which is the whole point here.

Extraction is best-effort: an engine may serve a consent wall or bot check, in
which case we say so and move on.
"""

import asyncio

from voidcrawl import BrowserConfig, BrowserSession

# (label, latitude, longitude, IANA timezone, locale)
ZONES = [
    ("New York City", 40.7580, -73.9855, "America/New_York", "en-US"),
    ("Chicago", 41.8781, -87.6298, "America/Chicago", "en-US"),
    ("Naples, Italy", 40.8518, 14.2681, "Europe/Rome", "it-IT"),
    ("Tokyo", 35.6595, 139.7004, "Asia/Tokyo", "ja-JP"),
]

QUERY = "pizza near me"
TOP_N = 5
DDG_URL = "https://duckduckgo.com/?q={q}&ia=web"
GOOGLE_URL = "https://www.google.com/search?q={q}&hl=en"

# Read navigator.geolocation so we can prove the teleport landed before searching.
READ_GEO_JS = """
window.__geo = null;
navigator.geolocation.getCurrentPosition(
  p => { window.__geo = {lat: p.coords.latitude, lon: p.coords.longitude}; },
  e => { window.__geo = {error: e.code}; }
);
true
"""

# DDG local pack: find the module by its stable "Results near" text, then read
# the numbered place lines ("1. Rosati's Pizza …") out of its innerText.
DDG_EXTRACT_JS = """
(() => {
  const mod = [...document.querySelectorAll('article')]
    .find(a => /Results near/i.test(a.innerText));
  if (!mod) return {near: null, places: []};
  const lines = mod.innerText.split('\\n').map(s => s.trim());
  const hdr = lines.find(l => /^Results near/i.test(l)) || '';
  const near = hdr.replace(/^Results near\\s*/i, '');
  const places = [];
  for (const l of lines) {
    const m = l.match(/^(\\d+)\\.\\s+(.+)$/);   // "1. Gino's East - South Loop"
    if (m) places.push(m[2]);
  }
  return {near, places: places.slice(0, __TOP_N__)};
})()
"""

# Google: prefer the local-pack place names; fall back to organic <h3> headings.
# Drop the map/affordance chrome ("Map", "More places", "Directions").
GOOGLE_EXTRACT_JS = """
(() => {
  const drop = /^(Map|More places|Directions|Website|Rating|Hours)$/i;
  const titles = [...document.querySelectorAll('div#search h3, h3')]
    .map(n => (n.innerText || '').trim())
    .filter(t => t && !drop.test(t));
  return [...new Set(titles)].slice(0, __TOP_N__);
})()
"""


async def poll(page, expr, tries=40, delay=0.05):
    for _ in range(tries):
        val = await page.evaluate_js(expr)
        if val is not None:
            return val
        await asyncio.sleep(delay)
    return None


async def load_twice(page, url: str) -> bool:
    """Prime then read — clears the engines' one-zone location lag.

    Returns False if navigation failed or a bot wall appeared.
    """
    target = url.format(q=QUERY.replace(" ", "+"))
    try:
        await page.goto(target, timeout=30.0)  # prime: sets the location cookie
        await page.goto(target, timeout=30.0)  # read: now reflects this zone
    except Exception:
        return False
    await asyncio.sleep(1.0)  # let the local module's async render settle
    return not await page.detect_captcha()


async def search_zone(label, lat, lon, tz, locale) -> None:
    """One zone, one fresh browser (clean cookie jar) so cities don't leak."""
    async with BrowserSession(BrowserConfig()) as browser:
        # Secure page so the navigator.geolocation proof-read works.
        page = await browser.new_page("https://example.com")
        await page.set_geolocation(lat, lon)
        await page.set_timezone(tz)
        await page.set_locale(locale)

        await page.goto("https://example.com")
        await page.evaluate_js(READ_GEO_JS)
        geo = await poll(page, "window.__geo")

        print(f"\n{'=' * 64}")
        print(f"📍  {label}  ({lat:.4f}, {lon:.4f} · {tz} · {locale})")
        if geo and "lat" in geo:
            print(f"    browser GPS reports → {geo['lat']:.4f}, {geo['lon']:.4f}")
        else:
            print(f"    geolocation read failed: {geo}")
        print("=" * 64)

        # DuckDuckGo — local pack
        print(f'\n  🦆 DuckDuckGo — "{QUERY}"')
        if await load_twice(page, DDG_URL):
            ddg = await page.evaluate_js(
                DDG_EXTRACT_JS.replace("__TOP_N__", str(TOP_N))
            )
            if ddg["places"]:
                print(f"     results near: {ddg['near']}")
                for i, p in enumerate(ddg["places"], 1):
                    print(f"     {i}. {p[:80]}")
            else:
                print("     (no local pack — DDG showed only web results)")
        else:
            print("     (navigation failed or bot wall)")

        # Google — local pack / organic
        print(f'\n  🔎 Google — "{QUERY}"')
        if await load_twice(page, GOOGLE_URL):
            titles = await page.evaluate_js(
                GOOGLE_EXTRACT_JS.replace("__TOP_N__", str(TOP_N))
            )
            for i, t in enumerate(titles or ["(no results parsed)"], 1):
                print(f"     {i}. {t[:80]}")
        else:
            print("     (navigation failed or bot wall)")

        await page.close()


async def main() -> None:
    # Sequential, not gathered: a fresh browser per zone keeps cookie jars
    # isolated, and serial output reads top-to-bottom like a tour.
    for zone in ZONES:
        await search_zone(*zone)


if __name__ == "__main__":
    asyncio.run(main())
