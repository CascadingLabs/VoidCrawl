#!/usr/bin/env python3
"""Live demo: log into a real (local) htmx site through VoidCrawl, observe the
credentials two ways, then replay them as direct HTTP requests with no browser.

Exercises the tools added for the CAS-149/CAS-237 prep work:
  - network_capture_arm / network_capture_wait  (CDP request+response capture)
  - session_cookies                             (interim, raw cookie read)

It also demonstrates the redaction boundary. After logging in, the page makes an
authenticated `fetch('/me')` carrying `Authorization: Bearer <jwt>`; that
request's headers are captured twice — once with the default redaction, once
with raw access granted — so you can see `<redacted>` actually replacing a live
credential rather than take the claim on faith.

Note the login POST itself carries no credential (it is the request that
establishes one), which is why the capture targets the *subsequent* call.

Raw credential access (session_cookies, and include_sensitive_headers) is gated
on VOIDCRAWL_ALLOW_CREDENTIAL_CAPTURE=1; this script sets it for its own child
process so the demo is self-contained.

Run:
    cd examples/jwt_login_demo
    python3 demo.py [path-to-voidcrawl-mcp-binary]

Defaults to ../../target/debug/voidcrawl-mcp relative to this file.
"""

import base64
import contextlib
import json
import os
import signal
import subprocess
import sys
import threading
import urllib.error
import urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from login_site import serve

DEFAULT_BIN = Path(__file__).parent / "../../target/debug/voidcrawl-mcp"
BASE = "http://127.0.0.1:8901"


