//! Compact rendered-page snapshots for MCP clients.
//!
//! This is intentionally a lossy perception surface. It gives agents enough
//! structure to orient on large rendered pages without pulling raw HTML into
//! context; schema-grade extraction remains a caller concern.

use std::time::{Duration, Instant};

use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use void_crawl_core::{AntibotVerdict, Page, VoidCrawlError};

use crate::{
    errors::map_err,
    server::VoidCrawlServer,
    sessions::DedicatedSession,
    tools::{fetch::AntibotInfo, wait},
};

pub const DEFAULT_TIMEOUT_SECS: u64 = 30;
pub const DEFAULT_MAX_CHARS: usize = 12_000;
pub const HARD_MAX_CHARS: usize = 60_000;

#[derive(Debug, Clone, Deserialize, JsonSchema, Default)]
pub struct FetchSnapshotArgs {
    /// Absolute URL to load.
    pub url:          String,
    /// Optional wait strategy: "networkidle" (default) or "selector:<css>".
    #[serde(default)]
    pub wait_for:     Option<String>,
    /// Navigation + wait timeout in seconds (default 30).
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    /// Approximate character budget for returned snapshot sections.
    /// Defaults to 12000 and is hard-capped at 60000.
    #[serde(default)]
    pub max_chars:    Option<usize>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema, Default)]
