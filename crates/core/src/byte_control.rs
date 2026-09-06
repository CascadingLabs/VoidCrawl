//! Browser-native byte limits and accounting.
//!
//! These types describe bytes observed and retained by VoidCrawl in concrete
//! browser/CDP domains. They deliberately do not model durable artifacts or
//! import Yosoi capture types.

use std::num::NonZeroU64;

use serde::{Deserialize, Deserializer, Serialize, de::Error as _};
use thiserror::Error;

/// Error returned when a browser byte limit cannot be represented.
#[derive(Debug, Clone, Copy, Error, PartialEq, Eq)]
pub enum BrowserByteLimitError {
    #[error("browser byte limit must be greater than zero")]
    Zero,
    #[error("browser byte limit exceeds the supported integer representation")]
    TooLarge,
}

/// Validated non-zero byte limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BrowserByteLimit(NonZeroU64);

impl BrowserByteLimit {
    #[must_use]
    pub const fn one() -> Self {
        Self(NonZeroU64::MIN)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }

    /// Converts the limit to the platform size used by in-memory buffers.
    pub fn as_usize(self) -> Result<usize, BrowserByteLimitError> {
        usize::try_from(self.get()).map_err(|_| BrowserByteLimitError::TooLarge)
    }
}

impl TryFrom<u64> for BrowserByteLimit {
    type Error = BrowserByteLimitError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        NonZeroU64::new(value).map(Self).ok_or(BrowserByteLimitError::Zero)
    }
}

impl TryFrom<usize> for BrowserByteLimit {
    type Error = BrowserByteLimitError;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        u64::try_from(value).map_err(|_| BrowserByteLimitError::TooLarge)?.try_into()
    }
}

/// Exact byte count observed by VoidCrawl.
#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct BrowserByteCount(u64);

impl BrowserByteCount {
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub fn try_from_usize(value: usize) -> Result<Self, BrowserByteLimitError> {
        Ok(Self(u64::try_from(value).map_err(|_| BrowserByteLimitError::TooLarge)?))
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// A byte count or an explicit reason it cannot be known.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum MeasuredBrowserBytes {
    Known { value: BrowserByteCount },
    Unavailable { reason: BrowserByteMeasurementUnavailableReason },
}

/// Why exact byte loss or extent is unavailable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserByteMeasurementUnavailableReason {
    ProviderDidNotReport,
    CaptureEndedEarly,
    NotApplicable,
}

/// Concrete browser/CDP representation in which bytes are counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserByteDomain {
    CdpDecodedBody,
    RenderedDomUtf8,
    AccessibilityJsonUtf8,
    RuntimeDiagnosticUtf8,
    ScreenshotPng,
    RecordingFrame,
    EncodedRecording,
}

/// Where a configured limit can actually be enforced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserLimitScope {
    /// Bytes are admitted incrementally and processing stops at the bound.
    StreamingAdmission,
    /// The provider materializes the payload before VoidCrawl retains a prefix.
    RetentionAfterProviderMaterialization,
}

/// Whether a byte budget applies independently or across a capture scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserBudgetScope {
    PerPayload,
    CaptureAggregate,
}

/// One configured browser byte budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserByteSpec {
    domain:       BrowserByteDomain,
    limit:        BrowserByteLimit,
    limit_scope:  BrowserLimitScope,
    budget_scope: BrowserBudgetScope,
}

impl BrowserByteSpec {
    #[must_use]
    pub const fn new(
        domain: BrowserByteDomain,
        limit: BrowserByteLimit,
        limit_scope: BrowserLimitScope,
        budget_scope: BrowserBudgetScope,
    ) -> Self {
        Self { domain, limit, limit_scope, budget_scope }
    }

    #[must_use]
    pub const fn domain(self) -> BrowserByteDomain {
        self.domain
    }

    #[must_use]
    pub const fn limit(self) -> BrowserByteLimit {
        self.limit
    }

    #[must_use]
    pub const fn limit_scope(self) -> BrowserLimitScope {
        self.limit_scope
    }

