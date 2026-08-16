"""Record an OpenSesame/noVNC login without putting credentials in the log.

This example is the evidence-producing wrapper around an external OpenSesame
actor. VoidCrawl owns the browser session and recorder; OpenSesame receives
same-tab attach coordinates and performs the authorized login, either through
a command or through a human using noVNC.

Docker headful/noVNC example::

    ./docker/run-headful.sh -d
    ffmpeg -version
    uv run python examples/deployment/opensesame_recorded_novnc_login.py \
        --docker-headful \
        --docker-version-url http://127.0.0.1:19222/json/version \
        --url "$AUTHORIZED_LOGIN_URL"

Set ``AUTHORIZED_LOGIN_URL`` to a real, authorized login URL before running;
``example.test`` is intentionally not a reachable site.

With an OpenSesame actor command::

    uv run python examples/deployment/opensesame_recorded_novnc_login.py \
        --docker-headful --url "$AUTHORIZED_LOGIN_URL" \
        --opensesame-command "python /path/to/opensesame_actor.py"

The actor receives ``VOIDCRAWL_OPENSESAME_ATTACH_JSON`` pointing to a JSON
file containing ``websocket_url``, ``target_id``, ``session_id``, and the
noVNC/VNC links. Without ``--opensesame-command``, the script waits for the
human to finish in noVNC. The output directory contains:

* ``recording.mp4`` (the obfuscated video, when ffmpeg is available),
* ``frames/`` (PNG evidence frames),
* ``audit.jsonl`` (lifecycle, attach, provenance, and recording facts), and
* ``opensesame_attach.json`` (non-secret attach coordinates).

The default mask is ``input[type=password]``. Add ``--mask-selector`` for
other sensitive fields. The script never writes HTML, cookies, typed values,
or command arguments to the audit log.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import shlex
import urllib.request
import uuid
from datetime import datetime, timezone
from pathlib import Path
from typing import Any
from urllib.parse import urlsplit

from voidcrawl import BrowserConfig, BrowserSession, NavigationError

DEFAULT_NOVNC_URL = "http://127.0.0.1:6080"
DEFAULT_VNC_URL = "vnc://127.0.0.1:5900"
DEFAULT_DOCKER_VERSION_URL = "http://127.0.0.1:19222/json/version"


class AuditLog:
    """Append-only JSONL audit log with deliberately narrow fields."""

    def __init__(self, path: Path) -> None:
        self.path = path

    def write(self, event: str, **fields: Any) -> None:
        record = {
            "timestamp": datetime.now(timezone.utc).isoformat(),
            "event": event,
            **fields,
        }
        with self.path.open("a", encoding="utf-8") as stream:
            json.dump(record, stream, sort_keys=True)
            stream.write("\n")


def safe_url(value: str) -> str:
    """Keep origin/path provenance while dropping query, fragment, and userinfo."""
    parsed = urlsplit(value)
    if not parsed.scheme or not parsed.hostname:
        return "<unavailable>"
    host = parsed.hostname
    if parsed.port is not None:
        host = f"{host}:{parsed.port}"
    return f"{parsed.scheme}://{host}{parsed.path}"


def antibot_dict(verdict: Any) -> dict[str, Any] | None:
    if verdict is None:
        return None
    return {
        "vendors": list(getattr(verdict, "vendors", []) or []),
        "challenged": bool(getattr(verdict, "challenged", False)),
        "challenge_vendor": getattr(verdict, "challenge_vendor", None),
        "corpus_version": getattr(verdict, "corpus_version", None),
        "evidence": getattr(verdict, "evidence", None),
    }


def safe_response_provenance(response: Any) -> dict[str, Any]:
    """Keep replay-useful response facts without copying arbitrary headers."""
    allowed_headers = (
        "content-type",
        "date",
        "server",
        "cf-ray",
        "x-cache",
        "x-request-id",
    )
    headers = getattr(response, "headers", {}) or {}
    return {
        "url": safe_url(getattr(response, "url", "")),
        "status_code": getattr(response, "status_code", None),
        "redirected": bool(getattr(response, "redirected", False)),
        "headers": {key: headers[key] for key in allowed_headers if key in headers},
        "endpoints": getattr(response, "endpoints", None),
        "endpoints_truncated": bool(getattr(response, "endpoints_truncated", False)),
        "endpoint_sanitizer_version": getattr(
            response, "endpoint_sanitizer_version", None
        ),
        "antibot": antibot_dict(getattr(response, "antibot", None)),
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--url", required=True, help="Login URL to open in the recorded tab"
    )
    parser.add_argument("--output-dir", default=None)
    parser.add_argument("--duration-secs", type=float, default=120.0)
    parser.add_argument("--fps", type=int, default=10)
    parser.add_argument(
        "--mask-selector",
        action="append",
        dest="mask_selectors",
        help=(
            "CSS selector to mask; repeat for multiple fields "
            "(default: input[type=password])"
        ),
    )
    parser.add_argument("--mask-pad", type=int, default=2)
    parser.add_argument(
        "--novnc-url", default=os.environ.get("VOIDCRAWL_NOVNC_URL", DEFAULT_NOVNC_URL)
    )
    parser.add_argument(
        "--vnc-url", default=os.environ.get("VOIDCRAWL_VNC_URL", DEFAULT_VNC_URL)
    )
    parser.add_argument(
        "--session-id", default=None, help="Correlation id supplied to OpenSesame"
    )
    parser.add_argument("--docker-headful", action="store_true")
    parser.add_argument("--docker-version-url", default=DEFAULT_DOCKER_VERSION_URL)
    parser.add_argument(
        "--opensesame-command",
        help=(
            "External actor command as one shell-like string; attach JSON path "
            "is passed in VOIDCRAWL_OPENSESAME_ATTACH_JSON"
        ),
    )
    return parser.parse_args()


def resolve_docker_ws_url(version_url: str) -> str:
    """Resolve Docker's CDP endpoint using the standard library."""
    with urllib.request.urlopen(version_url, timeout=3) as response:
        payload = json.loads(response.read().decode("utf-8"))
    websocket_url = payload.get("webSocketDebuggerUrl")
    if not isinstance(websocket_url, str) or not websocket_url:
        raise RuntimeError(f"no webSocketDebuggerUrl in {version_url}")
    return websocket_url


