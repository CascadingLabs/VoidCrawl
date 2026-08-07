# Rootless Docker Headful Mode

Run GPU-accelerated Chrome in an isolated Sway compositor. The image runs as
UID/GID `10001:10001`; its generated Sway/Supervisor configuration, sockets,
D-Bus state, logs, and default Chrome profiles live under `/tmp/voidcrawl`.
It does not mutate `/etc`, `/run`, `/var`, users, or groups at startup.

## Start Chrome

```bash
./docker/run-headful.sh                 # auto-detect GPU
./docker/run-headful.sh --gpu amd
./docker/run-headful.sh --gpu cpu       # software renderer
```

The compose file runs with `cap_drop: ALL` and `no-new-privileges`. For GPU
profiles, grant the image user access on the host instead of changing groups in
the container:

```bash
HOST_RENDER_GID=$(stat -c %g /dev/dri/renderD128) ./docker/run-headful.sh
```

Chrome instances and their Sway outputs are generated from `BROWSER_COUNT`.
CDP ports are deterministic: browser *n* listens on
`CDP_PORT_BASE + n - 1` (defaults `19222`, `19223`, …).

```bash
BROWSER_COUNT=3 CDP_PORT_BASE=19222 ./docker/run-headful.sh
export CHROME_WS_URLS=http://127.0.0.1:19222,http://127.0.0.1:19223,http://127.0.0.1:19224
```

## Local on-demand viewer

The viewer is disabled by default. In default mode no `wayvnc` or `websockify`
process starts and no VNC/noVNC socket listens.

Enable the local control mechanism when starting the one container:

```bash
VIEWER_MODE=local ./docker/run-headful.sh -d
```

Open a short-lived viewer for one browser:

```bash
./docker/viewer.sh open --browser 2 --ttl 15m
# viewer URL: http://127.0.0.1:6080/vnc.html?autoconnect=true&path=websockify%3Ftoken%3D...
```

The returned URL contains a random lease token. It starts one loopback-only
`wayvnc` backend and one shared loopback noVNC/websockify listener. Token target
selection maps that URL to the selected browser; there are no per-browser
noVNC ports. Opening another lease revokes the first. Either expiry or:

```bash
./docker/viewer.sh close
```

stops both processes and removes the token state. Native VNC is never published
by Compose; the temporary loopback `wayvnc` socket is only the backend for the
active noVNC lease. Do not expose or forward the noVNC port without protecting
the returned URL: it controls a live authenticated browser.

Useful controls:

| Variable | Default | Meaning |
|---|---:|---|
| `VIEWER_MODE` | `disabled` | `local` enables `docker/viewer.sh`; it does not start a listener. |
| `VIEWER_NOVNC_PORT` | `6080` | One loopback noVNC port while a lease is active. |
| `VIEWER_TTL` | `15m` | Default lease duration (`s`, `m`, `h`, `d`). |
| `VIEWER_MAX_FPS` | `30` | Capture cap while a viewer is active. |
| `VNC_PORT_BASE` | `5900` | Internal loopback backend port base; never published. |
| `VNC_WIDTH`, `VNC_HEIGHT` | `1920`, `1080` | Generated Sway output resolution. |

Viewing consumes compositor readback and encoding resources. Keep viewers
short-lived, lower `VIEWER_MAX_FPS` or the output resolution on loaded hosts,
and expect active viewing to compete with browser rendering (not CDP health).

## Profiles

Profiles are ephemeral by default. To persist logins/cookies, prepare the named
volume with the image identity, then opt in:

```bash
docker volume create voidcrawl-headful-profiles
docker run --rm -u 0:0 -v voidcrawl-headful-profiles:/profiles busybox \
  chown 10001:10001 /profiles
CHROME_PROFILES_DIR=/profiles ./docker/run-headful.sh -d
```

This ownership setup is a host/operator prerequisite; the rootless entrypoint
will not repair it.

## Scope

This is a local-Docker-only viewer. Central proxies, Pod agents, Tailnet
ingress, NetworkPolicy, Helm, and Nimbal/Kubernetes topologies are future work,
not supported deployment modes for this image.
