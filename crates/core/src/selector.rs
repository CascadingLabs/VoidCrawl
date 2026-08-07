//! Selector-backed screenshot bbox: resolve a Yosoi
//! [`SelectorEntry`](https://github.com/CascadingLabs/Yosoi — `yosoi/models/selectors.py`)
//! to one CSS-pixel rectangle, so `Page::screenshot`'s existing `bbox` crop
//! path can consume any Yosoi selector kind, not just an explicit
//! `(x, y, width, height)`.
//!
//! This module holds the pure data types and JS-snippet builders;
//! `Page::resolve_selector` (in `page.rs`) does the actual CDP round-trips,
//! since it needs access to `Page`'s private fields.
//!
//! ## Design: typed *responses*, not exceptions, for "nothing here"
//!
//! [`SelectorResolution`] has three outcomes —
//! [`Resolved`](SelectorResolution::Resolved),
//! [`Empty`](SelectorResolution::Empty),
//! [`Ambiguous`](SelectorResolution::Ambiguous) — and all three are `Ok(...)`.
//! A selector that matches nothing, or a kind this module doesn't resolve to a
//! rectangle (`jsonld`, `regex` — see below), is exactly as valid an outcome as
//! a match: the caller decides what "nothing here" means for their workflow
//! instead of catching an exception. `Err` is reserved for genuine infra
//! failures (a CDP call failed, the page evaluated to a JS exception) — never
//! for "no match."
//!
//! [`Page::screenshot`](crate::Page::screenshot)'s convenience `selector`
//! option is the one place this gets converted to an `Err`: a screenshot
//! fundamentally needs a rectangle, so `Empty`/`Ambiguous` there become
//! actionable typed errors ([`VoidCrawlError::ElementNotVisible`] /
//! [`VoidCrawlError::AmbiguousSelector`]) rather than silently cropping an
//! arbitrary target.
//!
//! ## Per-kind resolution
//!
//! | kind | resolution |
//! |---|---|
//! | `css` | `document.querySelectorAll(value)` |
//! | `xpath` | `document.evaluate(value, ...)` ordered node snapshot |
//! | `attr` | same as `css`; `name` is metadata (which attribute holds the datum), not part of the DOM query |
//! | `global_id` | same as `css`, filtered to elements whose `id` starts with `name` (the shared ID-token prefix) when `name` is set |
//! | `role` | `Accessibility.queryAXTree` role + accessible name (exact match, `nth`-disambiguated) — the same semantics as [`Page::click_by_role`](crate::Page::click_by_role) |
//! | `visual` | exact 1x1 CSS-pixel box at `(x, y)` — no invented hit-radius |
//! | `jsonld` | always [`Empty`](SelectorResolution::Empty) — a JSON-LD value lives in a `<script>` tag, which has no render box; this bbox surface is inherently visual, so it doesn't attempt to resolve non-visual data |
//! | `regex` | always [`Empty`](SelectorResolution::Empty) — a raw-HTML text match has no canonical DOM element; not resolved to a visual target |
//!
//! For `css`/`xpath`/`attr`/`global_id`/`role`, candidates are filtered to
//! *visible* ones first — attached to the DOM, non-zero-area, and neither
//! `display:none` nor `visibility:hidden` — before uniqueness is judged:
//! zero visible candidates is `Empty`, exactly one is `Resolved`, and two or
//! more resolve via `nth` (0-based) or are `Ambiguous` without it.

use serde::{Deserialize, Serialize};

use crate::page::Bbox;

/// The 8 selector strategies Yosoi can emit, in its own escalation order
/// (`SelectorLevel` in `yosoi/models/selectors.py`: css=1 ... visual=8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectorKind {
    Css,
    Xpath,
    Regex,
    Jsonld,
    Attr,
    GlobalId,
    Role,
    Visual,
}

/// A Yosoi `SelectorEntry`, mirrored field-for-field
/// (`yosoi/models/selectors.py`) so a caller can pass
/// `entry.model_dump()`/`entry.model_dump_json()` straight through.
///
/// All 8 kinds share this one flat shape rather than a tagged union with
/// per-variant payloads — which fields are meaningful depends on `kind`
/// (see the module docs table). `nth` is 0-based, matching
/// [`Page::click_by_role`](crate::Page::click_by_role)'s convention.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectorEntry {
    #[serde(rename = "type")]
    pub kind:  SelectorKind,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub regex: Option<String>,
    #[serde(default)]
    pub name:  Option<String>,
    #[serde(default)]
    pub nth:   Option<u32>,
    #[serde(default)]
    pub x:     Option<f64>,
    #[serde(default)]
    pub y:     Option<f64>,
}

/// The outcome of resolving a [`SelectorEntry`] to a rectangle. All three
/// variants are a normal, successful `Ok(...)` — see the module docs for
/// why "nothing here" is a typed response rather than an exception.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SelectorResolution {
    /// Exactly one unique, visible target was resolved.
    Resolved { bbox: Bbox },
    /// The selector is well-formed but nothing usable matched: zero
    /// elements, a matched element that's hidden/zero-area/detached, an
    /// out-of-viewport `visual` point, or a kind (`jsonld`, `regex`) this
    /// module never resolves to a rectangle.
    Empty { reason: String },
    /// Two or more visible candidates matched and `nth` wasn't given to
    /// pick one.
    Ambiguous { candidates: usize, reason: String },
}

