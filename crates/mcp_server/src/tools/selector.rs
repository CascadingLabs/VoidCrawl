//! Selector-backed screenshot bbox: the MCP `selector` arg type for
//! `screenshot`/`session_screenshot`, mirroring
//! [`void_crawl_core::SelectorEntry`] (itself a field-for-field mirror of
//! Yosoi's `SelectorEntry` — `yosoi/models/selectors.py`) with a
//! JSON-Schema-friendly shape for the MCP tool surface. See
//! `void_crawl_core::selector`'s module docs for the full resolution
//! design (CAS-252): three typed outcomes (resolved/empty/ambiguous), none
//! of them exceptions at the core layer — `screenshot`'s `selector` option
//! converts a non-resolved outcome into the `invalid_params` errors below.

use schemars::JsonSchema;
use serde::Deserialize;
use void_crawl_core::{SelectorEntry, SelectorKind};

/// The 8 selector strategies Yosoi can emit. See
/// `void_crawl_core::selector`'s module docs for how each kind resolves to
/// a rectangle (or deliberately doesn't — `jsonld`/`regex` are always
/// non-visual).
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SelectorKindArg {
    Css,
    Xpath,
    Regex,
    Jsonld,
    Attr,
    GlobalId,
    Role,
    Visual,
}

impl From<SelectorKindArg> for SelectorKind {
    fn from(kind: SelectorKindArg) -> Self {
        match kind {
            SelectorKindArg::Css => Self::Css,
            SelectorKindArg::Xpath => Self::Xpath,
            SelectorKindArg::Regex => Self::Regex,
            SelectorKindArg::Jsonld => Self::Jsonld,
            SelectorKindArg::Attr => Self::Attr,
            SelectorKindArg::GlobalId => Self::GlobalId,
            SelectorKindArg::Role => Self::Role,
            SelectorKindArg::Visual => Self::Visual,
        }
    }
}

/// A Yosoi `SelectorEntry`, field-for-field — pass `entry.model_dump()`
/// straight through as this arg's JSON. Which fields matter depends on
/// `type`:
///
/// | type | uses |
/// |---|---|
/// | `css` | `value` (CSS selector) |
/// | `xpath` | `value` (XPath expression) |
/// | `attr` | `value` (CSS selector); `name` is metadata (which attribute holds the datum), not part of the query |
/// | `global_id` | `value` (CSS selector); `name` filters matches to elements whose `id` starts with this prefix |
/// | `role` | `value` (ARIA role); `name` (accessible name, exact match) |
/// | `visual` | `x`, `y` (CSS pixels) — resolves to an exact 1x1 box |
/// | `jsonld`, `regex` | always resolve to "empty" — non-visual by nature, not cropped |
///
/// `nth` (0-based) disambiguates when a `css`/`xpath`/`attr`/`global_id`/
/// `role` selector matches more than one visible target.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SelectorArg {
    #[serde(rename = "type")]
    pub kind:  SelectorKindArg,
    #[serde(default)]
    pub value: Option<String>,
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

impl From<SelectorArg> for SelectorEntry {
    fn from(arg: SelectorArg) -> Self {
        Self {
            kind:  arg.kind.into(),
            value: arg.value.unwrap_or_default(),
            regex: arg.regex,
            name:  arg.name,
            nth:   arg.nth,
            x:     arg.x,
            y:     arg.y,
        }
    }
}