pub struct SessionSnapshotArgs {
    pub session_id: String,
    /// Approximate character budget for returned snapshot sections.
    /// Defaults to 12000 and is hard-capped at 60000.
    #[serde(default)]
    pub max_chars:  Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct HeadingSnapshot {
    pub level: u8,
    pub text:  String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct TextBlockSnapshot {
    pub tag:  String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct LinkSnapshot {
    pub text: String,
    pub href: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ControlSnapshot {
    pub tag:         String,
    #[serde(default)]
    pub r#type:      Option<String>,
    #[serde(default)]
    pub role:        Option<String>,
    #[serde(default)]
    pub name:        Option<String>,
    #[serde(default)]
    pub placeholder: Option<String>,
    #[serde(default)]
    pub disabled:    bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct FormSnapshot {
    #[serde(default)]
    pub action:   Option<String>,
    #[serde(default)]
    pub method:   Option<String>,
    #[serde(default)]
    pub controls: Vec<ControlSnapshot>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, Default)]
pub struct SnapshotCounts {
    pub headings:    usize,
    pub text_blocks: usize,
    pub links:       usize,
    pub controls:    usize,
    pub forms:       usize,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SnapshotStats {
    pub max_chars:      usize,
    pub total_chars:    usize,
    pub returned_chars: usize,
    pub truncated:      bool,
    pub total:          SnapshotCounts,
    pub returned:       SnapshotCounts,
    pub omitted:        SnapshotCounts,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PageSnapshot {
    pub url:         String,
    pub title:       Option<String>,
    pub status_code: Option<u16>,
    pub redirected:  Option<bool>,
    pub antibot:     Option<AntibotInfo>,
    pub headings:    Vec<HeadingSnapshot>,
    pub text_blocks: Vec<TextBlockSnapshot>,
    pub links:       Vec<LinkSnapshot>,
    pub controls:    Vec<ControlSnapshot>,
    pub forms:       Vec<FormSnapshot>,
    pub stats:       SnapshotStats,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct RawSnapshot {
    #[serde(default)]
    url:         String,
    #[serde(default)]
    title:       Option<String>,
    #[serde(default)]
    headings:    Vec<HeadingSnapshot>,
    #[serde(default)]
    text_blocks: Vec<TextBlockSnapshot>,
    #[serde(default)]
    links:       Vec<LinkSnapshot>,
    #[serde(default)]
    controls:    Vec<ControlSnapshot>,
    #[serde(default)]
    forms:       Vec<FormSnapshot>,
    #[serde(default)]
    total:       SnapshotCounts,
}

#[derive(Debug, Default)]
struct SnapshotMeta {
    status_code: Option<u16>,
    redirected:  Option<bool>,
    antibot:     Option<AntibotInfo>,
}

pub async fn fetch(
    server: &VoidCrawlServer,
    args: FetchSnapshotArgs,
) -> Result<PageSnapshot, VoidCrawlError> {
    let pool = server.state().pool().await?;
    let tab = pool.acquire().await?;
    let result = fetch_on_tab(&tab.page, args).await;
    pool.release(tab).await;
    result
}

async fn fetch_on_tab(
    page: &Page,
    args: FetchSnapshotArgs,
) -> Result<PageSnapshot, VoidCrawlError> {
    let total_timeout = Duration::from_secs(args.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS));
    let start = Instant::now();
    let resp = page.goto_and_wait_for_idle(&args.url, total_timeout).await?;
    wait::apply_post_navigate(page, args.wait_for.as_deref(), total_timeout).await?;
    let remaining = total_timeout.saturating_sub(start.elapsed());
    let raw = tokio::time::timeout(remaining, collect_raw(page))
        .await
        .map_err(|_| VoidCrawlError::Timeout("snapshot read exceeded timeout_secs".into()))??;
    let meta = SnapshotMeta {
        status_code: resp.status_code,
        redirected:  Some(resp.redirected),
        antibot:     resp.antibot.filter(AntibotVerdict::detected).map(AntibotInfo::from),
    };
    Ok(apply_budget(raw, meta, args.max_chars))
}

pub async fn session(
    server: &VoidCrawlServer,
    args: SessionSnapshotArgs,
) -> Result<PageSnapshot, ErrorData> {
    let handle = lookup(server, &args.session_id).await?;
    let last_navigation = handle.last_navigation.lock().await.clone();
    let meta = SnapshotMeta {
        status_code: last_navigation.as_ref().and_then(|nav| nav.status_code),
        redirected:  None,
        antibot:     last_navigation
            .and_then(|nav| nav.antibot)
            .filter(AntibotVerdict::detected)
            .map(AntibotInfo::from),
    };
    let page = handle.page.lock().await;
    let raw = collect_raw(&page).await.map_err(map_err)?;
    Ok(apply_budget(raw, meta, args.max_chars))
}

async fn lookup(
    server: &VoidCrawlServer,
    id: &str,
) -> Result<std::sync::Arc<DedicatedSession>, ErrorData> {
    server
        .state()
        .sessions
        .get(id)
        .await
        .ok_or_else(|| ErrorData::invalid_params(format!("unknown session_id: {id}"), None))
}

async fn collect_raw(page: &Page) -> Result<RawSnapshot, VoidCrawlError> {
    let value = page.evaluate_js(SNAPSHOT_JS).await?;
    serde_json::from_value(value)
        .map_err(|e| VoidCrawlError::JsEvalError(format!("snapshot decode failed: {e}")))
}

fn apply_budget(
    raw: RawSnapshot,
    meta: SnapshotMeta,
    requested_max_chars: Option<usize>,
) -> PageSnapshot {
    let max_chars = requested_max_chars.unwrap_or(DEFAULT_MAX_CHARS).min(HARD_MAX_CHARS);
    let total = normalize_totals(&raw);
    let total_chars = chars_headings(&raw.headings)
        + chars_text_blocks(&raw.text_blocks)
        + chars_links(&raw.links)
        + chars_controls(&raw.controls)
        + chars_forms(&raw.forms);

    let mut returned_chars = 0usize;
    let headings = take_entries(raw.headings, max_chars, &mut returned_chars, heading_chars);
    let text_blocks =
        take_entries(raw.text_blocks, max_chars, &mut returned_chars, text_block_chars);
    let links = take_entries(raw.links, max_chars, &mut returned_chars, link_chars);
    let controls = take_entries(raw.controls, max_chars, &mut returned_chars, control_chars);
    let forms = take_entries(raw.forms, max_chars, &mut returned_chars, form_chars);

    let returned = SnapshotCounts {
        headings:    headings.len(),
        text_blocks: text_blocks.len(),
        links:       links.len(),
        controls:    controls.len(),
        forms:       forms.len(),
    };
    let omitted = SnapshotCounts {
        headings:    total.headings.saturating_sub(returned.headings),
        text_blocks: total.text_blocks.saturating_sub(returned.text_blocks),
        links:       total.links.saturating_sub(returned.links),
        controls:    total.controls.saturating_sub(returned.controls),
        forms:       total.forms.saturating_sub(returned.forms),
    };
    let truncated = returned_chars < total_chars
        || omitted.headings > 0
        || omitted.text_blocks > 0
        || omitted.links > 0
        || omitted.controls > 0
        || omitted.forms > 0
        || requested_max_chars.is_some_and(|n| n > HARD_MAX_CHARS);

    PageSnapshot {
        url: raw.url,
        title: raw.title,
        status_code: meta.status_code,
        redirected: meta.redirected,
        antibot: meta.antibot,
        headings,
        text_blocks,
        links,
        controls,
        forms,
        stats: SnapshotStats {
            max_chars,
            total_chars,
            returned_chars,
            truncated,
            total,
            returned,
            omitted,
        },
    }
}

fn normalize_totals(raw: &RawSnapshot) -> SnapshotCounts {
    SnapshotCounts {
        headings:    raw.total.headings.max(raw.headings.len()),
        text_blocks: raw.total.text_blocks.max(raw.text_blocks.len()),
        links:       raw.total.links.max(raw.links.len()),
        controls:    raw.total.controls.max(raw.controls.len()),
        forms:       raw.total.forms.max(raw.forms.len()),
    }
}

fn take_entries<T>(
    entries: Vec<T>,
    max_chars: usize,
    returned_chars: &mut usize,
    chars: fn(&T) -> usize,
) -> Vec<T> {
    let mut kept = Vec::new();
    for entry in entries {
        let entry_chars = chars(&entry);
        if *returned_chars + entry_chars > max_chars {
            break;
        }
        *returned_chars += entry_chars;
        kept.push(entry);
    }
    kept
}

fn chars(s: &str) -> usize {
    s.chars().count()
}

fn opt_chars(s: &Option<String>) -> usize {
    s.as_deref().map_or(0, chars)
}

fn heading_chars(h: &HeadingSnapshot) -> usize {
    chars(&h.text) + 2
}

fn text_block_chars(t: &TextBlockSnapshot) -> usize {
    chars(&t.tag) + chars(&t.text)
}

fn link_chars(l: &LinkSnapshot) -> usize {
    chars(&l.text) + chars(&l.href)
}

fn control_chars(c: &ControlSnapshot) -> usize {
    chars(&c.tag)
        + opt_chars(&c.r#type)
        + opt_chars(&c.role)
        + opt_chars(&c.name)
        + opt_chars(&c.placeholder)
        + usize::from(c.disabled)
}

fn form_chars(f: &FormSnapshot) -> usize {
    opt_chars(&f.action) + opt_chars(&f.method) + chars_controls(&f.controls)
}

fn chars_headings(v: &[HeadingSnapshot]) -> usize {
    v.iter().map(heading_chars).sum()
}

fn chars_text_blocks(v: &[TextBlockSnapshot]) -> usize {
    v.iter().map(text_block_chars).sum()
}

fn chars_links(v: &[LinkSnapshot]) -> usize {
    v.iter().map(link_chars).sum()
}

fn chars_controls(v: &[ControlSnapshot]) -> usize {
    v.iter().map(control_chars).sum()
}

fn chars_forms(v: &[FormSnapshot]) -> usize {
    v.iter().map(form_chars).sum()
}

const SNAPSHOT_JS: &str = r#"
(() => {
  const MAX = {
    headings: 80,
    textBlocks: 240,
    links: 160,
    controls: 160,
    forms: 60,
    formControls: 30,
    textChars: 700,
    smallChars: 220
  };
  const clean = (value) => String(value || '').replace(/\s+/g, ' ').trim();
  const clip = (value, limit) => {
    const text = clean(value);
    return text.length > limit ? text.slice(0, Math.max(0, limit - 3)) + '...' : text;
  };
  const visible = (el) => {
    if (!el || !el.isConnected) return false;
    const style = window.getComputedStyle(el);
    if (!style || style.display === 'none' || style.visibility === 'hidden') return false;
    const rect = el.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0;
  };
  const attr = (el, name) => {
    const value = el.getAttribute(name);
    return value == null || value === '' ? null : clip(value, MAX.smallChars);
  };
  const labelText = (el) => {
    const id = el.id ? CSS.escape(el.id) : null;
    const label = id ? document.querySelector(`label[for="${id}"]`) : null;
    return clip(
      el.getAttribute('aria-label')
        || el.getAttribute('title')
        || el.getAttribute('placeholder')
        || (label && label.textContent)
        || el.value
        || el.textContent
        || el.name
        || '',
      MAX.smallChars
    );
  };
  const control = (el) => ({
    tag: el.tagName.toLowerCase(),
    type: attr(el, 'type'),
    role: attr(el, 'role'),
    name: labelText(el) || null,
    placeholder: attr(el, 'placeholder'),
    disabled: Boolean(el.disabled || el.getAttribute('aria-disabled') === 'true')
  });
  const all = (selector) => Array.from(document.querySelectorAll(selector)).filter(visible);
  const unique = (items) => Array.from(new Set(items));

  const headingNodes = all('h1,h2,h3,h4,h5,h6');
  const headings = headingNodes.slice(0, MAX.headings).map((el) => ({
    level: Number(el.tagName.slice(1)),
    text: clip(el.textContent, MAX.smallChars)
  })).filter((h) => h.text);

  const textNodes = unique([
    ...all('main p, main li, article p, article li, section p, blockquote, body > p, td, th'),
    ...all('[role="main"] p, [role="article"] p')
  ]).filter((el) => clean(el.textContent).length >= 20);
  const text_blocks = textNodes.slice(0, MAX.textBlocks).map((el) => ({
    tag: el.tagName.toLowerCase(),
    text: clip(el.textContent, MAX.textChars)
  })).filter((b) => b.text);

  const linkNodes = all('a[href]');
  const links = linkNodes.slice(0, MAX.links).map((el) => ({
    text: clip(el.textContent || el.getAttribute('aria-label') || el.href, MAX.smallChars),
    href: clip(el.href, MAX.smallChars)
  })).filter((l) => l.href);

  const controlNodes = all('button,input,select,textarea,[role="button"],[role="link"],[role="textbox"],[role="combobox"],[contenteditable="true"]');
  const controls = controlNodes.slice(0, MAX.controls).map(control);

  const formNodes = all('form');
  const forms = formNodes.slice(0, MAX.forms).map((form) => {
    const fields = Array.from(form.querySelectorAll('button,input,select,textarea,[role="button"],[role="textbox"],[role="combobox"]'))
      .filter(visible)
      .slice(0, MAX.formControls)
      .map(control);
    return {
      action: attr(form, 'action') || (form.action ? clip(form.action, MAX.smallChars) : null),
      method: clip(form.method || 'get', 20).toLowerCase(),
      controls: fields
    };
  });

  return {
    url: location.href,
    title: document.title || null,
    headings,
    text_blocks,
    links,
    controls,
    forms,
    total: {
      headings: headingNodes.length,
      text_blocks: textNodes.length,
      links: linkNodes.length,
      controls: controlNodes.length,
      forms: formNodes.length
    }
  };
})()
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_with_text_blocks(count: usize) -> RawSnapshot {
        RawSnapshot {
            url: "data:text/html,test".into(),
            title: Some("Test".into()),
            text_blocks: (0..count)
                .map(|i| TextBlockSnapshot {
                    tag:  "p".into(),
                    text: format!("block {i} abcdefghijklmnopqrstuvwxyz"),
                })
                .collect(),
            total: SnapshotCounts { text_blocks: count, ..SnapshotCounts::default() },
            ..RawSnapshot::default()
        }
    }

    #[test]
    fn max_chars_defaults_and_caps() {
        let snapshot = apply_budget(raw_with_text_blocks(1), SnapshotMeta::default(), None);
        assert_eq!(snapshot.stats.max_chars, DEFAULT_MAX_CHARS);

        let snapshot = apply_budget(
            raw_with_text_blocks(10),
            SnapshotMeta::default(),
            Some(HARD_MAX_CHARS + 1),
        );
        assert_eq!(snapshot.stats.max_chars, HARD_MAX_CHARS);
        assert!(snapshot.stats.truncated);
    }

    #[test]
    fn budget_enforces_returned_chars_and_omission_counts() {
        let snapshot = apply_budget(raw_with_text_blocks(20), SnapshotMeta::default(), Some(120));

        assert!(snapshot.stats.returned_chars <= 120);
        assert!(snapshot.stats.truncated);
        assert_eq!(snapshot.stats.total.text_blocks, 20);
        assert_eq!(snapshot.stats.omitted.text_blocks, 20 - snapshot.stats.returned.text_blocks);
        assert_eq!(snapshot.text_blocks.len(), snapshot.stats.returned.text_blocks);
    }
}
