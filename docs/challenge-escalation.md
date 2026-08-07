# Challenge Escalation With a Local noVNC Lease

VoidCrawl owns challenge detection and the live browser session. Resolvers act
on the same tab through a neutral event contract. V1 is intentionally manual:
obtain a short-lived local noVNC lease, clear the wall, mark the event resolved,
then resume.

## Phases

| Phase | Resolver | What ships |
|---|---|---|
| 1 | `manual_vnc` | Detect active challenges, expose CDP attach coordinates and an optional noVNC lease URL, then wait for a human to clear the wall. |
| 2 | `rotate_identity` / `fail` | If manual resolution is not allowed or fails, return structured evidence so the caller can rotate profile/proxy or stop cleanly. |
| 3 | `yosoi_recipe`, `open_sesame_session_actor`, `agent_mcp` | Automated resolvers attach to the same VoidCrawl session through MCP using `{ websocket_url, target_id, session_id }`. They do not launch a fresh browser. |

Presence-only CDN signals are telemetry. Only active challenges block.

## Start a headful browser and obtain a noVNC lease

From a VoidCrawl checkout, enable the disabled-by-default local viewer control:

```bash
VIEWER_MODE=local ./docker/run-headful.sh -d
./docker/viewer.sh open --browser 1 --ttl 15m
```

Open the returned tokenized noVNC URL. It is the only supported manual viewer;
there is no published native-VNC endpoint. See [Docker Headful Mode](docker-headful.md)
for expiry, close, and resource controls.

If the browser looks tiny or huge, restart with a fixed resolution:

```bash
VNC_WIDTH=1280 VNC_HEIGHT=720 docker compose -f docker/docker-compose.headful.yml up
```

## Start the MCP server with operator links

Set the operator URLs so `capture_challenge` includes handoff links:

```bash
VOIDCRAWL_NOVNC_URL="$(./docker/viewer.sh open --browser 1 --ttl 15m | sed -n 's/^viewer URL: //p')" \
uv run voidcrawl-mcp
```

If your MCP client starts `voidcrawl-mcp` for you, put the current lease URL in
its MCP config. A lease expires; request a new one before handing off another
challenge.

## Manual challenge flow

1. Open a session in headful mode.

```json
{
  "headful": true,
  "port": 9222
}
```

The result includes:

```json
{
  "session_id": "SESSION_ID",
  "websocket_url": "ws://127.0.0.1:9222/devtools/browser/...",
  "target_id": "TARGET_ID"
}
```

2. Navigate to the target.

```json
{
  "session_id": "SESSION_ID",
  "url": "https://example.com",
  "timeout_secs": 30
}
```

3. Capture the challenge event.

```json
{
  "session_id": "SESSION_ID",
  "novnc_url": "LEASE_URL_RETURNED_BY_VIEWERCTL"
}
```

The result has the fields a resolver needs:

```json
{
  "challenge": {
    "event_id": "EVENT_ID",
    "blocking": true,
    "antibot": {
      "vendors": ["cloudflare"],
      "challenged": true,
      "challenge_vendor": "cloudflare",
      "corpus_version": "cl-2026.06.01",
      "evidence": "headers"
    },
    "dom_captcha": {
      "kind": "turnstile",
      "widget_rendered": true,
      "active": true
    },
    "attach_coordinates": {
      "websocket_url": "ws://127.0.0.1:9222/devtools/browser/...",
      "target_id": "TARGET_ID",
      "session_id": "SESSION_ID",
      "novnc_url": "LEASE_URL_RETURNED_BY_VIEWERCTL"
    }
  }
}
```

4. Obtain a current lease URL with `./docker/viewer.sh open --browser <n> --ttl
   15m`, open it, solve the challenge in the visible browser, and wait until the
   target page continues.

5. Mark it resolved.

```json
{
  "session_id": "SESSION_ID",
  "event_id": "EVENT_ID",
  "resolver": "manual_vnc",
  "note": "operator cleared Turnstile in noVNC"
}
```

6. Wait for the resolution. This re-probes by default.

```json
{
  "session_id": "SESSION_ID",
  "event_id": "EVENT_ID",
  "timeout_secs": 10
}
```

Expected result:

```json
{
  "resolved": true,
  "captcha_still_present": false
}
```

7. Continue normally.

```json
{
  "session_id": "SESSION_ID",
  "selector": "main"
}
```

8. Close the session when done.

```json
{
  "session_id": "SESSION_ID"
}
```

## Resolver hook

Automated resolvers use the same event:

```json
{
  "event_id": "EVENT_ID",
  "resolver": "open_sesame_session_actor",
  "attach_coordinates": {
    "websocket_url": "ws://127.0.0.1:9222/devtools/browser/...",
    "target_id": "TARGET_ID",
    "session_id": "SESSION_ID"
  }
}
```

Rules for phase 3:

- Yosoi recipes replay only verified, domain-scoped actions.
- OpenSesame session actors attach to `{ websocket_url, target_id }` and act in
  the existing tab.
- Agent MCP resolvers receive the same coordinates and must mark the event
  resolved or failed.
- Automatic resolution is opt-in and domain-scoped.
- If a resolver cannot clear the wall, call `mark_challenge_failed` and rotate
  identity or fail with evidence.
