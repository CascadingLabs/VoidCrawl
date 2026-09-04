# VoidCrawl–Yosoi Browser Acquisition boundary

Status: CAS-313 architecture contract; no public API changes

This document defines how VoidCrawl can serve as the Chromium/CDP provider for
Yosoi Oxide Browser Acquisition without making either project own the other's
domain. It assesses VoidCrawl 0.5.0 against the pre-release Yosoi Web Capture
model and records the minimum boundaries that later readiness work must
preserve.

The assessment covers the Rust core. PyO3 and MCP are delivery facades over the
core and are not capture-domain authorities.

## Reference inputs

The VoidCrawl assessment is grounded in these implementation surfaces:

- `crates/core/src/session.rs`, `page.rs`, `response.rs`, and `pool.rs` for
  browser, navigation, network, and concurrency behavior;
- `crates/core/src/ax.rs`, `recording.rs`, `viewport.rs`, and `cookie_jar.rs`
  for accessibility, visual, layout, and cookie primitives;
- `crates/core/src/selector.rs`, `error.rs`, `interrupt.rs`, and
  `managed_profile.rs` for current coupling, failures, interruption, and
  profile-state boundaries;
- `crates/pyo3_bindings` and `crates/mcp_server` for facade constraints.

The comparison uses the current pre-release Yosoi Oxide sources and decisions:

- `yosoi-web-capture/src/acquisition.rs` and `environment/browser.rs`;
- `yosoi-web-capture/src/artifact/{family,capability,metadata,outcome}.rs`;
- `yosoi-web-capture/src/observation/{record,termination}.rs`;
- the Web Capture v1 wire contract and CAS-298 source-representation contract;
- CAS-308's planned acquisition-foundation certification and Browser
  Acquisition handoff.

These Yosoi contracts are still pre-release. The settled ownership boundary in
this document is intended to survive their evolution; field-for-field adapter
mappings are deliberately deferred.

## Decision

The dependency direction is:

```text
Yosoi orchestration, policy, extraction, replay, and SDK
                            |
Yosoi-owned VoidCrawl Browser Acquisition adapter
                /                           \
       yosoi-web-capture              void_crawl_core
                                                |
                                 chromiumoxide / Chromium
```

`void_crawl_core` does not depend on `yosoi-types` or `yosoi-web-capture`.
The adapter depends on both and translates between their types. This keeps
VoidCrawl independently publishable, keeps the Yosoi wire model authoritative
in Yosoi, and leaves room for another browser controller without introducing a
provider hierarchy now.

The adapter belongs in the Yosoi repository because it allocates Yosoi capture
identities, interprets Yosoi requests, constructs Yosoi artifacts, and finalizes
Yosoi capture attempts. A future crate may be named for the integration, but
CAS-313 does not commit a crate name or create it.

## Ownership contract

### VoidCrawl owns

- launching or attaching to Chromium and reporting whether the connection is
  still alive;
- browser, page, frame, context, profile, tab, and pool resource mechanics;
- issuing CDP commands and subscribing to CDP events;
- applying browser configuration and reporting effective facts that Chromium
  can observe;
- navigation, interaction, interruption, screenshots, recordings, downloads,
  and browser-state access;
- bounded provider-native collection, including honest partial, truncated,
  unavailable, and disconnected outcomes;
- cleanup of every task, listener, tab permit, capture lock, temporary file,
  and browser resource it creates;
- documenting CDP, Chromium, and chromiumoxide limitations.

VoidCrawl may expose owned byte payloads and secret-safe metadata. Its result
must not require a live `Browser`, `Page`, event stream, filesystem path, or
process-local registry in order to be interpreted after the operation ends.

### Yosoi owns

- `CaptureId`, `ActivityId`, artifact identities, provenance, schema identity,
  and canonical wire formats;
- `WebCaptureRequest`, semantic acquisition strategy, requested artifact
  families, and resolved attempt policy;
- provider capability profiles and final support declarations;
- the authoritative observation window, accounting, termination, activity
  receipt, capture completeness, and finalization invariants;
- artifact payload admission, digest verification, `CaptureBundle`, archive
  handoff, and cross-process transport;
- sensitivity and retention policy, retry/fallback orchestration, extraction,
  recipes, replay, archive, and SDK behavior;
- deciding whether browser state, credentials, scripts, downloads, or other
  effects are permitted for a job.

