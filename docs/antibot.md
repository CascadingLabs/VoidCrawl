# Anti-bot / CDN Vendor Detection

`voidcrawl` fingerprints **which** anti-bot / WAF / CDN vendor is gating a
fetched response, and whether that vendor is **actively challenging** us — so a
pipeline can route deterministically (rotate proxy/profile, go headful) instead
of retrying blind.

This is the **inbound** half of the problem — *which wall is in front of us*.
It is deliberately separate from the **outbound** half — *do we look like a
bot* — which is what [stealth patching](stealth.md) addresses. A clean outbound
fingerprint is the upstream cause of *not* being challenged here.

## Two detectors, one vocabulary

| | Signature detector (this doc) | DOM detector ([captcha-detection.md](captcha-detection.md)) |
|---|---|---|
| Input | Raw response: status + headers + bounded body | Live DOM, post-render |
| When | The instant bytes arrive, no JS needed | After navigation + JS execution |
| Answers | *Which vendor, and is it walling me?* | *Is there a solvable widget here?* |
| Coverage | ~12 vendors (Cloudflare, DataDome, Akamai, Imperva, PerimeterX, Kasada, AWS WAF, F5, Sucuri, CloudFront, reCAPTCHA, hCaptcha) | The handful that render a known widget |

They share one vendor vocabulary: `cloudflare` here lines up with the DOM
detector's `CloudflareChallenge`/`Turnstile`, `datadome` with `DatadomeBlock`,
and so on. The signature pass is the cheap, wide triage that runs on every
fetch; the DOM detector is the confirm/solve layer.

## Presence vs. challenge — the load-bearing distinction

Each vendor carries two kinds of signal:

- **presence** (`signals[]`) — the vendor is fronting the site (`server:
  cloudflare`, `x-amz-cf-id`). This is **telemetry only**. A CDN serving you
  real content is not a problem; do **not** rotate on presence.
- **challenge** (`challenge[]`) — an active wall fired (`cf-mitigated:
  challenge`, a `geo.captcha-delivery.com` script, an Akamai `Reference #`
  error page). **This** is the "rotate" signal.

The quiet win over blind retry: when Akamai or Incapsula is merely *present* and
serving fine, the detector says so and you spend **no** rotation.

## The verdict

Surfaced as a non-fatal annotation — presence never turns a working
`403`-with-HTML into an error, and `fetch_many` per-item isolation holds.

**MCP** (`fetch`, `fetch_many`, `session_navigate`) — an `antibot` field, only
present when a vendor is detected:

```json
{
  "url": "https://fortress.theplumber.dev/",
  "status_code": 200,
  "antibot": {
    "vendors": ["cloudflare"],
    "challenged": true,
    "challenge_vendor": "cloudflare",
    "corpus_version": "cl-2026.06.01",
    "evidence": "body"
  }
}
```

Note `status_code: 200` and `evidence: "body"` — this is a live capture. The
Cloudflare managed challenge here ships the interstitial in a **200** body with
no `cf-mitigated` header, so the cheap header tier misses it and the body-prefix
fallback is what catches it (`just a moment` / the Turnstile script). This is
exactly why the body tier exists: without it, this wall would be a false
negative reported as a clean 200. See `examples/antibot_detection.py` for the
runnable version.

**Python** — `PageResponse.antibot` (an `AntibotVerdict`) plus the raw
`PageResponse.headers` dict it was derived from (for provenance: `cf-ray`,
`x-cache`, …).

Fields:

- `vendors` — every vendor tag detected, sorted.
- `challenged` — an active wall fired.
- `challenge_vendor` — which vendor's challenge fired.
- `corpus_version` — the signature corpus this verdict was produced against.
  **Record it alongside captures**; a verdict is a captured fact, not something
  to recompute at replay time against a newer corpus.
- `evidence` — which tier matched: `none` / `headers` / `body`. The detector
  runs status + headers first and only scans a bounded body prefix
  (`BODY_PREFIX_LIMIT`, 64 KiB) as a fallback for 200-cloaking walls, so the
  common case never touches the body.

## The `AntibotChallenge` exception

A typed `AntibotChallenge { vendor }` exists across Rust, Python, and the MCP
error payload (tagged `{"exception": "AntibotChallenge", "vendor": …}`). It is
**not** raised on the `fetch` / `fetch_many` path — that path uses the non-fatal
annotation above. It is reserved for explicit detect/routing callers that opt
into failing hard on a wall.

## Routing (next)

The intended policy table, deterministic per vendor:

| Detected | Action |
|---|---|
| `cloudflare` + challenged | headful + warm profile (Turnstile needs a real widget instance) |
| `datadome` + challenged | rotate residential proxy (IP-reputation driven) |
| `perimeterx` / `kasada` | headful + slow + warm |
| any vendor, **presence only** | no action |

Detection lives VoidCrawl-side (fast, deterministic). The routing *policy* is a
follow-up; it must square `datadome → rotate` with the existing
[profile policy](profiles.md), and should jitter / vary its escalation so the
block→retry reaction is not itself a fingerprint.

Once a Cloudflare Turnstile widget actually renders, its "Verify you are human"
checkbox lives in a **closed shadow root inside a cross-origin
`challenges.cloudflare.com` iframe** — unreachable by page JS. Drive it with the
accessibility-tree locator (`ax_box_in_frame` / `click_ax_in_frame`) and a
trusted compositor click; a centred click mints the `cf-turnstile-response`
token without any shadow tampering. See
[cross-origin-frames.md](cross-origin-frames.md).

## Fixtures & health

- **`fortress.theplumber.dev`** — live anti-bot benchmark. Currently a
  Cloudflare Turnstile managed challenge, so it should trip the `cloudflare`
  *challenge* signals. Use as a liveness canary for detection + (optionally)
  end-to-end bypass rate — **not** as the accuracy metric.
- **`bot.sannysoft.com`** — outbound fingerprint regression canary for
  [stealth](stealth.md); a regression here is the early warning that we'll start
  getting challenged at real targets.
- **`tests/antibot_accuracy.rs`** — the offline, hermetic accuracy benchmark
  (precision/recall over a held-out labeled corpus, disjoint from the canaries).

See [`crates/core/CORPUS.md`](../crates/core/CORPUS.md) for corpus provenance and
how to add a vendor.
