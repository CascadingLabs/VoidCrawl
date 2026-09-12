//! Deterministic synchronous fixture work only; this does not profile Chromium.
#![allow(
    clippy::disallowed_macros,
    reason = "the Gungraun harness macro emits command-line diagnostics"
)]

extern crate gungraun;

use std::hint::black_box;

use gungraun::prelude::*;

#[library_benchmark]
fn fixture_token_count() -> usize {
    black_box(voidcrawl_benchmarks::fixture_dom_token_count())
}

library_benchmark_group!(
    name = deterministic_fixture_work;
    benchmarks = fixture_token_count
);

main!(library_benchmark_groups = deterministic_fixture_work);
