#![allow(clippy::expect_used)]

use void_crawl_core::{
    BrowserBudgetScope, BrowserByteAccounting, BrowserByteAccountingError, BrowserByteBudget,
    BrowserByteCount, BrowserByteDomain, BrowserByteLimit, BrowserByteLimitError,
    BrowserByteReport, BrowserByteReportError, BrowserByteSpec, BrowserLimitScope,
    BrowserPayloadExtent, BrowserPayloadFailureReason, MeasuredBrowserBytes,
};

fn spec(limit: u64, budget_scope: BrowserBudgetScope) -> BrowserByteSpec {
    BrowserByteSpec::new(
        BrowserByteDomain::CdpDecodedBody,
        BrowserByteLimit::try_from(limit).expect("positive limit"),
        BrowserLimitScope::RetentionAfterProviderMaterialization,
        budget_scope,
    )
}

#[test]
fn byte_limits_are_nonzero_and_round_trip() {
    assert_eq!(BrowserByteLimit::try_from(0_u64), Err(BrowserByteLimitError::Zero));
    let limit = BrowserByteLimit::try_from(17_u64).expect("valid limit");
    assert_eq!(limit.get(), 17);
    assert_eq!(limit.as_usize(), Ok(17));
    assert_eq!(serde_json::to_string(&limit).expect("serialize limit"), "17");
    assert!(serde_json::from_str::<BrowserByteLimit>("0").is_err());
}

#[test]
fn exact_limit_retains_the_complete_chunk_without_overflow() {
    let mut budget = BrowserByteBudget::new(spec(4, BrowserBudgetScope::PerPayload));
    let admission = budget.observe_chunk(BrowserByteCount::new(4)).expect("admit exact chunk");
    assert_eq!(admission.retain_prefix.get(), 4);
    assert_eq!(admission.discarded.get(), 0);
    assert!(!admission.limit_exceeded);
    let accounting = budget.accounting().expect("valid accounting");
    assert_eq!(accounting.observed().get(), 4);
    assert_eq!(accounting.retained().get(), 4);
    assert_eq!(
        accounting.discarded(),
        MeasuredBrowserBytes::Known { value: BrowserByteCount::new(0) }
    );
}

#[test]
fn one_byte_over_retains_an_exact_prefix() {
    let mut budget = BrowserByteBudget::new(spec(4, BrowserBudgetScope::PerPayload));
    let admission = budget.observe_chunk(BrowserByteCount::new(5)).expect("admit over-limit chunk");
    assert_eq!(admission.retain_prefix.get(), 4);
    assert_eq!(admission.discarded.get(), 1);
    assert!(admission.limit_exceeded);
    let accounting = budget.accounting().expect("valid accounting");
    assert_eq!(accounting.observed().get(), 5);
    assert_eq!(accounting.retained().get(), 4);
}

#[test]
fn aggregate_budget_is_shared_across_chunks() {
    let mut budget = BrowserByteBudget::new(spec(5, BrowserBudgetScope::CaptureAggregate));
    let first = budget.observe_chunk(BrowserByteCount::new(3)).expect("first chunk");
    let second = budget.observe_chunk(BrowserByteCount::new(4)).expect("second chunk");
    assert_eq!(first.retain_prefix.get(), 3);
    assert_eq!(second.retain_prefix.get(), 2);
    assert_eq!(second.discarded.get(), 2);
    let accounting = budget.accounting().expect("valid accounting");
    assert_eq!(accounting.observed().get(), 7);
    assert_eq!(accounting.retained().get(), 5);
    assert_eq!(
        accounting.discarded(),
        MeasuredBrowserBytes::Known { value: BrowserByteCount::new(2) }
    );
}

#[test]
fn empty_payload_is_complete_accounting() {
    let budget = BrowserByteBudget::new(spec(1, BrowserBudgetScope::PerPayload));
    let accounting = budget.accounting().expect("empty accounting");
    assert_eq!(accounting.observed().get(), 0);
    assert_eq!(accounting.retained().get(), 0);
    assert_eq!(
        accounting.discarded(),
        MeasuredBrowserBytes::Known { value: BrowserByteCount::new(0) }
    );
}

#[test]
fn contradictory_accounting_is_rejected() {
    assert_eq!(
        BrowserByteAccounting::new(
            BrowserByteCount::new(1),
            BrowserByteCount::new(2),
            MeasuredBrowserBytes::Known { value: BrowserByteCount::new(0) },
        ),
        Err(BrowserByteAccountingError::RetainedExceedsObserved)
    );
    assert_eq!(
        BrowserByteAccounting::new(
            BrowserByteCount::new(5),
            BrowserByteCount::new(3),
            MeasuredBrowserBytes::Known { value: BrowserByteCount::new(1) },
        ),
        Err(BrowserByteAccountingError::KnownDiscardMismatch)
    );
    assert!(
        serde_json::from_str::<BrowserByteAccounting>(
            r#"{"observed":5,"retained":3,"discarded":{"status":"known","value":1}}"#,
        )
        .is_err(),
        "deserialization must enforce the same accounting invariants",
    );
}

