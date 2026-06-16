# Docker Headful Mode

Run Chrome with a real GUI inside Docker — GPU-accelerated, isolated from your desktop, viewable via VNC.

## What's happening under the hood

```
┌─────────────────── Docker container ───────────────────┐
│                                                        │
│  Sway (Wayland compositor)                             │
│    Creates TWO virtual screens in GPU memory — one     │
│    per Chrome instance. A Chrome occluded by another   │
│    fullscreen window gets no frame callbacks and its   │
│    renderer stalls, so each Chrome must own an output. │
│    Runs with WLR_BACKENDS=headless so it doesn't try   │
│    to find a real display.                             │
│                                                        │
│  Chrome x2 (headful, GPU-accelerated)                  │
│    Render pages into their own Sway output using       │
│    --ozone-platform=wayland (routed by --class).      │
│    Use your GPU via /dev/dri passthrough.              │
│    Expose CDP on ports 19222 and 19223.                │
│                                                        │
│  wayvnc x2 (VNC server, one per output)                │
│    Reads pixels from each virtual screen and streams   │
│    them on ports 5900 (chrome-1) / 5901 (chrome-2),    │
│    capped at 30 fps so capture doesn't contend with    │
│    Chrome rendering. Uses --gpu for hardware encoding. │
│                                                        │
│  dbus (system bus)                                     │
│    Without it every Chrome process stalls on dbus      │
│    autolaunch attempts.                                │
└────────────────────────────────────────────────────────┘
         │                              │
         │ /dev/dri (GPU passthrough)   │ ports 5900/5901 (VNC)
         │ port 19222, 19223 (CDP)      │
         ▼                              ▼
┌──── Your host ──────────────────────────────────────────┐
│  void_crawl connects to localhost:19222 via CDP       │
│  VNC client connects to localhost:5900 to watch         │
└─────────────────────────────────────────────────────────┘
```

## Why not just run Chrome headful natively?

- Chrome headful on your desktop steals focus, pops up windows, and interferes with your work
- Docker isolates everything — Chrome runs in its own display server
- You can watch what Chrome sees via VNC, or ignore it entirely
- Same container works in CI/CD, on remote servers, anywhere Docker runs

## Quick start

```bash
# Auto-detects your GPU (AMD/Intel/NVIDIA) and starts everything
./docker/run-headful.sh

# Or specify GPU manually
./docker/run-headful.sh --gpu amd
./docker/run-headful.sh --gpu nvidia
./docker/run-headful.sh --gpu intel
./docker/run-headful.sh --gpu cpu        # no GPU, software rendering
```

Once running, you have three things available:

| Port | What | How to use |
|------|------|------------|
| `localhost:6080` | **noVNC (browser)** | Open **http://localhost:6080** in any browser to **watch Chrome 1** |
| `localhost:6081` | noVNC (browser) | Same for **Chrome 2** (each browser has its own screen) |
| `localhost:5900` | VNC (native) | Or use a VNC client (Remmina, TigerVNC) for lower latency — Chrome 1 |
| `localhost:5901` | VNC (native) | Chrome 2 |
| `localhost:19222` | CDP | void_crawl connects here to control Chrome browser 1 |
| `localhost:19223` | CDP | void_crawl connects here to control Chrome browser 2 |

> **All ports bind `127.0.0.1` only.** wayvnc and noVNC stream a live browser
> that may hold authenticated sessions, with no auth on the wire, so they are
> not exposed off-box. To watch from another machine, forward the port over
> SSH rather than opening the bind:
> `ssh -L 6080:localhost:6080 <host>`, then open `http://localhost:6080` locally.

## Connecting void_crawl to Docker Chrome

```python
import asyncio
import os
from void_crawl import BrowserPool

# Tell the pool to connect to Docker's Chrome instances
os.environ["CHROME_WS_URLS"] = "http://localhost:19222,http://localhost:19223"

async def main():
    async with BrowserPool.from_env() as pool:
        async with pool.acquire() as tab:
            # This navigation happens inside Docker's Chrome.
            # If you have a VNC client open on localhost:5900,
            # you'll see the page load in real time.
            await tab.goto("https://en.wikipedia.org/wiki/Web_scraping")
            print(f"Title: {await tab.title()}")
            print(f"HTML length: {len(await tab.content())} chars")

asyncio.run(main())
```

## Persistent Chrome profiles