VoidCrawl reports facts needed by these decisions. It does not decide that a
family is `OmittedByPolicy`, that an attempt should fall back to Direct HTTP, or
that evidence satisfies a Yosoi contract.

### The adapter owns

- resolving a Yosoi `BrowserContextRef` or `PageContextRef` to a live VoidCrawl
  resource without serializing the live handle;
- translating a resolved Yosoi browser request into explicit VoidCrawl runtime
  options;
- selecting and arming only the provider-native observations needed for the
  requested artifact families;
- correlating VoidCrawl relative timing and loss facts with the Yosoi
  observation window;
- mapping VoidCrawl capabilities and typed failures to Yosoi capability,
  artifact-family, interruption, and receipt outcomes;
- submitting owned payload bytes to Yosoi's payload-integrity boundary;
- constructing and finalizing the Yosoi `WebCapture`.

The adapter must not reimplement CDP behavior. VoidCrawl must not construct a
Yosoi aggregate.

### First document-navigation flow

The first integration should remain a direct composition rather than a generic
provider interface:

```text
resolved Yosoi document-navigation request
  -> adapter resolves or allocates a VoidCrawl runtime context
  -> adapter reads effective VoidCrawl environment and capabilities
  -> adapter asks VoidCrawl to arm the requested bounded observations
  -> VoidCrawl performs navigation while observations are active
  -> VoidCrawl stops collectors and returns owned payloads plus terminal facts
  -> adapter admits payloads and maps provider outcomes
  -> Yosoi finalizes WebCapture and CaptureBundle
```

Retries or fallback allocate another Yosoi capture attempt. Reusing a live
browser context does not reuse a `CaptureId`, and a VoidCrawl target ID is not a
Yosoi page, capture, or artifact identity.

## Capability assessment vocabulary

- **Reusable**: the existing Rust primitive is suitable to call from the
  adapter without changing its core semantics. Additional capture metadata may
  still be required around it.
- **Partial**: useful behavior exists, but it cannot yet satisfy the full
  artifact-family contract honestly.
- **Missing**: no first-class Rust primitive exists for the family.
- **Disabled**: VoidCrawl has a primitive, but the selected CDP mode prevents
  it from working.
- **Unsupported**: the selected browser/controller configuration inherently
  cannot supply the family and should declare that rather than attempt it.

No current VoidCrawl result is by itself a finalized Yosoi artifact. “Reusable”
therefore describes the provider primitive, not Yosoi artifact completeness.

## Web Artifact capability matrix

| Yosoi family | Current VoidCrawl primitives | Current assessment | Missing for Browser Acquisition | Primary follow-up |
| --- | --- | --- | --- | --- |
| Source | `Page::arm_navigation_capture`; `MainDocumentSource`; focused `ResponseCapture` | Reusable final-document source primitive when Network is enabled | Yosoi payload admission/digest/schema; Chromium cannot incrementally bound its internal response buffering | Browser Acquisition adapter |
| Rendered DOM | `Page::rendered_dom_snapshot`; `Page::content`; `PageResponse::html` | Reusable bounded live-DOM primitive with document epoch/scope | Yosoi payload admission, digest, and schema; Chromium still materializes full DOM before VoidCrawl retention limits | Browser Acquisition adapter |
| Accessibility tree | bounded top-level/frame `AccessibilitySnapshot`; raw query helpers; `ax_tree_outline` projection | Reusable raw AX primitive with depth/node/byte bounds and unavailable frame state | Yosoi payload admission, digest, and schema; OOPIF availability remains renderer-dependent | Browser Acquisition adapter |
| Network | `NavigationCaptureReport` bounded resource graph; response expectations; endpoint and Resource Timing projections | Reusable bounded graph primitive when Network is enabled | Yosoi network payload schema and artifact admission; raw ExtraInfo remains unavailable in the current client | Browser Acquisition adapter |
| Cookies | `Page::get_cookies`, set/delete methods; `CookieLease` and `CookieProvenance` | Reusable access and lease primitives, partial artifact observation | Capture-context scope; snapshot timing; bounded payload schema; value retention/sensitivity outcome; artifact extent and digest handoff | CAS-315, then Browser Acquisition adapter |
| Storage | Arbitrary `evaluate_js` could inspect some storage | Missing as a first-class capture primitive | Scoped cookie-external storage vocabulary; categories; origin/frame scope; bounds; inaccessible/unsupported outcomes; safe value handling | CAS-316 only after a concrete requirement |
| Layout | `Page::layout_snapshot`; typed CSS layout viewport, visual viewport, and content-size facts; selector/AX box resolution | Reusable point-in-time layout primitive with document epoch/scope | Yosoi payload schema; style/text-box/paint-order support only when a concrete request requires it | Browser Acquisition adapter |
| Visual | `Page::visual_snapshot`; typed PNG dimensions, capture region, viewport/DPR, byte extent, time, and document epoch; recordings with timing, epoch, masks, frame offsets, and drop counts | Reusable visual capture primitives | Yosoi payload admission/digest/schema; screencast encoding remains policy-selected and successful recordings report delivery loss honestly | Browser Acquisition adapter |
| Runtime diagnostics | `ObservationScope` bounded protected console/exception text linked to event sequence; individual JS evaluation errors | Reusable page-level diagnostic primitive; arming escalates Runtime when needed | Yosoi payload admission/schema and frame association when CDP supplies it | Browser Acquisition adapter |

