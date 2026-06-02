# Anti-bot signature corpus

The signature corpus embedded in [`src/antibot.rs`](src/antibot.rs) (the
`CORPUS_JSON` const) fingerprints which anti-bot / WAF / CDN vendor is gating an
HTTP response, and whether that vendor is *actively challenging* us (a wall) vs.
merely *present* (a CDN fronting a site that served us fine).

## Provenance & license

The vendor list and the `signals[]` / `challenge[]` split are modeled on
**`albinstman/antibot-print`** (MIT) — <https://github.com/albinstman/antibot-print>.

- **Upstream license:** MIT (compatible with this Apache-2.0 / MIT crate).
- **Reference commit:** *record the SHA you authored against here when syncing.*
- **What we took:** the *idea* and the vendor taxonomy. The actual regex
  patterns are **first-party** — hand-authored against the vendors we actually
  meet, not a wholesale vendoring of the upstream JSON. This keeps the ruleset
  small, readable, owned, and unit-testable, and avoids tracking upstream churn
  or importing a pattern that bloats compile.

## Governance

- The corpus is **not** auto-synced. Treat any change as a reviewable diff.
- **Bump `CORPUS_VERSION`** in `src/antibot.rs` on every edit. Verdicts are
  recorded with this version so a replay-grade archive can reproduce a
  classification deterministically — a verdict is captured data, never
  recomputed at replay time against a newer corpus.
- Drift is caught loudly by the **offline accuracy benchmark**
  (`tests/antibot_accuracy.rs`): a held-out labeled corpus scored for
  precision/recall. If a vendor changes its markers and a signature rots, that
  test fails rather than silently lowering field accuracy.
- The benchmark corpus is deliberately **disjoint** from the live canaries
  (`fortress.theplumber.dev`, `bot.sannysoft.com`) so we measure the detector,
  not one target.

## Adding / editing a vendor

1. Edit `CORPUS_JSON` in `src/antibot.rs`. Patterns are RE2-style (no
   backreferences / lookaround) so they run under the linear-time `regex`
   engine and are safe on attacker-controlled input. Matched case-insensitively
   against the normalized `S:`/`H:`/`B:` form.
   - `H:` patterns are anchored at the header line start (`h:server: …`).
   - `B:` patterns match their marker *anywhere* in the (bounded) body.
   - Put markers that only appear during an **active block** in `challenge[]`;
     put mere-presence markers (CDN headers) in `signals[]`.
2. Add a labeled fixture to `tests/antibot_accuracy.rs`.
3. Bump `CORPUS_VERSION`.
4. `cargo test -p void_crawl_core antibot` + `--test antibot_accuracy`.

## Vendor coverage

WAF/CDN: Cloudflare, Akamai, Imperva/Incapsula, AWS WAF, F5 BigIP, Sucuri,
CloudFront. Bot detection: DataDome, PerimeterX/HUMAN, Kasada. Challenge
widgets: reCAPTCHA, hCaptcha (Turnstile is folded into the `cloudflare` vendor).
