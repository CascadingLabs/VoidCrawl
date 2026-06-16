# Driving the MCP through a Docker Chromium farm

By default the `voidcrawl-mcp` server launches Chromium itself on the host, so
the browser inherits whatever the host has — driver versions, GPU state, the
Chromium build. For reproducibility (and consistent, hardware-accelerated
stealth across Intel/AMD/NVIDIA boxes) you can instead run Chromium in a
container and have the MCP **attach** to it over CDP for every call.

The MCP never launches Chromium in this mode; it connects to the containerized
Chrome farm via `CHROME_WS_URLS`. Per-page stealth (UA / Client-Hints /
`navigator.webdriver` patches) is still applied over the attached CDP session —
only the *launch* flags (hardware GPU + anti-automation) come from the
container, where they're baked into the Chrome launch (`supervisord.conf` /
`voidcrawl.scale`).

## 1. Run the Chromium farm

```bash
docker compose -f docker/docker-compose.yml up -d --build
# exposes CDP on http://localhost:9222 and :9223 (host networking)
curl -sf http://localhost:9222/json/version    # sanity check
```

The compose file passes the host GPU through (`devices: /dev/dri`) so the farm
renders on real hardware via Mesa/ANGLE instead of SwiftShader (a bot signal —
see CAS-64). GPU support by vendor:

| GPU | Setup | Notes |
|-----|-------|-------|
| AMD (iGPU/dGPU) | `/dev/dri` passthrough (default) | Mesa RADV. Works out of the box. |
| Intel (iGPU) | `/dev/dri` passthrough (default) | Mesa ANV. Works out of the box. |
| NVIDIA | host `nvidia-container-toolkit` + `gpus: all` | Drop `/dev/dri`; add `deploy.resources.reservations.devices` for NVIDIA. |
| None / unsupported | remove the `devices:` block | Falls back to SwiftShader (software) — works everywhere, but software-rendered. |

If the render node's group differs from `render`, set it: the compose uses
`group_add: ["render"]`; replace with the numeric GID from
`stat -c '%g' /dev/dri/renderD128` if needed.

### Persistent profiles (optional)

By default the farm's Chromes use ephemeral `/tmp` profiles. To persist cookies,
logins, and Cloudflare clearance across restarts, set
`CHROME_PROFILES_DIR=/profiles` — it points the profiles at the
`voidcrawl-headless-profiles` volume the compose file already mounts:

```bash
CHROME_PROFILES_DIR=/profiles docker compose -f docker/docker-compose.yml up -d
# reset the saved profiles: docker compose -f docker/docker-compose.yml down -v
```

Each browser gets its own subdir (`chrome-profile-1`, `chrome-profile-2`, …).

## 2. Point the MCP at the farm

In your host MCP config (`.mcp.json` / Claude Code / Codex / opencode), set
`CHROME_WS_URLS` on the `voidcrawl` server so it attaches instead of launching:

```json
{
  "mcpServers": {
    "voidcrawl": {
      "command": "voidcrawl-mcp",
      "args": [],
      "env": {
        "CHROME_WS_URLS": "http://localhost:9222,http://localhost:9223",
        "TABS_PER_BROWSER": "4"
      }
    }
  }
}
```

With `CHROME_WS_URLS` set, the MCP ignores `CHROME_HEADLESS` / `BROWSER_COUNT`
(it doesn't launch Chrome) and drives the containerized farm for every call.

## 3. Verify the browser is hardware-accelerated

```bash
# from a checkout with the python extension built (./build.sh):
python - <<'PY'
import asyncio, json
from voidcrawl import BrowserConfig, BrowserSession
async def main():
    async with BrowserSession(BrowserConfig(ws_url="http://localhost:9222")) as b:
        p = await b.new_page("https://example.com")
        print(await p.evaluate_js(
            "(()=>{const c=document.createElement('canvas').getContext('webgl');"
            "const d=c.getExtension('WEBGL_debug_renderer_info');"
            "return d?c.getParameter(d.UNMASKED_RENDERER_WEBGL):'no-ext';})()"))
asyncio.run(main())
PY
# Expect e.g. "ANGLE (AMD, Vulkan ... RADV ...), radv" — NOT "...SwiftShader...".
```

## Warm profiles (Cloudflare `cf_clearance`, logins)

The farm's profiles are ephemeral (`/tmp/chrome-profile-N`, reset on container
restart). To persist a warm profile (e.g. to bank a `cf_clearance` cookie or a
login), mount a volume over the profile dir in `docker-compose.yml`:

```yaml
    volumes:
      - ./.voidcrawl-profiles/p1:/tmp/chrome-profile-1
      - ./.voidcrawl-profiles/p2:/tmp/chrome-profile-2
```

Note Chrome locks a profile while running — one Chrome per profile dir.

## Headful variant

For a visible (VNC) browser with a real Wayland compositor + GPU, use the
headful container instead — see [docker-headful.md](docker-headful.md). That
path is the better choice for interactive bot-wall challenges that need a human
to solve a CAPTCHA once into a persisted profile.