### Source is not rendered DOM

`Page::content` returns the current outer HTML after page execution. It is
rendered-DOM evidence. `PageResponse::html` calls that same method after a
network-idle event and is also rendered-DOM evidence, despite living beside
HTTP status and headers.

A source artifact requires independently observed response-body bytes and a
truthful byte-layer declaration. The provider must not label `Page::content`
as source, reconstruct unavailable source from the DOM, or treat a JavaScript
application shell as equivalent to its later live DOM.

### Presentation projections are not authoritative artifacts

`Page::document_snapshot`, `ax_tree_outline`, sanitized endpoint sets, MCP
`extract`, anti-bot verdicts, and similar compact values are useful
presentation or routing projections. They are not substitutes for the
underlying rendered DOM, raw AX evidence, or bounded network observation.
They may later become derived artifacts with explicit provenance if Yosoi has a
concrete consumer.

## Existing primitives expected to remain reusable

Later tickets should prefer composing these APIs over replacing them:

| Area | Existing API | Boundary note |
| --- | --- | --- |
| Browser lifecycle | `BrowserSessionBuilder`, `BrowserSession::{launch_headless, launch_headful, connect, close, is_alive}`; `Page::environment_snapshot` | Launch/attach mechanics and provider-native effective environment reporting remain VoidCrawl-owned. |
| Page lifecycle | `new_blank_page`, `new_page`, `attach_page`, `Page::{navigate, wait_for_navigation, close}` | Browser Acquisition should normally create or resolve the page, arm observations, and then navigate. |
| Pre-trigger subscription | `Page::arm_observation`; `Page::expect_responses` / `ResponseCapture` | Observation scope owns bounded marker collection and teardown; response expectations retain their focused body-capture behavior. |
| Rendered state | `Page::{content, url, title}` | Reuse as point observations, then add explicit scope, bounds, and metadata around them. |
| Accessibility | `get_full_ax_tree`, frame AX helpers, `query_ax_tree` | Raw evidence remains separate from compact `ax_tree_outline`. |
| Visual | `screenshot`, `start_recording`, `RecordingHandle::stop` | Keep capture and masking mechanics; return enough metadata for durable interpretation. |
| Cookies | `get_cookies`, `CookieLease`, `CookieProvenance` | Keep values non-serializable by default; Yosoi owns artifact retention and replay eligibility policy. |
| Profiles | `ProfileRegistry`, `ManagedProfileLease`, `ManagedProfileSnapshot` | Runtime locators and paths stay out of capture metadata. Snapshotting is an isolation mechanism, not a Yosoi environment type. |
| Interruption | `InterruptRegistry` and session interrupt methods | VoidCrawl parks the live page; Yosoi maps the factual event to its own interruption and orchestration vocabulary. |
| Concurrency | `BrowserPool`, semaphore accounting, tab recycling | Reuse for shared-state workloads only after CAS-315 makes state semantics explicit. Benchmark before changing defaults. |

Input actions, captcha/challenge helpers, download scanning, MCP session
registries, and Python convenience flows remain useful VoidCrawl product
features. They do not become Browser Acquisition contract types merely because
they share a page.

## APIs that must not define the integration boundary

### `PageResponse`

`PageResponse` combines final live DOM, selected document response metadata,
a redirect boolean, a lossy endpoint projection, and an anti-bot classifier.
It is a convenient fetch result, not a provider-neutral capture attempt. The
adapter may temporarily call it for existing behavior, but new Browser
Acquisition semantics must not be added to this aggregate.