    #[must_use]
    pub const fn budget_scope(self) -> BrowserBudgetScope {
        self.budget_scope
    }
}

/// Contradiction or overflow in browser byte accounting.
#[derive(Debug, Clone, Copy, Error, PartialEq, Eq)]
pub enum BrowserByteAccountingError {
    #[error("retained bytes cannot exceed observed bytes")]
    RetainedExceedsObserved,
    #[error("retained and known discarded bytes must equal observed bytes")]
    KnownDiscardMismatch,
    #[error("browser byte accounting overflowed")]
    Overflow,
    #[error("browser byte specification domain does not match the reported payload")]
    SpecDomainMismatch,
}

/// Bytes delivered to VoidCrawl, retained, and discarded after observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct BrowserByteAccounting {
    observed:  BrowserByteCount,
    retained:  BrowserByteCount,
    discarded: MeasuredBrowserBytes,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BrowserByteAccountingWire {
    observed:  BrowserByteCount,
    retained:  BrowserByteCount,
    discarded: MeasuredBrowserBytes,
}

impl TryFrom<BrowserByteAccountingWire> for BrowserByteAccounting {
    type Error = BrowserByteAccountingError;

    fn try_from(value: BrowserByteAccountingWire) -> Result<Self, Self::Error> {
        Self::new(value.observed, value.retained, value.discarded)
    }
}

impl<'de> Deserialize<'de> for BrowserByteAccounting {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        BrowserByteAccountingWire::deserialize(deserializer)?.try_into().map_err(D::Error::custom)
    }
}

impl BrowserByteAccounting {
    pub fn new(
        observed: BrowserByteCount,
        retained: BrowserByteCount,
        discarded: MeasuredBrowserBytes,
    ) -> Result<Self, BrowserByteAccountingError> {
        if retained > observed {
            return Err(BrowserByteAccountingError::RetainedExceedsObserved);
        }
        if let MeasuredBrowserBytes::Known { value: discarded } = discarded {
            let explained = retained
                .get()
                .checked_add(discarded.get())
                .ok_or(BrowserByteAccountingError::Overflow)?;
            if explained != observed.get() {
                return Err(BrowserByteAccountingError::KnownDiscardMismatch);
            }
        }
        Ok(Self { observed, retained, discarded })
    }

    #[must_use]
    pub const fn observed(self) -> BrowserByteCount {
        self.observed
    }

    #[must_use]
    pub const fn retained(self) -> BrowserByteCount {
        self.retained
    }

    #[must_use]
    pub const fn discarded(self) -> MeasuredBrowserBytes {
        self.discarded
    }
}