def browser_config(args: argparse.Namespace) -> BrowserConfig:
    if args.docker_headful:
        return BrowserConfig(
            ws_url=resolve_docker_ws_url(args.docker_version_url), headless=False
        )
    return BrowserConfig(headless=False)


async def wait_for_human(novnc_url: str) -> None:
    print(
        f"Open {novnc_url} and complete the authorized OpenSesame login "
        "in the same tab."
    )
    await asyncio.to_thread(input, "Press Enter only after the login is complete... ")


async def run_actor(
    command: list[str] | None,
    attach_path: Path,
    novnc_url: str,
) -> None:
    if not command:
        await wait_for_human(novnc_url)
        return

    env = os.environ.copy()
    env["VOIDCRAWL_OPENSESAME_ATTACH_JSON"] = str(attach_path)
    print("Starting OpenSesame actor with the attach manifest")
    process = await asyncio.create_subprocess_exec(*command, env=env)
    return_code = await process.wait()
    if return_code != 0:
        raise RuntimeError(f"OpenSesame actor exited with status {return_code}")


async def main() -> None:
    args = parse_args()
    run_id = args.session_id or uuid.uuid4().hex
    output_dir = Path(args.output_dir or f"output/opensesame-login/{run_id}")
    output_dir.mkdir(parents=True, exist_ok=True)
    log = AuditLog(output_dir / "audit.jsonl")
    mask_selectors = args.mask_selectors or ["input[type=password]"]
    masks = [{"type": "css", "value": selector} for selector in mask_selectors]
    actor_command = (
        shlex.split(args.opensesame_command) if args.opensesame_command else None
    )

    config = browser_config(args)
    log.write(
        "session_starting",
        session_id=run_id,
        url=safe_url(args.url),
        output_dir=str(output_dir),
        mask_selectors=mask_selectors,
        mask_pad=args.mask_pad,
        opensesame_actor=bool(actor_command),
    )

    async with BrowserSession(config) as browser:
        page = await browser.new_page_in_window("about:blank")
        try:
            response = await page.goto(args.url, capture_endpoints=True)
        except NavigationError as exc:
            log.write(
                "navigation_failed",
                url=safe_url(args.url),
                error_type=type(exc).__name__,
            )
            raise SystemExit(
                f"navigation failed for {args.url!r}; use a real authorized URL "
                "instead of the documentation placeholder"
            ) from exc
        target_id = await page.target_id()
        websocket_url = await browser.websocket_url()
        attach = {
            "session_id": run_id,
            "websocket_url": websocket_url,
            "target_id": target_id,
            "novnc_url": args.novnc_url,
            "vnc_url": args.vnc_url,
        }
        attach_path = output_dir / "opensesame_attach.json"
        attach_path.write_text(json.dumps(attach, indent=2) + "\n", encoding="utf-8")
        log.write(
            "session_started", **attach, provenance=safe_response_provenance(response)
        )

        print(
            json.dumps(
                {"attach": attach, "provenance": safe_response_provenance(response)},
                indent=2,
            )
        )
        handle = await page.start_recording(
            duration_secs=args.duration_secs,
            fps=args.fps,
            output_dir=str(output_dir),
            frame_format="png",
            write_frames=True,
            masks=masks,
            mask_pad=args.mask_pad,
            encode=["mp4"],
        )
        log.write(
            "recording_started",
            format="png",
            encode=["mp4"],
            alone_in_window=await page.alone_in_window(),
        )

        recording = None
        try:
            await run_actor(actor_command, attach_path, args.novnc_url)
            log.write(
                "opensesame_login_complete",
                final_url=safe_url(await page.url()),
                title=await page.title(),
                credentials_logged=False,
            )
        except Exception as exc:
            log.write("opensesame_login_failed", error_type=type(exc).__name__)
            raise
        finally:
            recording = await handle.stop()
            region = recording.regions[0]
            log.write(
                "recording_finished",
                duration_ms=recording.duration_ms,
                frames_captured=recording.frames_captured,
                frames_dropped=recording.frames_dropped,
                effective_fps=recording.effective_fps(),
                foregrounded=recording.foregrounded,
                device_pixel_ratio=recording.device_pixel_ratio,
                masks=[
                    {
                        "label": mask.label,
                        "bbox": mask.bbox,
                        "tracked": mask.tracked,
                        "unresolved_ticks": mask.unresolved_ticks,
                        "stale_frames": mask.stale_frames,
                    }
                    for mask in recording.masks
                ],
                outputs=list(region.outputs),
                credentials_logged=False,
            )
            print(
                "Obfuscated video: "
                f"{region.outputs[0] if region.outputs else '<encoding failed>'}"
            )
            print(f"Audit log: {output_dir / 'audit.jsonl'}")


if __name__ == "__main__":
    asyncio.run(main())