/// Build the JS expression that gathers this selector's raw DOM candidates,
/// as a JS array of `Element`s — before visibility filtering. `None` for
/// kinds with no DOM candidate-gathering step (`role`, `visual`, `jsonld`,
/// `regex` — each of those is handled without this helper).
pub(crate) fn candidates_js(entry: &SelectorEntry) -> Option<String> {
    match entry.kind {
        SelectorKind::Css | SelectorKind::Attr => {
            Some(format!("Array.from(document.querySelectorAll({:?}))", entry.value))
        }
        SelectorKind::GlobalId => {
            let base = format!("Array.from(document.querySelectorAll({:?}))", entry.value);
            match &entry.name {
                Some(prefix) => {
                    Some(format!("{base}.filter(el => (el.id || '').startsWith({prefix:?}))"))
                }
                None => Some(base),
            }
        }
        SelectorKind::Xpath => Some(format!(
            "(() => {{ \
               const r = document.evaluate({:?}, document, null, \
                 XPathResult.ORDERED_NODE_SNAPSHOT_TYPE, null); \
               const out = []; \
               for (let i = 0; i < r.snapshotLength; i++) out.push(r.snapshotItem(i)); \
               return out; \
             }})()",
            entry.value
        )),
        SelectorKind::Role | SelectorKind::Visual | SelectorKind::Jsonld | SelectorKind::Regex => {
            None
        }
    }
}

/// Wrap a candidate-gathering expression (from [`candidates_js`]) in the
/// shared visibility filter + rect-extraction tail. Returns a JS array of
/// `{x, y, width, height}` (CSS pixels, viewport-relative — the same frame
/// [`Bbox`] already uses) for every *visible* candidate, in DOM order:
/// attached to the document, non-zero area, and neither `display:none` nor
/// `visibility:hidden`.
pub(crate) fn visible_rects_js(candidates_expr: &str) -> String {
    format!(
        "(() => {{ \
           const els = {candidates_expr}; \
           return els.filter(el => {{ \
             if (!el || !el.isConnected) return false; \
             const style = window.getComputedStyle(el); \
             if (!style || style.display === 'none' || style.visibility === 'hidden') return false; \
             const r = el.getBoundingClientRect(); \
             return r.width > 0 && r.height > 0; \
           }}).map(el => {{ \
             const r = el.getBoundingClientRect(); \
             return {{ x: r.x, y: r.y, width: r.width, height: r.height }}; \
           }}); \
         }})()"
    )
}

/// A raw `{x, y, width, height}` rect as returned by [`visible_rects_js`],
/// before rounding into the integer-pixel [`Bbox`] the screenshot crop path
/// wants.
#[derive(Debug, Clone, Copy, Deserialize)]
pub(crate) struct RawRect {
    pub x:      f64,
    pub y:      f64,
    pub width:  f64,
    pub height: f64,
}

impl RawRect {
    /// Round to the integer CSS-pixel [`Bbox`] `Page::screenshot`'s crop
    /// path expects. Floors the origin and ceils the extent so the crop
    /// never clips a fractional-pixel edge of the target.
    pub(crate) fn to_bbox(self) -> Bbox {
        let x = self.x.floor().max(0.0);
        let y = self.y.floor().max(0.0);
        let width = (self.x + self.width - x).ceil().max(1.0);
        let height = (self.y + self.height - y).ceil().max(1.0);
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "CSS pixel coordinates are always small non-negative values in practice"
        )]
        Bbox { x: x as u32, y: y as u32, width: width as u32, height: height as u32 }
    }
}

