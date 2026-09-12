//! Allocation evidence for a synchronous fixture-only helper.
//!
//! This intentionally does not claim allocations made by Tokio workers or
//! Chromium child processes.

use divan::{AllocProfiler, Bencher, black_box};

#[global_allocator]
static ALLOCATOR: AllocProfiler = AllocProfiler::system();

fn main() {
    divan::main();
}

#[divan::bench]
fn fixture_token_count(bencher: Bencher<'_, '_>) {
    bencher.bench_local(|| black_box(voidcrawl_benchmarks::fixture_dom_token_count()));
}

#[divan::bench]
fn fixture_payload_copy(bencher: Bencher<'_, '_>) {
    bencher.bench_local(|| black_box(voidcrawl_benchmarks::FIXTURE.as_bytes()).to_vec());
}