#[test]
fn unknown_discard_is_preserved_without_inference() {
    let accounting = BrowserByteAccounting::new(
        BrowserByteCount::new(5),
        BrowserByteCount::new(3),
        MeasuredBrowserBytes::Unavailable {
            reason: void_crawl_core::BrowserByteMeasurementUnavailableReason::ProviderDidNotReport,
        },
    )
    .expect("unknown loss is valid");
    assert!(matches!(accounting.discarded(), MeasuredBrowserBytes::Unavailable { .. }));
}

#[test]
fn reports_derive_complete_and_truncated_extent_from_one_accounting_source() {
    let complete = BrowserByteReport::from_known_extent(
        BrowserByteDomain::RenderedDomUtf8,
        None,
        BrowserByteCount::new(4),
        BrowserByteCount::new(4),
    )
    .expect("complete report");
    assert_eq!(complete.extent(), BrowserPayloadExtent::Complete);

    let truncated = BrowserByteReport::from_known_extent(
        BrowserByteDomain::CdpDecodedBody,
        Some(spec(4, BrowserBudgetScope::PerPayload)),
        BrowserByteCount::new(5),
        BrowserByteCount::new(4),
    )
    .expect("truncated report");
    assert!(matches!(truncated.extent(), BrowserPayloadExtent::Truncated { .. }));
    assert_eq!(truncated.accounting().retained().get(), 4);
    assert_eq!(
        BrowserByteReport::from_known_extent(
            BrowserByteDomain::RenderedDomUtf8,
            Some(spec(4, BrowserBudgetScope::PerPayload)),
            BrowserByteCount::new(4),
            BrowserByteCount::new(4),
        ),
        Err(BrowserByteReportError::Accounting(BrowserByteAccountingError::SpecDomainMismatch)),
    );
}

#[test]
fn discarded_and_failed_reports_preserve_terminal_invariants_through_serde() {
    let discarded = BrowserByteReport::discarded(
        BrowserByteDomain::CdpDecodedBody,
        Some(spec(4, BrowserBudgetScope::PerPayload)),
        MeasuredBrowserBytes::Known { value: BrowserByteCount::new(7) },
    )
    .expect("discarded report");
    assert_eq!(discarded.accounting().retained().get(), 0);
    assert!(matches!(discarded.extent(), BrowserPayloadExtent::Discarded { .. }));
    let discarded_json = serde_json::to_string(&discarded).expect("serialize discarded");
    assert_eq!(
        serde_json::from_str::<BrowserByteReport>(&discarded_json).expect("deserialize discarded"),
        discarded,
    );

    let failed = BrowserByteReport::failed(
        BrowserByteDomain::CdpDecodedBody,
        Some(spec(4, BrowserBudgetScope::PerPayload)),
        BrowserPayloadFailureReason::ProviderDisconnected,
    )
    .expect("failed report");
    assert!(matches!(failed.extent(), BrowserPayloadExtent::Failed { .. }));
    let failed_json = serde_json::to_string(&failed).expect("serialize failed");
    assert_eq!(
        serde_json::from_str::<BrowserByteReport>(&failed_json).expect("deserialize failed"),
        failed,
    );
    assert!(serde_json::from_str::<BrowserByteReport>(
        r#"{"domain":"cdp_decoded_body","spec":null,"accounting":{"observed":7,"retained":1,"discarded":{"status":"known","value":6}},"extent":{"status":"discarded","observed_bytes":{"status":"known","value":6}},"additional_loss":{"status":"known","value":0}}"#
    )
    .is_err(), "discarded reports must not deserialize with retained bytes");
}

#[test]
fn core_has_no_yosoi_dependency() {
    let manifest = include_str!("../Cargo.toml");
    assert!(
        !manifest.to_ascii_lowercase().contains("yosoi"),
        "void_crawl_core must remain independent of Yosoi crates"
    );
}

#[test]
fn accounting_overflow_is_rejected() {
    assert_eq!(
        BrowserByteAccounting::new(
            BrowserByteCount::new(u64::MAX),
            BrowserByteCount::new(u64::MAX),
            MeasuredBrowserBytes::Known { value: BrowserByteCount::new(1) },
        ),
        Err(BrowserByteAccountingError::Overflow)
    );
}