A navigation timeout currently returns `Err` and does not return the already
observed document/network facts. That is correct for the convenience API but
insufficient for Yosoi partial-capture finalization.

### `ResponseCapture`

`ResponseCapture` is useful for named response expectations. It completes only
when every named glob has matched, and a timeout returns an error rather than
all partial matches. Bodies are obtained with `Network.getResponseBody` after
`loadingFinished`; the byte limit is applied to the returned buffer, not to the
browser's transport or decode path. It must not be presented as an
incrementally bounded resource graph.

The acquisition snapshots added for Browser Acquisition use **retention
bounds**, not transport-memory bounds. Chromium/chromiumoxide materializes
`Network.getResponseBody`, serialized live DOM, and the full requested AX tree
before VoidCrawl applies source, DOM, node, or byte retention limits. The
navigation scope's deadline and cancellation now preempt an in-flight body
request, but callers must not interpret these limits as a guarantee on peak
renderer, CDP-client, or process memory. CAS-317/CAS-310 measurement must size
and budget that unavoidable provider-side materialization explicitly.

### Browser targets and legacy selector compatibility

`crates/core/src/selector.rs` now owns the provider-native `BrowserTarget`,
`BrowserTargetKind`, and `TargetResolution` vocabulary used by screenshots,
recordings, masks, and geometry resolution. Higher-level recipe or selector
models translate into these primitives at their adapter boundary.

The pre-CAS-321 Rust names remain deprecated type aliases, and MCP/PyO3 retain
their existing flat selector wire/keyword shapes for compatibility. Those
facades translate into `BrowserTarget`; they do not make an external selector
model authoritative in VoidCrawl core.

### Structured error boundary

Every `VoidCrawlError` now exposes a stable namespaced code, broad category,
and secret-safe summary. Standard `Display` and `Debug` formatting omit raw
provider diagnostics for typed variants; Rust callers can still recover local
diagnostics by matching the variant. The legacy `Other` escape hatch retains
its compatibility display behavior and must not be used for new public
operations. Python exceptions and MCP errors use safe messages and structured
dispatch fields by default. Machine-local profile
search paths and raw browser/page diagnostics do not cross those facades.

### Environment input structs

`StealthConfig`, `Viewport`, `PoolConfig`, Chrome arguments, profile paths, and
environment variables are operational inputs. They are not proof of effective
browser state. In particular, `Page::current_viewport` reports only an override
tracked by that `Page` handle, and an attached browser may have unobserved
ambient state. `Page::environment_snapshot` queries representation-affecting
page values and explicitly reports attached-browser mode as unavailable rather
than inferring it from process-local configuration.

`ManagedProfile` serializes a local `PathBuf`; it must never be embedded in a
Yosoi capture environment or provider-native durable observation.

### MCP and PyO3 wire results

MCP structs and Python classes optimize for agent and Python ergonomics. They
may redact, flatten, stringify, or omit provider facts. New capture behavior is
implemented and tested in `void_crawl_core` first; bindings expose it only when
a public consumer requires them.

## Provider-native observation requirements

The following are requirements, not committed Rust type names or a speculative
provider interface. Later tickets should introduce the smallest concrete types
needed by the first VoidCrawl-backed Browser Acquisition path.

### Common requirements

Every observation returned across the adapter boundary should:

1. be owned data with no live CDP objects, stream handles, locks, registry
   entries, or required filesystem paths;
2. distinguish complete, partial, truncated, discarded, unavailable, disabled,
   and unsupported states where those meanings apply;
3. report exact retained byte/event counts and known loss, while preserving an
   explicit unknown-loss state when Chromium cannot report it;
4. carry monotonic offsets or sequence information from one provider scope and
   enough wall-clock correlation for the adapter to create Yosoi provenance;
5. identify document, frame, loader, request, or context relationships with
   provider-local opaque identities rather than Yosoi artifact IDs;
6. separate secret-bearing payloads from ordinary serializable metadata by
   type, not only by documentation;
7. bound collections and payloads before returning them and report which bound
   stopped admission;
8. use typed enums for closed semantics and reserve raw CDP JSON for facts that
   have intentionally not been normalized;
9. expose the controller/renderer version and collection mode needed to explain
   support and limitations;
10. remain provider-native: no `CaptureId`, `WebArtifact`, Yosoi `ReasonCode`,
    Yosoi canonical JSON, or archive location.

