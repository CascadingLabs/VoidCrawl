# Driving the MCP through a Docker Chromium farm

By default the `voidcrawl-mcp` server launches Chromium itself on the host, so
the browser inherits whatever the host has — driver versions, GPU state, the
Chromium build. For reproducibility (and consistent, hardware-accelerated
stealth across Intel/AMD/NVIDIA boxes) you can instead run Chromium in a
container and have the MCP **attach** to it over CDP for every call.

The MCP never launches Chromium in this mode; it connects to the containerized
Chrome farm via `CHROME_WS_URLS`. Attached/headful tabs keep the already-running
Chrome's native fingerprint and avoid pre-navigation stealth mutations; the
container supplies the hardware GPU, profile, and launch-surface hygiene baked
into `supervisord.conf` / `voidcrawl.scale`.

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

### DNS override (optional)

If the container inherits an unreachable host stub resolver and Chrome cannot
resolve public sites, set deterministic nameservers at startup:

```bash
VOIDCRAWL_DNS_SERVERS=1.1.1.1,8.8.8.8 \
  docker compose -f docker/docker-compose.yml up -d --build
```

The entrypoint rewrites `/etc/resolv.conf` before Chrome starts. Leave the
variable unset for Docker's normal resolver behavior.

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

Use `CHROME_PROFILES_DIR=/profiles` to persist warm profiles in the named Docker
volume described above. Each Chrome owns one subdirectory under that base. Note
Chrome locks a profile while running — one Chrome per profile dir.

## Headful variant

For a visible (VNC) browser with a real Wayland compositor + GPU, use the
headful container instead — see [docker-headful.md](docker-headful.md). That
path is the better choice for interactive bot-wall challenges that need a human
to solve a CAPTCHA once into a persisted profile.