/// Pick the resolved outcome from a set of visible-candidate rects and an
/// optional `nth`. Shared by every DOM-candidate kind (`css`, `xpath`,
/// `attr`, `global_id`) — `role` has its own AX-specific version of this
/// same logic in `page.rs` since it disambiguates AX nodes, not rects.
pub(crate) fn pick_resolution(
    total_matches: usize,
    visible: &[RawRect],
    nth: Option<u32>,
    describe: impl Fn() -> String,
) -> SelectorResolution {
    if visible.is_empty() {
        let reason = if total_matches == 0 {
            format!("{} matched no elements", describe())
        } else {
            format!(
                "{} matched {total_matches} element(s), but none are visible \
                 (hidden, zero-area, or detached)",
                describe()
            )
        };
        return SelectorResolution::Empty { reason };
    }
    if let Some(n) = nth {
        let idx = n as usize;
        return match visible.get(idx) {
            Some(rect) => SelectorResolution::Resolved { bbox: rect.to_bbox() },
            None => SelectorResolution::Empty {
                reason: format!(
                    "nth={n} out of range: {} visible match(es) for {}",
                    visible.len(),
                    describe()
                ),
            },
        };
    }
    match visible.len() {
        1 => SelectorResolution::Resolved { bbox: visible[0].to_bbox() },
        n => SelectorResolution::Ambiguous {
            candidates: n,
            reason:     format!(
                "{n} visible matches for {}; pass `nth` to disambiguate",
                describe()
            ),
        },
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test harness")]
mod tests {
    use super::*;

    fn rect(x: f64, y: f64, w: f64, h: f64) -> RawRect {
        RawRect { x, y, width: w, height: h }
    }

    #[test]
    fn candidates_js_css_and_attr_use_plain_query_selector_all() {
        let css = SelectorEntry {
            kind:  SelectorKind::Css,
            value: "h1".into(),
            regex: None,
            name:  None,
            nth:   None,
            x:     None,
            y:     None,
        };
        let js = candidates_js(&css).expect("css produces a candidate expression");
        assert!(js.contains("querySelectorAll"));
        assert!(js.contains("\"h1\""));
    }

    #[test]
    fn candidates_js_global_id_filters_by_prefix() {
        let entry = SelectorEntry {
            kind:  SelectorKind::GlobalId,
            value: "tr.athing".into(),
            regex: None,
            name:  Some("score_".into()),
            nth:   None,
            x:     None,
            y:     None,
        };
        let js = candidates_js(&entry).expect("global_id produces a candidate expression");
        assert!(js.contains("startsWith"));
        assert!(js.contains("\"score_\""));
    }

    #[test]
    fn candidates_js_role_visual_jsonld_regex_are_none() {
        for kind in
            [SelectorKind::Role, SelectorKind::Visual, SelectorKind::Jsonld, SelectorKind::Regex]
        {
            let entry = SelectorEntry {
                kind,
                value: String::new(),
                regex: None,
                name: None,
                nth: None,
                x: None,
                y: None,
            };
            assert!(candidates_js(&entry).is_none(), "{kind:?} should have no DOM candidate step");
        }
    }

    #[test]
    fn pick_resolution_zero_matches_is_empty_with_zero_reason() {
        let outcome = pick_resolution(0, &[], None, || "css \"h1\"".into());
        match outcome {
            SelectorResolution::Empty { reason } => assert!(reason.contains("matched no elements")),
            other => panic!("expected Empty, got {other:?}"),
        }
    }

    #[test]
    fn pick_resolution_matches_but_none_visible_is_empty_with_visibility_reason() {
        let outcome = pick_resolution(3, &[], None, || "css \".x\"".into());
        match outcome {
            SelectorResolution::Empty { reason } => {
                assert!(reason.contains("none are visible"), "got: {reason}");
            }
            other => panic!("expected Empty, got {other:?}"),
        }
    }

    #[test]
    fn pick_resolution_single_visible_is_resolved() {
        let outcome = pick_resolution(1, &[rect(1.0, 2.0, 10.0, 20.0)], None, || "x".into());
        match outcome {
            SelectorResolution::Resolved { bbox } => {
                assert_eq!(bbox, Bbox { x: 1, y: 2, width: 10, height: 20 });
            }
            other => panic!("expected Resolved, got {other:?}"),
        }
    }

    #[test]
    fn pick_resolution_multiple_without_nth_is_ambiguous() {
        let outcome = pick_resolution(
            2,
            &[rect(0.0, 0.0, 5.0, 5.0), rect(10.0, 10.0, 5.0, 5.0)],
            None,
            || "css \".x\"".into(),
        );
        match outcome {
            SelectorResolution::Ambiguous { candidates, .. } => assert_eq!(candidates, 2),
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn pick_resolution_multiple_with_nth_resolves_that_index() {
        let outcome = pick_resolution(
            2,
            &[rect(0.0, 0.0, 5.0, 5.0), rect(10.0, 10.0, 6.0, 7.0)],
            Some(1),
            || "css \".x\"".into(),
        );
        match outcome {
            SelectorResolution::Resolved { bbox } => {
                assert_eq!(bbox, Bbox { x: 10, y: 10, width: 6, height: 7 });
            }
            other => panic!("expected Resolved, got {other:?}"),
        }
    }

    #[test]
    fn pick_resolution_nth_out_of_range_is_empty() {
        let outcome =
            pick_resolution(1, &[rect(0.0, 0.0, 5.0, 5.0)], Some(5), || "css \".x\"".into());
        match outcome {
            SelectorResolution::Empty { reason } => assert!(reason.contains("out of range")),
            other => panic!("expected Empty, got {other:?}"),
        }
    }

    #[test]
    fn raw_rect_to_bbox_rounds_outward() {
        let bbox = RawRect { x: 1.2, y: 2.8, width: 10.1, height: 5.9 }.to_bbox();
        // origin floors to 1,2; far edge is 1.2+10.1=11.3 -> ceil width from
        // floored origin = ceil(11.3 - 1) = 11; 2.8+5.9=8.7 -> ceil(8.7-2)=7.
        assert_eq!(bbox, Bbox { x: 1, y: 2, width: 11, height: 7 });
    }
}