### Observation scope report

The provider scope will need to report:

- start correlation and terminal monotonic offset;
- why it stopped from the provider's perspective: caller finish, deadline,
  cancellation, explicit interrupt, page close, renderer failure, browser
  disconnect, or internal failure;
- admitted, retained, and dropped event/byte accounting, including unknown
  dropped counts;
- in-flight work when observable;
- per-collector completion and failure without erasing successful sibling
  collectors;
- cleanup completion or cleanup failure.

Yosoi maps this report into its policy-specific observation and receipt types.
VoidCrawl must not claim that browser `networkIdle` satisfies a future Yosoi
settlement policy unless the adapter explicitly makes and validates that
mapping.

The initial `ObservationScope` now provides this lifecycle boundary with
secret-safe Network/Runtime markers, event bounds, an elapsed deadline,
in-flight request accounting, explicit terminal outcomes, and drop-safe
collector teardown. Console and exception text is retained only behind its own
byte budget, excluded from serialization/default debug, and linked to the
corresponding event sequence. Detailed network payload records remain in their
focused collectors.

### Family observations

The first implementation is expected to need small concrete results for:

- main-document source and redirect/resource observations;
- rendered DOM bytes plus document scope;
- raw AX bytes plus frame/depth/limit facts;
- bounded network resource records and body availability;
- scoped cookie state when requested;
- visual bytes plus dimensions and coordinate context;
- bounded console and JavaScript exception records.

Storage and broad style/text-box/paint-order layout snapshots should not
receive generalized DTOs until a Browser Acquisition request demonstrates the
required categories and scope. `LayoutSnapshot` deliberately reports only the
coordinate spaces needed to interpret visual evidence.

## Browser-state binding and cleanup semantics

Every page now reports a provider-native `BrowserStateBinding`; a pooled tab is
never labeled isolated.