class McpClient:
    """Minimal MCP stdio JSON-RPC client — enough to drive tool calls."""

    def __init__(self, binary: Path, *, allow_raw: bool) -> None:
        env = dict(os.environ)
        if allow_raw:
            env["VOIDCRAWL_ALLOW_CREDENTIAL_CAPTURE"] = "1"
        else:
            env.pop("VOIDCRAWL_ALLOW_CREDENTIAL_CAPTURE", None)
        # start_new_session so close() can sweep the whole Chrome tree: Chrome
        # is torn down asynchronously over CDP, and killing only the server
        # would orphan it to init.
        self.proc = subprocess.Popen(
            [str(binary)],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            # The server logs to stderr; a filled pipe buffer would block it.
            stderr=subprocess.DEVNULL,
            text=True,
            bufsize=1,
            start_new_session=True,
            env=env,
        )
        self._id = 0
        self._request(
            "initialize",
            {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "jwt-login-demo", "version": "0.0.1"},
            },
        )
        self._write({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def _write(self, msg: dict) -> None:
        self.proc.stdin.write(json.dumps(msg) + "\n")
        self.proc.stdin.flush()

    def _request(self, method: str, params: dict) -> dict:
        self._id += 1
        self._write(
            {"jsonrpc": "2.0", "id": self._id, "method": method, "params": params}
        )
        while True:
            line = self.proc.stdout.readline()
            if not line:
                raise RuntimeError("voidcrawl-mcp exited unexpectedly")
            try:
                msg = json.loads(line)
            except json.JSONDecodeError:
                continue
            if msg.get("id") == self._id:
                return msg

    def call(self, tool: str, args: dict) -> dict:
        resp = self._request("tools/call", {"name": tool, "arguments": args})
        if "error" in resp:
            raise RuntimeError(f"{tool} failed: {resp['error']}")
        result = resp["result"]
        if result.get("isError"):
            raise RuntimeError(f"{tool} tool error: {result}")
        return json.loads(result["content"][0]["text"])

    def close(self) -> None:
        """Close stdin (the server's shutdown signal), then sweep the group.

        killpg only runs while the child is still alive, so a reaped PID can't
        be reused by an unrelated process group in the meantime.
        """
        pgid = os.getpgid(self.proc.pid)
        with contextlib.suppress(BrokenPipeError, OSError):
            self.proc.stdin.close()
        with contextlib.suppress(subprocess.TimeoutExpired):
            self.proc.wait(timeout=3)
        if self.proc.poll() is None:
            with contextlib.suppress(ProcessLookupError, subprocess.TimeoutExpired):
                os.killpg(pgid, signal.SIGTERM)
                self.proc.wait(timeout=2)
        if self.proc.poll() is None:
            with contextlib.suppress(ProcessLookupError):
                os.killpg(pgid, signal.SIGKILL)


def http_get(url: str, headers: dict) -> tuple[int, str]:
    req = urllib.request.Request(url, headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=5) as resp:
            return resp.status, resp.read().decode()
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode()


def wait_for_htmx(client: "McpClient", sid: str) -> None:
    """Block until htmx has initialised and bound the form's hx-post.

    Without this the click can land before htmx.min.js has executed, in which
    case the form falls back to a native submit (a GET to `/`, since it has no
    method/action) and the expected POST to /login never happens — an
    intermittent, load-dependent capture timeout that looks like a tool bug.

    Event-driven: network-idle, then assert the global is actually present.
    """
    client.call("wait_for_network_idle", {"session_id": sid, "timeout_secs": 10})
    ready = client.call(
        "eval_js",
        {"session_id": sid, "expression": "typeof window.htmx !== 'undefined'"},
    )
    if ready.get("value") is not True:
        raise RuntimeError("htmx did not initialise; the form would submit natively")


def run_flow(binary: Path, *, raw: bool) -> dict:
    """Log in, then capture an authenticated request that carries the bearer."""
    client = McpClient(binary, allow_raw=raw)
    try:
        sid = client.call("session_open", {"headless": True})["session_id"]
        client.call("session_navigate", {"session_id": sid, "url": f"{BASE}/"})
        wait_for_htmx(client, sid)

        # Log in. This POST mints the JWT; it carries no credential itself.
        client.call(
            "network_capture_arm",
            {
                "session_id": sid,
                "patterns": [{"name": "login", "url_glob": "**/login"}],
                "capture_body": True,
            },
        )
        client.call(
            "type_text", {"session_id": sid, "selector": "#username", "text": "demo"}
        )
        client.call(
            "type_text",
            {"session_id": sid, "selector": "#password", "text": "demo123"},
        )
        client.call("click", {"session_id": sid, "selector": "#login-btn"})
        login = client.call(
            "network_capture_wait", {"session_id": sid, "timeout_secs": 20}
        )["captures"]["login"]
        bearer = (
            base64.b64decode(login["body_base64"])
            .decode()
            .split('data-token="')[1]
            .split('"')[0]
        )

        # Now capture an AUTHENTICATED call — this one carries the bearer, so it
        # is what actually exercises redaction.
        client.call(
            "network_capture_arm",
            {
                "session_id": sid,
                "patterns": [{"name": "me", "url_glob": "**/me*"}],
                "include_sensitive_headers": raw,
            },
        )
        client.call(
            "eval_js",
            {
                "session_id": sid,
                "expression": (
                    f"fetch('/me', {{headers:{{Authorization:'Bearer {bearer}'}}}})"
                    ".then(r=>r.text())"
                ),
            },
        )
        me = client.call(
            "network_capture_wait", {"session_id": sid, "timeout_secs": 20}
        )["captures"]["me"]

        # Value-free cookie handoff: the lease reports everything a caller needs
        # to decide whether replay is safe, and no cookie value crosses the wire.
        lease = client.call(
            "cookie_lease_open",
            {"session_id": sid, "replay_origin": BASE},
        )

        cookie = None
        if raw:
            # Reading the actual value still needs the gated raw tool, because
            # applying a lease to an outbound request is caller-side and not
            # built here. That split is the point, not an oversight.
            jar = client.call("session_cookies", {"session_id": sid})["cookies"]
            cookie = next(c for c in jar if c["name"] == "session")
        client.call(
            "cookie_lease_revoke",
            {"session_id": sid, "lease_id": lease["lease_id"], "reason": "demo_done"},
        )
        client.call("session_close", {"session_id": sid})
        return {"me": me, "bearer": bearer, "cookie": cookie, "lease": lease}
    finally:
        client.close()


def show_headers(label: str, headers: list) -> None:
    print(f"\n--- request headers of the authenticated GET /me — {label} ---")
    for name, value in sorted(headers):
        if is_credential_header(name):
            print(f"  {name}: {value}   <-- credential-bearing")


def is_credential_header(name: str) -> bool:
    return any(n in name for n in ("authorization", "cookie", "token"))


def show_lease(lease: dict) -> None:
    """Print the lease's value-free provenance and prove no value is in it."""
    print("\n--- cookie_lease_open: value-free provenance ---")
    print(f"  lease {lease['lease_id'][:8]}…  origin {lease['replay_origin']}")
    print(f"  observed in: {lease['observed_in']}")
    for cookie in lease["cookies"]:
        print(
            f"    {cookie['name']}: httpOnly={cookie['http_only']} "
            f"secure={cookie['secure']} sameSite={cookie['same_site']} "
            f"session={cookie['is_session_cookie']} has_value={cookie['has_value']}"
        )
        print(
            f"      issuing_origin={cookie['issuing_origin']} "
            f"top_level_site={cookie['top_level_site']}"
        )
    # The invariant, checked rather than asserted in prose.
    blob = json.dumps(lease)
    leaked = "eyJhbGciOi" in blob
    print(f"  JWT value anywhere in this payload? {leaked}")
    if leaked:
        raise RuntimeError("cookie value leaked into lease provenance")


def main() -> None:
    binary = Path(sys.argv[1]) if len(sys.argv) > 1 else DEFAULT_BIN
    if not binary.exists():
        sys.exit(
            f"voidcrawl-mcp not found at {binary}\n"
            "build it with: cargo build -p voidcrawl-mcp"
        )

    server = serve(port=8901)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    print(f"[site] demo login page at {BASE}/")

    try:
        # Pass 1: defaults. Credential header values must come back redacted.
        print("\n=== PASS 1: default (redaction on, no env opt-in) ===")
        redacted = run_flow(binary, raw=False)
        show_headers("DEFAULT", redacted["me"]["request_headers"])

        # Pass 2: raw access granted, to prove pass 1 was really hiding a value.
        print(
            "\n=== PASS 2: VOIDCRAWL_ALLOW_CREDENTIAL_CAPTURE=1 "
            "+ include_sensitive_headers ==="
        )
        raw = run_flow(binary, raw=True)
        show_headers("RAW", raw["me"]["request_headers"])

        show_lease(raw["lease"])

        bearer = raw["bearer"]
        cookie = raw["cookie"]
        print("\n--- credentials obtained ---")
        print(f"  bearer JWT (from response BODY): {bearer[:38]}...")
        print(
            f"  session cookie (from cookie store): {cookie['value'][:38]}... "
            f"httpOnly={cookie['httpOnly']}"
        )

        print("\n=== direct HTTP replay — no browser from here ===\n")
        status, resp = http_get(f"{BASE}/me", {})
        print(f"  GET /me  no auth               -> {status} {resp}")
        status, resp = http_get(f"{BASE}/me", {"Cookie": f"session={cookie['value']}"})
        print(f"  GET /me  replayed cookie       -> {status} {resp}")
        status, resp = http_get(f"{BASE}/me", {"Authorization": f"Bearer {bearer}"})
        print(f"  GET /me  replayed bearer JWT   -> {status} {resp}")
    finally:
        server.shutdown()


if __name__ == "__main__":
    main()
