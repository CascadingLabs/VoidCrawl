# Nodriver / Zendriver parity notes

Source inspected locally from PyPI wheels in `/tmp/driver-study`:

- `nodriver==0.50.3`
- `zendriver==0.15.4`

## Launch posture

Common low-noise flags:

- `--remote-allow-origins=*`
- `--no-first-run`
- `--no-service-autorun`
- `--no-default-browser-check`
- `--homepage=about:blank`
- `--no-pings`
- `--password-store=basic`
- `--disable-breakpad`
- `--disable-dev-shm-usage`
- `--disable-session-crashed-bubble`
- `--disable-search-engine-choice-screen`
- `--disable-features=IsolateOrigins,site-per-process`

VoidCrawl mirrors that low-noise core for launched sessions and Docker headless,
except it deliberately omits the legacy automation-shaped `--disable-infobars`
flag. The headful Docker farm also intentionally omits broad background/render
suppression flags and `AutomationControlled`; launched headless adds
`AutomationControlled` only because Chrome otherwise reports
`navigator.webdriver === true`.

## CDP posture

- Nodriver attaches to targets directly (`Target.attachToTarget(flatten=True)`).
- Nodriver uses `Target.getTargets()` polling; it does not need eager domain
  enables for normal operation.
- Zendriver can enable `Target.setDiscoverTargets(discover=True)` for target
  tracking, but auto-enables domains only when handlers are registered.
- Zendriver's headless preparation uses one-shot `Runtime.evaluate` to read the
  UA and `Network.setUserAgentOverride` to strip `Headless`.

VoidCrawl minimal mode follows the same direction:

- no `Runtime.enable`
- no `Network.enable`
- no `Performance.enable` / `Log.enable`
- no `Target.setAutoAttach`
- no isolated utility world
- no global target discovery in minimal mode; created targets are synthesized
  from `Target.createTarget` and attached directly

## Cloudflare helpers

Zendriver includes an interactive Cloudflare helper in
`zendriver/core/cloudflare.py`. It searches shadow roots for
`challenges.cloudflare.com`, computes the iframe box, and uses compositor-level
mouse clicks. This confirms VoidCrawl's AX/tree + trusted CDP input approach is
aligned with the ecosystem; it does not imply full-page managed challenges pass
without a suitable IP/profile/environment.

## Current live parity evidence

From this host/container:

- VoidCrawl minimal + Docker headful CDP: `Just a moment...`
- Nodriver temporary run: `Just a moment...`
- Bare Docker headful Chrome opened without VoidCrawl navigation: `Just a moment...`

So the current Cloudflare canary is an environment/profile/IP gate rather than a
VoidCrawl-only CDP regression.

Headless fingerprint benchmark on a local `data:` URL (`uv run --with nodriver
python scripts/bench_antibot_cdp.py ...`) shows VoidCrawl is already stricter
than stock nodriver defaults on local browser consistency:

| Signal | VoidCrawl minimal | Nodriver headless |
|---|---|---|
| `navigator.webdriver` | `false` | `false` |
| UA | `Chrome/...` (no `HeadlessChrome`) | `HeadlessChrome/...` |
| viewport / screen | coherent `1920x1080` / `1920x1080` | `780x493` / `800x600` |
| WebGL | host AMD/RADV hardware renderer | SwiftShader software renderer |

This does not prove every WAF will prefer VoidCrawl, but it does prove the
current launch/fingerprint posture is at least on par with nodriver and better
on the obvious headless consistency checks.