| Path | Binding | Release/disposal semantics |
| --- | --- | --- |
| ordinary launched-session page | `shared_browser_profile` | Shares the session's ephemeral profile until the browser closes. Closing a page does not clear profile state. |
| pooled tab | `shared_browser_profile` (or the session's attached binding) | `release_checked` navigates to `about:blank` and resets abandoned download behavior. Cookies, HTTP cache, local/session storage, IndexedDB, service workers, permissions, headers, viewport, and init scripts remain shared; reset failure disposes the tab rather than lending it again. |
| `new_isolated_context` | `isolated_browser_context` | `Target.disposeBrowserContext` closes every page and atomically removes context-bound cookies, cache, origin storage, service workers, permissions, and network state. Explicit disposal returns `ContextCleanupReport`; drop schedules best-effort disposal. |
| caller-selected persistent profile | `managed_profile` | State deliberately survives pages and sessions according to the managed-profile lease. |
| remote-debug attachment | `attached_browser` | Ambient state is externally owned and must never be described as reset or isolated. A newly created isolated context is still isolated and disposable. |

One-shot screenshot/recording viewport and scroll changes restore themselves.
Observation and navigation collectors own drop-safe teardown. Download behavior
is reset after completed downloads and again on checked pool release when an
armed capture was abandoned. Download files remain caller-owned quarantine
artifacts; browser-context disposal does not claim to delete caller filesystem
paths. Broad storage capture DTOs remain deferred because isolation does not
itself define Yosoi storage-retention policy.

## CDP and chromiumoxide constraints

These constraints are part of the capability story, not incidental bugs to
hide in the adapter.

1. **CDP mode changes support.** `CdpMode::Minimal` skips eager Runtime,
   Network, Performance, Log, and child-target auto-attach. Navigation,
   screenshots, accessibility, input, and main-world evaluation remain
   available. Some helpers, including `goto_and_wait_for_idle`, explicitly
   enable Network and therefore escalate the tab; `ResponseCapture` does not.
   Capability must describe the effective instrumentation after such
   escalation rather than treating “minimal” as a permanent guarantee.
2. **ExtraInfo events are currently absent.** The vendored chromiumoxide 0.9.1
   integration does not deliver `requestWillBeSentExtraInfo` or
   `responseReceivedExtraInfo` in the tested path. Browser-managed `Cookie` and
   raw `Set-Cookie` headers are therefore not captured from the wire.
3. **Response bodies are post-completion decoded representations.**
   `Network.getResponseBody` runs after `loadingFinished`; a deterministic gzip
   fixture verifies that Chromium has already removed content coding. Redirect
   bodies are unavailable, cache and service-worker paths can be unavailable,
   and VoidCrawl's retained-byte limit cannot incrementally bound Chromium's
   internal buffering. `MainDocumentSource` reports this layer and limitation
   explicitly.
4. **Header fidelity is limited.** CDP response headers are exposed through a
   map-shaped value on current paths, so duplicate lines, original casing, and
   wire order cannot be claimed unless another reviewed event supplies them.
5. **Navigation settlement is not universal.** `networkIdle` can be delayed or
   absent on long-polling and continuously active pages. It is one provider
   signal, not a universal definition of capture completion.
6. **Frames depend on instrumentation.** Cross-origin and out-of-process frame
   access requires Runtime contexts and target auto-attach; some field-trial
   isolation also requires an opt-in Chrome flag. Unavailable frame evidence
   must not be represented as an observed empty frame.
7. **Visual capture has compositor constraints.** Screenshots serialize the
   activate-and-capture step on a browser-wide lock. Screencasts are
   viewport-only, paint-driven rather than clock-driven, and shared-window tabs
   may require foregrounding for the entire recording.
8. **Attached browsers are only partially controlled.** VoidCrawl deliberately
   avoids reapplying launch-time stealth to attached Chrome. Ambient profile,
   launch, renderer, viewport, locale, and state facts may be unknown and must
   be queried or reported unavailable.
9. **Pooled tabs retain state.** Checked release navigates to `about:blank`
   and resets abandoned download behavior, but deliberately retains profile,
   origin, viewport, header, and instrumentation state. It is document cleanup,
   not context isolation; a failed reset disposes the tab instead of reusing it.
10. **The CDP client is patched.** The workspace uses a vendored chromiumoxide
    fork. Relevant event routing, CDP-mode, and lifecycle behavior require a
    documented patch/upgrade conformance surface rather than an assumption that
    upstream behavior is identical.

## Decisions settled here

1. VoidCrawl remains the concrete Chromium/CDP controller and observation
   producer.
2. Yosoi remains the authority for capture semantics and durable evidence.
3. The Yosoi-owned adapter is the only layer that depends on both crates.
4. VoidCrawl core will not import Yosoi Oxide types for this integration.
5. Provider-native observation results contain owned facts and bytes, never
   live browser state.
6. Source response and rendered DOM are separate evidence families.
7. Capability is reported for the effective mode; unavailable runtime evidence
   is not fabricated or silently converted to an empty artifact.
8. Rust core owns behavior before PyO3 or MCP exposure.
9. Existing concrete primitives are reused; no generic provider trait or plugin
   framework is introduced by the readiness project.
10. Baselines are measured before changing pool, tab, context, or CDP defaults.

## Decisions deferred to CAS-308 or later evidence

CAS-313 establishes ownership and information requirements, but does not freeze
unfinished Yosoi contracts.

- CAS-308 must certify the provider-neutral lifecycle, accounting, payload, and
  conformance seams demonstrated by Direct HTTP before Browser Acquisition
  adopts them.
- The final source body-layer and decode handoff waits for the Direct HTTP
  source contract and implementation evidence. VoidCrawl must report only the
  byte layer it can prove.
- Exact provider-native Rust type names, module layout, schema versions, and
  serialization choices belong to their implementation tickets.
- Exact Yosoi browser artifact payload schemas and multiplicities belong to the
  future Browser Acquisition project.
- Context isolation by incognito browser context, profile snapshot, tab reset,
  or browser process is decided by CAS-315 after behavioral and performance
  evidence.
- Settlement relevance, controller-stop criteria, and adaptive HTTP/browser
  fallback remain Yosoi policy/orchestration decisions.
- Storage categories and broad layout evidence remain deferred until a concrete
  Browser Acquisition consumer requires them.
- Archive admission, WARC/WACZ, remote payload storage, and replay credential
  handling remain outside this project.

## Implementation sequence and mutation ownership

Issue dependencies are authoritative. The numbered order is the preferred
implementation and review sequence; work may overlap only where file ownership
is disjoint.

| Order | Issue | Primary ownership | Shared-file hazards |
| --- | --- | --- | --- |
| 1 | CAS-313 boundary and capability matrix | This document | None after acceptance; later changes should be intentional contract updates. |
| 2 | CAS-317 baseline benchmarks | New benchmark harnesses and result docs | Do not optimize production code while collecting the baseline. |
| 3 | CAS-321 selector/error boundary | `selector.rs`, `error.rs`, adapter-facing error data; binding compatibility | `page.rs`, `lib.rs`, PyO3 exception mapping, MCP selector schemas |
| 4 | CAS-319 environment/capabilities | Prefer a focused new core module; `session.rs`, `viewport.rs`, `stealth.rs` only as required | `session.rs`, `page.rs`, `lib.rs`, public bindings |
| 5 | CAS-315 isolation/reset | `pool.rs`, `session.rs`, profile/context mechanics and focused tests | Pool semantics, profile locks, manifests, PyO3 acquire/release boundary |
| 6 | CAS-314 observation scope | Prefer a focused new core module and lifecycle tests | `page.rs`, `session.rs`, `error.rs`, `lib.rs`, Tokio task ownership |
| 7 | CAS-312 source/resource graph | `response.rs`, focused network modules/tests, vendored event routing when proven necessary | `page.rs`, observation scope, chromiumoxide fork, Cargo lock/manifests |
| 8 | CAS-311 DOM/AX | `ax.rs`, focused DOM/AX result modules and tests | `page.rs`, observation scope, frame handling |
| 9 | CAS-316 visual/layout/runtime | `recording.rs` and focused result/collector modules | `page.rs`, capture lock, observation scope, screenshot/recording bindings |
| 10 | CAS-318 conformance corpus | Deterministic fixture server/routes and golden scenarios | Golden regeneration and shared integration fixtures need one owner. |
| 11 | CAS-310 stress/soak | Scale harnesses, opt-in live workloads, operating-envelope docs | Do not mutate production behavior while measuring a candidate baseline. |
| 12 | CAS-320 certification | Fresh-context review and handoff docs | Fixes are assigned only after findings; CAS-308 is also a blocker. |

Only one writer should mutate `crates/core/src/page.rs`, `session.rs`, `lib.rs`,
`error.rs`, workspace manifests/lockfiles, or vendored chromiumoxide files at a
time. Prefer focused modules over adding another responsibility to the existing
large `page.rs`. A designated integrator should apply re-exports and binding
surface changes after each production module is stable.

At most two production-code lanes should run concurrently. Read-only research,
independent tests, fixtures, benchmarks, and review may fan out when they do not
regenerate shared goldens or modify the same browser profile.

## Verification requirements for later tickets

The boundary implies four verification layers:

1. **Pure Rust contract tests:** option validation, typed states, bounds,
   accounting, secret-safe serialization, and cleanup state machines without a
   browser.
2. **Deterministic Chromium integration:** local fixtures covering early events,
   redirects, mutation, long polling, frames, service workers/cache, partial
   bodies, console failures, cancellation, interruption, and disconnect.
3. **Cross-surface bindings:** only when a changed primitive is exposed through
   PyO3 or MCP; stubs and wire/error mappings must agree with Rust.
4. **Performance and soak evidence:** CDP microbenchmarks, repeated
   acquire/release, increasing concurrency, failure injection, resource-growth
   checks, and bounded opt-in live-site workloads.

Mutable public sites are canaries and research inputs, never required CI.
The provider-neutral v1 corpus lives at
`crates/core/tests/conformance/browser-acquisition-v1.json`, with checked-in
synthetic payloads, provenance, categorical goldens, stable scenario IDs, and
coverage-symbol validation in `browser_acquisition_conformance.rs`. It records
renderer crash as a deadline when Chromium leaves event streams open rather
than manufacturing a disconnect, and treats cross-origin frame payload versus
explicit unavailability as equally truthful provider outcomes.

Performance reports must state browser/build/machine metadata and completion
condition so response receipt, DOM load, network idle, selector readiness, and
controller stop are not compared as if they measured the same work.

## CAS-313 acceptance mapping

- **Explicit ownership:** recorded in the ownership contract and dependency
  diagram.
- **Every artifact family assessed:** recorded in the nine-family capability
  matrix with mode constraints and concrete gaps.
- **No imagined provider abstraction:** requirements describe the first
  VoidCrawl adapter; no provider trait, plugin registry, or common DTO crate is
  proposed.
- **Reviewable against CAS-308:** unfinished provider-neutral lifecycle, source,
  payload, and conformance decisions are listed as deferred.
- **Minimum sequencing and ownership:** recorded with issue order, primary
  modules, collision hazards, and verification layers.