/// Failure to construct a canonical browser byte report.
#[derive(Debug, Clone, Copy, Error, PartialEq, Eq)]
pub enum BrowserByteReportError {
    #[error(transparent)]
    Limit(#[from] BrowserByteLimitError),
    #[error(transparent)]
    Accounting(#[from] BrowserByteAccountingError),
    #[error("browser byte report extent contradicts its accounting")]
    ExtentMismatch,
}

/// Terminal extent of one browser payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum BrowserPayloadExtent {
    Complete,
    Truncated { complete_bytes: MeasuredBrowserBytes },
    Discarded { observed_bytes: MeasuredBrowserBytes },
    Unavailable { reason: BrowserPayloadUnavailableReason },
    Failed { reason: BrowserPayloadFailureReason },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserPayloadUnavailableReason {
    ProviderDidNotReport,
    NotCollected,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserPayloadFailureReason {
    ProviderRejected,
    ProviderDisconnected,
    InvalidEncoding,
    Deadline,
    Cancelled,
    SinkFailure,
}

/// Canonical byte facts for one browser payload or aggregate collector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct BrowserByteReport {
    domain:          BrowserByteDomain,
    spec:            Option<BrowserByteSpec>,
    accounting:      BrowserByteAccounting,
    extent:          BrowserPayloadExtent,
    additional_loss: MeasuredBrowserBytes,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BrowserByteReportWire {
    domain:          BrowserByteDomain,
    spec:            Option<BrowserByteSpec>,
    accounting:      BrowserByteAccounting,
    extent:          BrowserPayloadExtent,
    additional_loss: MeasuredBrowserBytes,
}

impl<'de> Deserialize<'de> for BrowserByteReport {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = BrowserByteReportWire::deserialize(deserializer)?;
        Self::new(wire.domain, wire.spec, wire.accounting, wire.extent, wire.additional_loss)
            .map_err(D::Error::custom)
    }
}

impl BrowserByteReport {
    fn new(
        domain: BrowserByteDomain,
        spec: Option<BrowserByteSpec>,
        accounting: BrowserByteAccounting,
        extent: BrowserPayloadExtent,
        additional_loss: MeasuredBrowserBytes,
    ) -> Result<Self, BrowserByteReportError> {
        if spec.is_some_and(|spec| spec.domain() != domain) {
            return Err(BrowserByteAccountingError::SpecDomainMismatch.into());
        }
        let known_discarded = matches!(
            accounting.discarded(),
            MeasuredBrowserBytes::Known { value } if value.get() > 0
        );
        match extent {
            BrowserPayloadExtent::Complete if known_discarded => {
                return Err(BrowserByteReportError::ExtentMismatch);
            }
            BrowserPayloadExtent::Truncated { complete_bytes }
                if !known_discarded
                    || complete_bytes
                        != MeasuredBrowserBytes::Known { value: accounting.observed() } =>
            {
                return Err(BrowserByteReportError::ExtentMismatch);
            }
            BrowserPayloadExtent::Discarded { observed_bytes }
                if accounting.retained().get() != 0 || observed_bytes != accounting.discarded() =>
            {
                return Err(BrowserByteReportError::ExtentMismatch);
            }
            BrowserPayloadExtent::Unavailable { .. } | BrowserPayloadExtent::Failed { .. }
                if accounting.observed().get() != 0 || accounting.retained().get() != 0 =>
            {
                return Err(BrowserByteReportError::ExtentMismatch);
            }
            _ => {}
        }
        Ok(Self { domain, spec, accounting, extent, additional_loss })
    }
    pub fn from_known_extent(
        domain: BrowserByteDomain,
        spec: Option<BrowserByteSpec>,
        observed: BrowserByteCount,
        retained: BrowserByteCount,
    ) -> Result<Self, BrowserByteReportError> {
        if spec.is_some_and(|spec| spec.domain() != domain) {
            return Err(BrowserByteAccountingError::SpecDomainMismatch.into());
        }
        let discarded = observed
            .get()
            .checked_sub(retained.get())
            .ok_or(BrowserByteAccountingError::RetainedExceedsObserved)?;
        let accounting = BrowserByteAccounting::new(
            observed,
            retained,
            MeasuredBrowserBytes::Known { value: BrowserByteCount::new(discarded) },
        )?;
        let extent = if discarded == 0 {
            BrowserPayloadExtent::Complete
        } else {
            BrowserPayloadExtent::Truncated {
                complete_bytes: MeasuredBrowserBytes::Known { value: observed },
            }
        };
        Self::new(
            domain,
            spec,
            accounting,
            extent,
            MeasuredBrowserBytes::Known { value: BrowserByteCount::new(0) },
        )
    }

    /// Reports a payload deliberately discarded without retaining its bytes.
    pub fn discarded(
        domain: BrowserByteDomain,
        spec: Option<BrowserByteSpec>,
        observed_bytes: MeasuredBrowserBytes,
    ) -> Result<Self, BrowserByteReportError> {
        let observed = match observed_bytes {
            MeasuredBrowserBytes::Known { value } => value,
            MeasuredBrowserBytes::Unavailable { .. } => BrowserByteCount::new(0),
        };
        let accounting =
            BrowserByteAccounting::new(observed, BrowserByteCount::new(0), observed_bytes)?;
        Self::new(
            domain,
            spec,
            accounting,
            BrowserPayloadExtent::Discarded { observed_bytes },
            MeasuredBrowserBytes::Known { value: BrowserByteCount::new(0) },
        )
    }

    /// Reports a failed payload without presenting it as a complete capture.
    pub fn failed(
        domain: BrowserByteDomain,
        spec: Option<BrowserByteSpec>,
        reason: BrowserPayloadFailureReason,
    ) -> Result<Self, BrowserByteReportError> {
        let zero = BrowserByteCount::new(0);
        let unavailable = MeasuredBrowserBytes::Unavailable {
            reason: BrowserByteMeasurementUnavailableReason::CaptureEndedEarly,
        };
        let accounting = BrowserByteAccounting::new(zero, zero, unavailable)?;
        Self::new(domain, spec, accounting, BrowserPayloadExtent::Failed { reason }, unavailable)
    }

    pub fn unavailable(domain: BrowserByteDomain, reason: BrowserPayloadUnavailableReason) -> Self {
        let zero = BrowserByteCount::new(0);
        Self {
            domain,
            spec: None,
            accounting: BrowserByteAccounting {
                observed:  zero,
                retained:  zero,
                discarded: MeasuredBrowserBytes::Unavailable {
                    reason: BrowserByteMeasurementUnavailableReason::NotApplicable,
                },
            },
            extent: BrowserPayloadExtent::Unavailable { reason },
            additional_loss: MeasuredBrowserBytes::Unavailable {
                reason: BrowserByteMeasurementUnavailableReason::ProviderDidNotReport,
            },
        }
    }

    #[must_use]
    pub const fn domain(self) -> BrowserByteDomain {
        self.domain
    }
    #[must_use]
    pub const fn spec(self) -> Option<BrowserByteSpec> {
        self.spec
    }
    #[must_use]
    pub const fn accounting(self) -> BrowserByteAccounting {
        self.accounting
    }
    #[must_use]
    pub const fn extent(self) -> BrowserPayloadExtent {
        self.extent
    }
    #[must_use]
    pub const fn additional_loss(self) -> MeasuredBrowserBytes {
        self.additional_loss
    }
}

/// Admission decision for one observed byte chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BrowserByteAdmission {
    pub observed:       BrowserByteCount,
    pub retain_prefix:  BrowserByteCount,
    pub discarded:      BrowserByteCount,
    pub limit_exceeded: bool,
}

/// Checked incremental budget used by streaming collectors and bounded sinks.
#[derive(Debug, Clone, Copy)]
pub struct BrowserByteBudget {
    spec:      BrowserByteSpec,
    observed:  u64,
    retained:  u64,
    discarded: u64,
}

impl BrowserByteBudget {
    #[must_use]
    pub const fn new(spec: BrowserByteSpec) -> Self {
        Self { spec, observed: 0, retained: 0, discarded: 0 }
    }

