//! Effective, secret-safe browser environment and capability observations.
//!
//! These are provider-native facts from VoidCrawl, not Yosoi capture-domain
//! types. A higher-level adapter decides how to map them into a durable
//! capture.

use chromiumoxide::{CdpMode, cdp::browser_protocol::browser::GetVersionReturns};
use serde::Serialize;
use serde_json::Value;

/// A browser fact that was observed, unavailable, or deliberately omitted.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum EnvironmentObservation<T> {
    Known { value: T },
    Unavailable { reason: EnvironmentUnavailableReason },
    Omitted { reason: EnvironmentOmissionReason },
}

/// Why VoidCrawl could not report an environment fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentUnavailableReason {
    /// VoidCrawl attached to a browser it did not launch and cannot prove the
    /// requested fact from its own configuration.
    AttachedBrowserNotControlled,
    /// Chromium did not return the requested fact.
    BrowserDidNotReport,
    /// Chromium returned a value that could not be represented honestly.
    InvalidBrowserValue,
}

/// Why VoidCrawl deliberately left an environment fact out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentOmissionReason {
    /// The observation was skipped to avoid additional browser instrumentation.
    MinimizeInstrumentation,
    /// The value was withheld because it may contain sensitive information.
    SensitiveValue,
}

/// VoidCrawl build identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ControllerVersion {
    pub name:    String,
    pub version: String,
}

impl ControllerVersion {
    pub(crate) fn current() -> Self {
        Self {
            name:    env!("CARGO_PKG_NAME").to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

/// Renderer and protocol identity reported by `Browser.getVersion`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RendererVersion {
    pub product:          String,
    pub revision:         String,
    pub protocol_version: String,
    pub js_version:       String,
}

impl From<GetVersionReturns> for RendererVersion {
    fn from(value: GetVersionReturns) -> Self {
        Self {
            product:          value.product,
            revision:         value.revision,
            protocol_version: value.protocol_version,
            js_version:       value.js_version,
        }
    }
}

/// Effective windowing mode when VoidCrawl launched the browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserVisibilityMode {
    Headless,
    Headful,
}

/// Effective CSS viewport dimensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct EffectiveViewport {
    pub width_css_pixels:  u32,
    pub height_css_pixels: u32,
}

/// Effective `prefers-color-scheme` value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectiveColorScheme {
    Light,
    Dark,
    NoPreference,
}

/// Effective `prefers-reduced-motion` value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectiveReducedMotion {
    Reduce,
    NoPreference,
}

/// Representation-affecting facts observed in the page's main world.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BrowserRenderingEnvironment {
    pub viewport:            EnvironmentObservation<EffectiveViewport>,
    pub device_scale_factor: EnvironmentObservation<f64>,
    pub user_agent:          EnvironmentObservation<String>,
    pub locale:              EnvironmentObservation<String>,
    pub timezone:            EnvironmentObservation<String>,
    pub color_scheme:        EnvironmentObservation<EffectiveColorScheme>,
    pub reduced_motion:      EnvironmentObservation<EffectiveReducedMotion>,
}

/// Configured chromiumoxide initialization mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstrumentationMode {
    Normal,
    Minimal,
}

impl From<CdpMode> for InstrumentationMode {
    fn from(value: CdpMode) -> Self {
        match value {
            CdpMode::Normal => Self::Normal,
            CdpMode::Minimal => Self::Minimal,
        }
    }
}

/// Effective CDP instrumentation known for one page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "each flag reports an independent CDP domain or page-state fact"
)]
pub struct InstrumentationSnapshot {
    pub configured_mode:        InstrumentationMode,
    pub network_enabled:        bool,
    pub runtime_enabled:        bool,
    pub performance_enabled:    bool,
    pub log_enabled:            bool,
    pub target_auto_attach:     bool,
    pub utility_world_enabled:  bool,
    pub pre_navigation_stealth: bool,
    pub attached_browser:       bool,
    pub escalated_from_minimal: bool,
}

impl InstrumentationSnapshot {
    #[allow(
        clippy::fn_params_excessive_bools,
        reason = "inputs mirror independent CDP domain and page-state facts"
    )]
    pub(crate) fn for_page(
        mode: CdpMode,
        network_enabled_lazily: bool,
        runtime_enabled_lazily: bool,
        pre_navigation_stealth: bool,
        attached_browser: bool,
    ) -> Self {
        let normal = matches!(mode, CdpMode::Normal);
        let network_enabled = normal || network_enabled_lazily;
        let runtime_enabled = normal || runtime_enabled_lazily;
        Self {
            configured_mode: mode.into(),
            network_enabled,
            runtime_enabled,
            performance_enabled: normal,
            log_enabled: normal,
            target_auto_attach: normal,
            utility_world_enabled: normal,
            pre_navigation_stealth,
            attached_browser,
            escalated_from_minimal: matches!(mode, CdpMode::Minimal)
                && (network_enabled_lazily || runtime_enabled_lazily),
        }
    }
}

