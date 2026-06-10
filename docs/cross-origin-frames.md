# Cross-Origin & Closed-Shadow Frames

A controlling debugger is not bound by the same-origin policy or by shadow-DOM
mode the way page JavaScript is. `voidcrawl` exposes that reach as two sibling
families of frame-scoped primitives — one that runs **JS inside** a frame, one
that **locates by accessibility** inside a frame and pierces closed shadow roots.
Both select the target frame by a substring of its URL (`resolve_frame`).

| | JS-world — read/drive a frame's DOM | AX-world — locate inside a frame |
|---|---|---|
| Methods | `eval_js_in_frame` / `evaluate_js_in_frame`, `frame_urls` | `ax_outline_in_frame`, `ax_box_in_frame`, `click_ax_in_frame` |
| Runs in | the frame's **page-world** execution context | the **browser-computed** accessibility tree |
| Crosses | the same-origin `contentDocument`-is-`null` wall | the same wall **plus closed shadow roots** |
| Reaches | Stripe/Braintree fields, OAuth iframes, consent walls, reCAPTCHA's `bframe` | widgets page JS can't see — Cloudflare Turnstile's checkbox in a closed shadow root |
| Since | 0.3.5 | 0.3.6 |

Both live on `Page` and `PooledTab` (PyO3). `eval_js_in_frame` is additionally an
MCP tool; the AX-in-frame locators are Python/Rust API only.

## Shared in-process requirement

For either family the target frame must be in the page's **renderer process** —
the debugger can only attach to a frame Chrome keeps in-process. Ordinary
cross-origin frames stay in-process under voidcrawl's default flags, but Chrome
field-trial-isolates a few origins out-of-process regardless — notably
**google.com** (reCAPTCHA) and **challenges.cloudflare.com** (Turnstile).
Reaching those is an explicit opt-in:

```python
from voidcrawl import BrowserConfig, BrowserSession

browser = BrowserSession(
    BrowserConfig(extra_args=["disable-site-isolation-trials"])
)
```

It is not a global default because it weakens the browser's isolation posture.
Without it, a matched frame surfaces as `FrameNotFound`. `frame_urls()` lists
every frame the browser tracks — handy for discovering the right substring and
for confirming the isolated origins came back in-process.

## JS-world: `eval_js_in_frame`

Same-origin policy makes a cross-origin iframe's `contentDocument` return `null`
to the parent's JavaScript. That is a *page-script* restriction; it does not
apply to the controlling debugger. `eval_js_in_frame(frame_url_pattern, expr)`
evaluates `expr` inside the target frame's **own** execution context — where
`document` is that frame's document and the origin check is satisfied. You can
both read and drive the frame's DOM.

```python
secret = await page.eval_js_in_frame(
    "recaptcha/api2/anchor",
    "document.querySelector('#recaptcha-anchor').getAttribute('aria-checked')",
)
```

`VoidCrawlError::FrameNotFound` / `AmbiguousFrame` surface as MCP
`invalid_params`. See [`examples/cross_origin_iframe_eval.py`](../examples/cross_origin_iframe_eval.py).

## AX-world: locate inside a frame, pierce closed shadow roots

Page JS cannot see into a **closed** shadow root — `element.shadowRoot` is
`null`, and `eval_js_in_frame` runs in page-world JS, so it can't pierce them
either. The accessibility tree can: `Accessibility.getFullAXTree` is
browser-computed and **ignores shadow-DOM mode**, so rooting it at a frame
descends into that frame's closed shadow roots. Three methods build on that:

- **`ax_outline_in_frame(frame_url_pattern, depth=None)`** — a compact
  `role "name"` outline of the frame (the same shape as the `session_ax_tree`
  outline), for discovering the role + accessible name to target. `depth` caps
  descendant traversal; `None` returns the whole tree.
- **`ax_box_in_frame(frame_url_pattern, role, name, nth=0)`** — locate the match
  and return its on-page rectangle `[x, y, width, height]` in CSS pixels,
  **without** clicking — so you can drive a humanized click yourself (curved
  approach via `dispatch_mouse_event`, press at a jittered point in the box).
- **`click_ax_in_frame(frame_url_pattern, role, name, nth=0)`** — locate and
  click the match at its box-model centre with a **trusted compositor** mouse
  event (`Input.dispatchMouseEvent`), *not* a DOM `.click()`. Challenge widgets
  reject untrusted clicks; this one reports `isTrusted=true`.

An empty `name` matches any node of that `role`; `nth` disambiguates duplicates.
Each errors if no node matches. These are the cross-frame, shadow-piercing
analogues of `query_ax_tree` / `ax_box` / `click_by_role`.

### Why a compositor click, and no shadow tampering

Two rules make the difference against a real challenge widget:

1. **Trusted input.** A DOM `.click()` dispatched via `CallFunctionOn` is
   untrusted (`isTrusted=false`) and challenge widgets ignore it. A compositor
   `Input.dispatchMouseEvent` at the element's pixel rect is trusted.
2. **No page-JS shadow tampering.** voidcrawl once shipped a patch that
   force-opened all shadow DOMs (patching `Element.prototype.attachShadow`). It
   was **removed** because Turnstile tamper-checks its closed root and fails the
   challenge with `ERROR 600010` (see the note in [stealth.md](stealth.md)). The
   AX route reads the browser's own tree and clicks by pixel — it never touches
   page JS or shadow mode, so it doesn't trip that check.

## Worked example: Cloudflare Turnstile's checkbox

Turnstile's "Verify you are human" checkbox is a real `<input type=checkbox>`
inside a **closed shadow root** inside a **cross-origin**
`challenges.cloudflare.com` iframe — previously unreachable by any page-JS path.
With the AX locator:

```python
FRAME = "challenges.cloudflare.com"

# 1. discover the role/name (pierces the closed shadow root):
print(await page.ax_outline_in_frame(FRAME))      # → checkbox "Verify you are human"

# 2. one-call trusted click:
await page.click_ax_in_frame(FRAME, "checkbox", "Verify you are human")

# …or drive it yourself for a jittered, humanized click:
x, y, w, h = await page.ax_box_in_frame(FRAME, "checkbox", "Verify you are human")
await page.dispatch_mouse_event("mousePressed", x + w / 2, y + h / 2)
await page.dispatch_mouse_event("mouseReleased", x + w / 2, y + h / 2)

# 3. the click mints the token into the hidden response input:
token = await page.evaluate_js(
    "document.querySelector('input[name=cf-turnstile-response]').value"
)
```

Verified live on `https://2captcha.com/demo/cloudflare-turnstile`:
`ax_outline_in_frame` surfaced `checkbox "Verify you are human"`,
`ax_box_in_frame` returned the 24×24 checkbox rect, and a compositor click
minted the `cf-turnstile-response` token. Runnable version:
[`examples/turnstile_checkbox_ax.py`](../examples/turnstile_checkbox_ax.py).

> Managed Turnstile that scores you *before* any interaction is a separate
> problem — that's the headful/GPU fingerprint job in [stealth.md](stealth.md).
> This locator is for the interactive checkbox once the widget renders.
