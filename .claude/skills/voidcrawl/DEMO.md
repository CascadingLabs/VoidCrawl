# voidcrawl skill — demo use cases

Concrete, verified flows for driving a browser through the `voidcrawl` MCP
server in Claude Code or Codex. Each shows the prompt to give the agent and the
tool sequence it should run. (Verified end-to-end against the installed binary.)

## Prereq
`voidcrawl-mcp` on PATH and wired into your host — see `SETUP.md`. Confirm the
server is connected (your host should list a `voidcrawl` MCP server / its tools).

---

## Demo 1 — Scrape headlines and open an article (stateful, AX-first)

**Paste this to the agent:**
> Using the voidcrawl MCP, open a session, go to https://qscrape.dev/l2/news,
> list the top article headlines, then open the first one and tell me its URL.

**Tool sequence it should run** (this is the canonical perceive → act pattern):
1. `session_open {}` → `session_id`
2. `session_navigate { session_id, url: "https://qscrape.dev/l2/news" }` → `{status_code: 200, …}`
3. `session_ax_tree { session_id }` → compact outline. On this page: ~572 nodes,
   ~337 named, ~3.6 KB — every headline shows up as `button "<headline>"`. The
   agent reads headlines straight off the outline (no HTML needed).
4. `click_by_role { session_id, role: "button", name: "<exact headline>" }`
5. `eval_js { session_id, expression: "location.href" }` → confirms the URL
   changed (e.g. `…/news/?id=MHH-016`).
6. `session_close { session_id }`

**Why it's the good path:** the AX outline is ~10–50× smaller than the page
HTML and already semantic, and `click_by_role` targets the headline by its
accessible name, so it survives markup changes a CSS selector wouldn't.

---

## Demo 2 — Parallel scrape (stateless fan-out)

**Paste this to the agent:**
> Using voidcrawl, fetch these three pages in parallel and give me each one's
> status code and title: https://example.com, https://qscrape.dev/l2/news,
> https://httpbin.org/html

**Tool:** one `fetch_many` call:
```json
{ "requests": [ {"url":"https://example.com"},
                {"url":"https://qscrape.dev/l2/news"},
                {"url":"https://httpbin.org/html"} ] }
```
**Result shape (important):**
```json
{ "results": [ { "ok": true, "result": { "url": …, "status_code": 200, "title": …, "html": … }, "error": null }, … ] }
```
`status_code`/`title` live under each item's **`result`** (not at the item top
level); per-item failures set `ok:false`+`error` and don't abort the batch.
Pass an `extract` JS expression in each request to bring back just the fields
you want instead of full HTML.

---

## Gotchas (discovered by driving it for real)

- **`click_by_role` name matching is EXACT** — case- and whitespace-sensitive.
  Read the exact `name` from `session_ax_tree` first; uppercasing or trimming
  differently → `no AX node … at index 0`. Use `nth` to disambiguate duplicates.
- **`wait_for_network_idle` can burn its full timeout after an in-page (SPA)
  click** — the URL updates with no new network activity, so idle never fires.
  Pass a short `timeout_secs` (2–3) or prefer `wait_for: "selector:<css>"`, or
  just read state directly with `eval_js`.
- **Don't dump raw HTML to reason over a page** — `session_ax_tree` (compact) is
  the default "see the page" tool. Only fall to `screenshot`/HTML when the AX
  tree is thin (low `named_count` vs `node_count`).
- **Always `session_close`** — an open session keeps a Chrome alive until the
  server exits.
- **Captchas:** on a `CaptchaDetected` error, surface it and rotate
  proxy/profile — don't retry the same URL, don't try to solve.

## Quick connectivity check (no agent)
From this repo: `python3 /tmp/vc_ready.py` style — or just confirm your host
lists the `voidcrawl` tools. The server also ships a one-paragraph usage primer
as its MCP `instructions`, so any client sees the workflow on connect.