/// Whether a concrete VoidCrawl primitive is usable in the active page state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CapabilityState {
    Supported,
    Unavailable { reason: CapabilityUnavailableReason },
    Disabled { reason: CapabilityDisabledReason },
    Unsupported { reason: CapabilityUnsupportedReason },
}

/// Why VoidCrawl cannot establish whether a primitive is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityUnavailableReason {
    AttachedBrowserStateNotControlled,
}

/// Why a normally available primitive is disabled for this page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityDisabledReason {
    MinimalCdpMode,
}

/// Why the current VoidCrawl build does not expose a primitive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityUnsupportedReason {
    NotImplemented,
}

/// Exhaustive capabilities of the current VoidCrawl browser primitive set.
///
/// These are intentionally more concrete than Yosoi artifact families. For
/// example, selected response bodies do not claim support for a complete
/// document-source artifact or bounded network resource graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BrowserCaptureCapabilities {
    pub document_navigation:        CapabilityState,
    pub document_response_metadata: CapabilityState,
    pub selected_response_bodies:   CapabilityState,
    pub document_source:            CapabilityState,
    pub rendered_dom:               CapabilityState,
    pub accessibility_tree:         CapabilityState,
    pub network_resource_graph:     CapabilityState,
    pub cookies:                    CapabilityState,
    pub web_storage:                CapabilityState,
    pub element_geometry:           CapabilityState,
    pub screenshot:                 CapabilityState,
    pub screencast:                 CapabilityState,
    pub runtime_diagnostics:        CapabilityState,
}

impl BrowserCaptureCapabilities {
    pub(crate) fn from_instrumentation(instrumentation: InstrumentationSnapshot) -> Self {
        let network = if instrumentation.network_enabled {
            CapabilityState::Supported
        } else if instrumentation.attached_browser {
            CapabilityState::Unavailable {
                reason: CapabilityUnavailableReason::AttachedBrowserStateNotControlled,
            }
        } else {
            CapabilityState::Disabled { reason: CapabilityDisabledReason::MinimalCdpMode }
        };
        let runtime = if instrumentation.runtime_enabled {
            CapabilityState::Supported
        } else if instrumentation.attached_browser {
            CapabilityState::Unavailable {
                reason: CapabilityUnavailableReason::AttachedBrowserStateNotControlled,
            }
        } else {
            CapabilityState::Disabled { reason: CapabilityDisabledReason::MinimalCdpMode }
        };
        let not_implemented =
            CapabilityState::Unsupported { reason: CapabilityUnsupportedReason::NotImplemented };
        Self {
            document_navigation:        CapabilityState::Supported,
            document_response_metadata: network,
            selected_response_bodies:   network,
            document_source:            network,
            rendered_dom:               CapabilityState::Supported,
            accessibility_tree:         CapabilityState::Supported,
            network_resource_graph:     network,
            cookies:                    CapabilityState::Supported,
            web_storage:                not_implemented,
            element_geometry:           CapabilityState::Supported,
            screenshot:                 CapabilityState::Supported,
            screencast:                 CapabilityState::Supported,
            runtime_diagnostics:        runtime,
        }
    }
}

/// Effective browser environment and active primitive capabilities for a page.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BrowserEnvironmentSnapshot {
    pub controller:      ControllerVersion,
    pub renderer:        RendererVersion,
    pub mode:            EnvironmentObservation<BrowserVisibilityMode>,
    pub rendering:       BrowserRenderingEnvironment,
    pub instrumentation: InstrumentationSnapshot,
    pub capabilities:    BrowserCaptureCapabilities,
}

pub(crate) const ENVIRONMENT_SNAPSHOT_JS: &str = r#"
(() => {
  const dark = window.matchMedia('(prefers-color-scheme: dark)').matches;
  const light = window.matchMedia('(prefers-color-scheme: light)').matches;
  const reduce = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  const intl = Intl.DateTimeFormat().resolvedOptions();
  return {
    viewport_width: window.innerWidth,
    viewport_height: window.innerHeight,
    device_scale_factor: window.devicePixelRatio,
    user_agent: navigator.userAgent,
    locale: intl.locale || navigator.language,
    timezone: intl.timeZone,
    color_scheme: dark ? 'dark' : (light ? 'light' : 'no_preference'),
    reduced_motion: reduce ? 'reduce' : 'no_preference'
  };
})()
"#;

