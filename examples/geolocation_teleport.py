"""Teleport the browser around the world via CDP geolocation override.

For each stop we override geolocation + timezone + locale, then prove the spoof
took hold three independent ways:

  1. navigator.geolocation (read on a secure page) reports the fake coordinates
  2. the page's local clock jumps to the city's timezone
  3. a real reverse-geocoder, given the coords navigator.geolocation reported,
     names the city back to us

navigator.geolocation only works in a secure context, so we read it on an https
page (example.com) rather than a data: URL. The reverse-geocode step is
best-effort (needs network) and degrades to just printing coordinates.
"""

import asyncio
import json

from voidcrawl import BrowserConfig, BrowserSession

# (label, latitude, longitude, IANA timezone, locale)
TOUR = [
    ("Times Square, New York", 40.7580, -73.9855, "America/New_York", "en-US"),
    ("Eiffel Tower, Paris", 48.8584, 2.2945, "Europe/Paris", "fr-FR"),
    ("Shibuya Crossing, Tokyo", 35.6595, 139.7004, "Asia/Tokyo", "ja-JP"),
    ("Sydney Opera House", -33.8568, 151.2153, "Australia/Sydney", "en-AU"),
]

SECURE_PAGE = "https://example.com"
REVERSE_GEOCODE = (
    "https://api.bigdatacloud.net/data/reverse-geocode-client"
    "?latitude={lat}&longitude={lon}&localityLanguage=en"
)

# Fire getCurrentPosition and stash the result where we can poll for it.
READ_GEO_JS = """
window.__geo = null;
navigator.geolocation.getCurrentPosition(
  p => { window.__geo = {lat: p.coords.latitude, lon: p.coords.longitude}; },
  e => { window.__geo = {error: e.code}; }
);
true
"""


async def poll(page, expr, tries=40, delay=0.05):
    for _ in range(tries):
        val = await page.evaluate_js(expr)
        if val is not None:
            return val
        await asyncio.sleep(delay)
    return None


async def reverse_geocode(page, lat, lon) -> str:
    """Top-level navigation to a keyless reverse-geocoder; parse its JSON.

    Top-level nav sidesteps CORS. Best-effort: returns '' on any failure.
    """
    try:
        await page.goto(REVERSE_GEOCODE.format(lat=lat, lon=lon))
        body = await page.evaluate_js("document.body.innerText")
        data = json.loads(body)
        city = data.get("city") or data.get("locality") or "?"
        return f"{city}, {data.get('countryName', '?')}"
    except Exception as exc:  # demo: never let geocoding abort the tour
        return f"(reverse-geocode unavailable: {exc})"


async def main() -> None:
    async with BrowserSession(BrowserConfig()) as browser:
        page = await browser.new_page(SECURE_PAGE)

        for label, lat, lon, tz, locale in TOUR:
            await page.set_geolocation(lat, lon)
            await page.set_timezone(tz)
            await page.set_locale(locale)

            # Re-load the secure page so the overrides are in effect, then read.
            await page.goto(SECURE_PAGE)
            await page.evaluate_js(READ_GEO_JS)
            geo = await poll(page, "window.__geo")
            local_time = await page.evaluate_js(
                "new Intl.DateTimeFormat(undefined,"
                "{dateStyle:'medium',timeStyle:'short'}).format(new Date())"
            )

            print(f"\n🌍  Teleporting to {label}")
            print(f"    set       → {lat:.4f}, {lon:.4f}  ({tz}, {locale})")
            if geo and "lat" in geo:
                reported = f"{geo['lat']:.4f}, {geo['lon']:.4f}"
                print(f"    browser   → navigator.geolocation says {reported}")
            else:
                print(f"    browser   → geolocation read failed: {geo}")
            print(f"    clock     → local time now reads {local_time}")

            place = await reverse_geocode(page, lat, lon)
            print(f"    geocoder  → {place}")

        await page.close()


if __name__ == "__main__":
    asyncio.run(main())