By default each Chrome runs with an **ephemeral** profile in `/tmp` — cookies,
logins, and Cloudflare clearance are wiped when the container stops.

To **persist** profiles (log in / clear a wall once, then reuse it across
restarts), set `CHROME_PROFILES_DIR=/profiles`. That points the profiles at the
`voidcrawl-headful-profiles` Docker volume the compose file already mounts:

```bash
CHROME_PROFILES_DIR=/profiles ./docker/run-headful.sh -d
# or compose-native:
CHROME_PROFILES_DIR=/profiles COMPOSE_PROFILES=amd \
  docker compose -f docker/docker-compose.headful.yml up -d
```

Each Chrome gets its own subdir (`chrome-profile-1`, `chrome-profile-2`), so the
two browsers never contend on the same profile lock. Reset the saved profiles
(start fresh) with:

```bash
docker compose -f docker/docker-compose.headful.yml down -v
```

> A persisted profile keeps a real session — handy for sites behind a login or a
> one-time human check, and it lets a cleared Cloudflare cookie survive a
> restart. It also pins one identity; for many isolated identities keep the
> default ephemeral profiles and rotate per run.

## Viewing Chrome

### In your browser (easiest)
noVNC is built into the container. Just open:

```
http://localhost:6080
```

Click **Connect** — you'll see Chrome's windows live inside Sway. No software to install.

### Native VNC client (lower latency)

#### Remmina (Linux)
1. Open Remmina
2. New connection → Protocol: VNC
3. Server: `localhost:5900`
4. Connect

#### TigerVNC viewer (any OS)
```bash
vncviewer localhost:5900
```

> **Note**: VNC uses a binary protocol (RFB), not HTTP. You cannot open `localhost:5900` directly in a browser — that's what noVNC on port 6080 is for.

## Custom resolution

```bash
# 2K resolution
VNC_WIDTH=2560 VNC_HEIGHT=1440 ./docker/run-headful.sh

# 720p for lower memory usage
VNC_WIDTH=1280 VNC_HEIGHT=720 ./docker/run-headful.sh
```

## GPU support matrix

| GPU | Driver | Container setup | Notes |
|-----|--------|----------------|-------|
| AMD iGPU | amdgpu | `/dev/dri` passthrough | Works out of the box. Uses Mesa RADV. |
| AMD discrete | amdgpu | `/dev/dri` passthrough | Same as iGPU |
| Intel iGPU | i915/xe | `/dev/dri` passthrough | Works out of the box. Uses Mesa ANV. |
| NVIDIA | nvidia | `/dev/dri` + `--gpus all` | Needs `nvidia-container-toolkit` on host + `nvidia-drm.modeset=1` kernel param. Sway runs with `--unsupported-gpu`. |
| None | — | No device passthrough | Falls back to `pixman` (CPU software rendering). Slower but works everywhere. |

## Stopping

```bash
# If started in foreground (no -d), just Ctrl+C

# If started detached
docker compose -f docker/docker-compose.headful.yml --profile amd down
```

## Platform support

| Platform | Headful GPU | Headless Docker | Notes |
|----------|------------|----------------|-------|
| **Linux** | Yes | Yes | Full GPU passthrough via `/dev/dri`. This is the primary target. |
| **macOS** | No | Yes | Docker Desktop runs a Linux VM — no GPU passthrough. Use the headless `docker/Dockerfile` instead. |
| **Windows** | No | Yes | Same as macOS — Docker Desktop's VM has no GPU access. `network_mode: host` also behaves differently. Use the headless `docker/Dockerfile` instead. |
| **WSL2** | Partial | Yes | WSL2 has `/dev/dri` for Intel/AMD iGPUs via Mesa, but Docker-in-WSL GPU passthrough is unreliable. Not officially supported. |

The headful GPU container is a **Linux-only** feature. It relies on:
- `/dev/dri` device passthrough (Linux DRM subsystem)
- Sway/wlroots (Linux Wayland compositor)
- `network_mode: host` (Linux Docker only)

For Windows/macOS, use the standard headless Docker setup (`docker/docker-compose.yml`).

## Troubleshooting

**VNC shows black screen**: Sway might not have started yet. Wait a few seconds — wayvnc auto-reconnects.

**Chrome not responding on CDP**: Check `docker logs <container>` for errors. Common cause: port conflict if you have native Chrome also running on 19222.

**High memory usage**: Each headful Chrome instance uses ~300-500 MB more than headless because it maintains a real render tree + GPU buffers. This is expected.