pub(crate) fn rendering_environment(value: &Value) -> BrowserRenderingEnvironment {
    BrowserRenderingEnvironment {
        viewport:            viewport_observation(value),
        device_scale_factor: positive_number(value, "device_scale_factor"),
        user_agent:          non_empty_string(value, "user_agent"),
        locale:              non_empty_string(value, "locale"),
        timezone:            non_empty_string(value, "timezone"),
        color_scheme:        color_scheme_observation(value),
        reduced_motion:      reduced_motion_observation(value),
    }
}

fn viewport_observation(value: &Value) -> EnvironmentObservation<EffectiveViewport> {
    let width = value.get("viewport_width").and_then(Value::as_u64);
    let height = value.get("viewport_height").and_then(Value::as_u64);
    match (width, height) {
        (Some(width), Some(height)) if width > 0 && height > 0 => {
            match (u32::try_from(width), u32::try_from(height)) {
                (Ok(width_css_pixels), Ok(height_css_pixels)) => EnvironmentObservation::Known {
                    value: EffectiveViewport { width_css_pixels, height_css_pixels },
                },
                _ => invalid_value(),
            }
        }
        (None, _) | (_, None) => unavailable(),
        _ => invalid_value(),
    }
}

fn positive_number(value: &Value, key: &str) -> EnvironmentObservation<f64> {
    match value.get(key).and_then(Value::as_f64) {
        Some(value) if value.is_finite() && value > 0.0 => EnvironmentObservation::Known { value },
        Some(_) => invalid_value(),
        None => unavailable(),
    }
}

fn non_empty_string(value: &Value, key: &str) -> EnvironmentObservation<String> {
    match value.get(key).and_then(Value::as_str) {
        Some(value) if !value.is_empty() => {
            EnvironmentObservation::Known { value: value.to_string() }
        }
        Some(_) => invalid_value(),
        None => unavailable(),
    }
}

fn color_scheme_observation(value: &Value) -> EnvironmentObservation<EffectiveColorScheme> {
    match value.get("color_scheme").and_then(Value::as_str) {
        Some("light") => EnvironmentObservation::Known { value: EffectiveColorScheme::Light },
        Some("dark") => EnvironmentObservation::Known { value: EffectiveColorScheme::Dark },
        Some("no_preference") => {
            EnvironmentObservation::Known { value: EffectiveColorScheme::NoPreference }
        }
        Some(_) => invalid_value(),
        None => unavailable(),
    }
}

fn reduced_motion_observation(value: &Value) -> EnvironmentObservation<EffectiveReducedMotion> {
    match value.get("reduced_motion").and_then(Value::as_str) {
        Some("reduce") => EnvironmentObservation::Known { value: EffectiveReducedMotion::Reduce },
        Some("no_preference") => {
            EnvironmentObservation::Known { value: EffectiveReducedMotion::NoPreference }
        }
        Some(_) => invalid_value(),
        None => unavailable(),
    }
}

fn unavailable<T>() -> EnvironmentObservation<T> {
    EnvironmentObservation::Unavailable {
        reason: EnvironmentUnavailableReason::BrowserDidNotReport,
    }
}