    pub fn observe_chunk(
        &mut self,
        bytes: BrowserByteCount,
    ) -> Result<BrowserByteAdmission, BrowserByteAccountingError> {
        self.observed =
            self.observed.checked_add(bytes.get()).ok_or(BrowserByteAccountingError::Overflow)?;
        let remaining = self.spec.limit().get().saturating_sub(self.retained);
        let retain_prefix = remaining.min(bytes.get());
        let discarded =
            bytes.get().checked_sub(retain_prefix).ok_or(BrowserByteAccountingError::Overflow)?;
        self.retained =
            self.retained.checked_add(retain_prefix).ok_or(BrowserByteAccountingError::Overflow)?;
        self.discarded =
            self.discarded.checked_add(discarded).ok_or(BrowserByteAccountingError::Overflow)?;
        Ok(BrowserByteAdmission {
            observed:       bytes,
            retain_prefix:  BrowserByteCount::new(retain_prefix),
            discarded:      BrowserByteCount::new(discarded),
            limit_exceeded: discarded != 0,
        })
    }

    pub fn accounting(self) -> Result<BrowserByteAccounting, BrowserByteAccountingError> {
        BrowserByteAccounting::new(
            BrowserByteCount::new(self.observed),
            BrowserByteCount::new(self.retained),
            MeasuredBrowserBytes::Known { value: BrowserByteCount::new(self.discarded) },
        )
    }

    #[must_use]
    pub const fn spec(self) -> BrowserByteSpec {
        self.spec
    }
}
