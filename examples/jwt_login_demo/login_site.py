"""Tiny self-contained htmx site that mints a JWT on login.

Stdlib-only demo target for `demo.py`. The HS256 signing here is intentionally
minimal so the mechanics stay readable — it is NOT production crypto guidance;
use a vetted JWT library for anything real.

Routes:
  GET  /            - htmx login form
  POST /login       - checks demo/demo123, mints a JWT, sets it as an HttpOnly
                      session cookie, AND echoes it into the swapped-in HTML
                      fragment (real apps often do both so client-side code can
                      use the token)
  GET  /me          - protected: accepts either the session cookie or an
                      `Authorization: Bearer <jwt>` header; 401 otherwise
  GET  /htmx.min.js - vendored htmx.org 1.9.12 (Zero-Clause BSD; see
                      htmx.LICENSE), served locally so the demo needs no
                      external network
"""

import base64
import contextlib
import hashlib
import hmac
import json
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs

SECRET = b"demo-signing-secret-do-not-use-in-prod"
USERNAME = "demo"
PASSWORD = "demo123"
HTMX_JS = (Path(__file__).parent / "htmx.min.js").read_bytes()


def _b64url(data: bytes) -> str:
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode()


def mint_jwt(sub: str, ttl_seconds: int = 300) -> str:
    header = _b64url(json.dumps({"alg": "HS256", "typ": "JWT"}).encode())
    payload = _b64url(
        json.dumps({"sub": sub, "exp": int(time.time()) + ttl_seconds}).encode()
    )
    signing_input = f"{header}.{payload}".encode()
    sig = _b64url(hmac.new(SECRET, signing_input, hashlib.sha256).digest())
    return f"{header}.{payload}.{sig}"


def verify_jwt(token: str) -> str | None:
    """Return the subject, or None if the token is invalid/expired.

    Fails closed: any malformed input returns None rather than raising.
    """
    try:
        header, payload, sig = token.split(".")
        expected = _b64url(
            hmac.new(SECRET, f"{header}.{payload}".encode(), hashlib.sha256).digest()
        )
        if not hmac.compare_digest(sig, expected):
            return None
        claims = json.loads(base64.urlsafe_b64decode(payload + "=="))
    except (ValueError, TypeError, json.JSONDecodeError):
        return None
    if claims.get("exp", 0) < time.time():
        return None
    return claims.get("sub")


LOGIN_PAGE = b"""<!doctype html>
<html><head><meta charset="utf-8"><script src="/htmx.min.js"></script></head>
<body>
<h1>Demo Login</h1>
<form hx-post="/login" hx-target="#result" hx-swap="innerHTML">
  <input id="username" name="username" placeholder="username">
  <input id="password" name="password" type="password" placeholder="password">
  <button id="login-btn" type="submit">Log in</button>
</form>
<div id="result"></div>
</body></html>"""

UNAUTHORIZED = json.dumps({"error": "unauthorized"}).encode()


class Handler(BaseHTTPRequestHandler):
    def _send(self, status: int, body: bytes, content_type: str = "text/html") -> None:
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _subject(self) -> str | None:
        """Authenticate via bearer header first, then the session cookie."""
        auth = self.headers.get("Authorization", "")
        if auth.startswith("Bearer "):
            sub = verify_jwt(auth.removeprefix("Bearer "))
            if sub:
                return sub
        for raw in self.headers.get("Cookie", "").split(";"):
            part = raw.strip()
            if part.startswith("session="):
                sub = verify_jwt(part.removeprefix("session="))
                if sub:
                    return sub
        return None

    def do_GET(self) -> None:
        if self.path == "/":
            self._send(200, LOGIN_PAGE)
        elif self.path == "/htmx.min.js":
            self._send(200, HTMX_JS, "application/javascript")
        elif self.path == "/me":
            sub = self._subject()
            if sub:
                body = json.dumps({"user": sub, "authenticated": True}).encode()
                self._send(200, body, "application/json")
            else:
                self._send(401, UNAUTHORIZED, "application/json")
        else:
            self.send_error(404)

    def do_POST(self) -> None:
        if self.path != "/login":
            self.send_error(404)
            return
        length = int(self.headers.get("Content-Length", 0))
        # parse_qs handles percent-encoding and `+`, which a naive split does not.
        form = parse_qs(self.rfile.read(length).decode())
        username = form.get("username", [""])[0]
        password = form.get("password", [""])[0]
        if username != USERNAME or password != PASSWORD:
            self._send(401, b'<div id="result">Invalid credentials</div>')
            return

        token = mint_jwt(USERNAME)
        body = (
            f'<div id="result" data-token="{token}">Logged in as {USERNAME}</div>'
        ).encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/html")
        self.send_header("Content-Length", str(len(body)))
        self.send_header(
            "Set-Cookie", f"session={token}; HttpOnly; Path=/; SameSite=Lax"
        )
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format: str, *args: object) -> None:  # noqa: A002
        pass


def serve(port: int = 8901) -> ThreadingHTTPServer:
    return ThreadingHTTPServer(("127.0.0.1", port), Handler)


if __name__ == "__main__":
    srv = serve()
    print(f"listening on http://127.0.0.1:{srv.server_address[1]}")
    with contextlib.suppress(KeyboardInterrupt):
        srv.serve_forever()