fn invalid_value<T>() -> EnvironmentObservation<T> {
    EnvironmentObservation::Unavailable {
        reason: EnvironmentUnavailableReason::InvalidBrowserValue,
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn normal_mode_reports_eager_domains_and_network_capabilities() {
        let instrumentation =
            InstrumentationSnapshot::for_page(CdpMode::Normal, false, false, true, false);
        let capabilities = BrowserCaptureCapabilities::from_instrumentation(instrumentation);

        assert!(instrumentation.network_enabled);
        assert!(instrumentation.runtime_enabled);
        assert!(!instrumentation.escalated_from_minimal);
        assert_eq!(capabilities.document_response_metadata, CapabilityState::Supported);
        assert_eq!(capabilities.selected_response_bodies, CapabilityState::Supported);
    }

    #[test]
    fn untouched_minimal_mode_disables_only_network_dependent_existing_primitives() {
        let instrumentation =
            InstrumentationSnapshot::for_page(CdpMode::Minimal, false, false, false, false);
        let capabilities = BrowserCaptureCapabilities::from_instrumentation(instrumentation);
        let disabled =
            CapabilityState::Disabled { reason: CapabilityDisabledReason::MinimalCdpMode };

        assert!(!instrumentation.network_enabled);
        assert!(!instrumentation.runtime_enabled);
        assert_eq!(capabilities.document_response_metadata, disabled);
        assert_eq!(capabilities.selected_response_bodies, disabled);
        assert_eq!(capabilities.document_source, disabled);
        assert_eq!(capabilities.network_resource_graph, disabled);
        assert_eq!(capabilities.rendered_dom, CapabilityState::Supported);
        assert_eq!(capabilities.accessibility_tree, CapabilityState::Supported);
        assert_eq!(capabilities.screenshot, CapabilityState::Supported);
    }

    #[test]
    fn minimal_mode_reports_lazy_escalation() {
        let instrumentation =
            InstrumentationSnapshot::for_page(CdpMode::Minimal, true, false, false, false);
        let capabilities = BrowserCaptureCapabilities::from_instrumentation(instrumentation);

        assert!(instrumentation.escalated_from_minimal);
        assert!(instrumentation.network_enabled);
        assert_eq!(capabilities.document_response_metadata, CapabilityState::Supported);
    }

    #[test]
    fn runtime_diagnostic_capability_tracks_runtime_instrumentation() {
        let untouched =
            InstrumentationSnapshot::for_page(CdpMode::Minimal, false, false, false, false);
        let enabled =
            InstrumentationSnapshot::for_page(CdpMode::Minimal, false, true, false, false);
        assert_eq!(
            BrowserCaptureCapabilities::from_instrumentation(untouched).runtime_diagnostics,
            CapabilityState::Disabled { reason: CapabilityDisabledReason::MinimalCdpMode },
        );
        assert_eq!(
            BrowserCaptureCapabilities::from_instrumentation(enabled).runtime_diagnostics,
            CapabilityState::Supported,
        );
    }

    #[test]
    fn untouched_attached_minimal_mode_reports_network_state_unavailable() {
        let instrumentation =
            InstrumentationSnapshot::for_page(CdpMode::Minimal, false, false, false, true);
        let capabilities = BrowserCaptureCapabilities::from_instrumentation(instrumentation);
        let unavailable = CapabilityState::Unavailable {
            reason: CapabilityUnavailableReason::AttachedBrowserStateNotControlled,
        };

        assert_eq!(capabilities.document_response_metadata, unavailable);
        assert_eq!(capabilities.selected_response_bodies, unavailable);
    }

    #[test]
    fn unimplemented_capture_families_are_explicitly_unsupported() {
        let instrumentation =
            InstrumentationSnapshot::for_page(CdpMode::Normal, false, false, true, false);
        let capabilities = BrowserCaptureCapabilities::from_instrumentation(instrumentation);
        let unsupported =
            CapabilityState::Unsupported { reason: CapabilityUnsupportedReason::NotImplemented };

        assert_eq!(capabilities.document_source, CapabilityState::Supported);
        assert_eq!(capabilities.network_resource_graph, CapabilityState::Supported);
        assert_eq!(capabilities.web_storage, unsupported);
        assert_eq!(capabilities.runtime_diagnostics, CapabilityState::Supported);
    }

    #[test]
    fn rendering_values_distinguish_known_missing_and_invalid() {
        let rendering = rendering_environment(&json!({
            "viewport_width": 1280,
            "viewport_height": 720,
            "device_scale_factor": 2.0,
            "user_agent": "ExampleBrowser/1",
            "locale": "en-US",
            "timezone": "UTC",
            "color_scheme": "dark",
            "reduced_motion": "bogus"
        }));

        assert!(matches!(rendering.viewport, EnvironmentObservation::Known { .. }));
        assert_eq!(
            rendering.color_scheme,
            EnvironmentObservation::Known { value: EffectiveColorScheme::Dark }
        );
        assert_eq!(
            rendering.reduced_motion,
            EnvironmentObservation::Unavailable {
                reason: EnvironmentUnavailableReason::InvalidBrowserValue,
            }
        );

        let missing = rendering_environment(&Value::Null);
        assert_eq!(
            missing.user_agent,
            EnvironmentObservation::Unavailable {
                reason: EnvironmentUnavailableReason::BrowserDidNotReport,
            }
        );
    }

    #[test]
    fn observation_states_serialize_distinctly() {
        let known = EnvironmentObservation::Known { value: BrowserVisibilityMode::Headful };
        let unavailable: EnvironmentObservation<BrowserVisibilityMode> =
            EnvironmentObservation::Unavailable {
                reason: EnvironmentUnavailableReason::AttachedBrowserNotControlled,
            };
        let omitted: EnvironmentObservation<BrowserVisibilityMode> =
            EnvironmentObservation::Omitted {
                reason: EnvironmentOmissionReason::MinimizeInstrumentation,
            };

        assert_eq!(
            serde_json::to_value(known).expect("serialize known observation"),
            json!({"status": "known", "value": "headful"})
        );
        assert_eq!(
            serde_json::to_value(unavailable).expect("serialize unavailable observation"),
            json!({
                "status": "unavailable",
                "reason": "attached_browser_not_controlled"
            })
        );
        assert_eq!(
            serde_json::to_value(omitted).expect("serialize omitted observation"),
            json!({
                "status": "omitted",
                "reason": "minimize_instrumentation"
            })
        );
    }
}
