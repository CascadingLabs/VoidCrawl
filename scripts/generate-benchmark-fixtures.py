#!/usr/bin/env python3
"""Generate and validate the committed loopback fixture manifest and checksum."""

import hashlib
import json
import sys
from pathlib import Path

root = Path(__file__).resolve().parents[1]
fixtures = root / "voidcrawl-benchmarks/fixtures"
fixture = fixtures / "index.html"
sums = fixtures / "SHA256SUMS"
manifest = fixtures / "manifest.json"
html = fixture.read_bytes()
bytes_body = b"x" * 1024
payload = {
    "generator": "scripts/generate-benchmark-fixtures.py",
    "version": 1,
    "fixtures": [
        {
            "route": "/index.html",
            "file": "index.html",
            "size_bytes": len(html),
            "sha256": hashlib.sha256(html).hexdigest(),
            "content_type": "text/html; charset=utf-8",
            "generator": "hand-curated HTML emitted/validated by the fixture generator",
        },
        {
            "route": "/bytes?size=1024",
            "size_bytes": len(bytes_body),
            "sha256": hashlib.sha256(bytes_body).hexdigest(),
            "content_type": "text/plain; charset=utf-8",
            "generator": "loopback server deterministic b'x' body",
        },
    ],
}
sum_text = f"{payload['fixtures'][0]['sha256']}  index.html\n"
manifest_text = json.dumps(payload, indent=2) + "\n"
if "--check" in sys.argv:
    if sums.read_text() != sum_text or manifest.read_text() != manifest_text:
        raise SystemExit(
            "fixture manifest is stale; run scripts/generate-benchmark-fixtures.py"
        )
else:
    sums.write_text(sum_text)
    manifest.write_text(manifest_text)
