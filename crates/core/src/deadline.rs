//! `Instant + Duration` is a panic if the sum overflows the clock's internal
//! representation. All timeout/deadline call sites in this workspace use
//! this helper instead of the raw `+` so that an absurdly large timeout
//! degrades to a far-future deadline rather than crashing the process.

use tokio::time::{Duration, Instant};

/// A far enough future to serve as a deadline that is effectively "no timeout".
const FALLBACK_HORIZON: Duration = Duration::from_secs(365 * 24 * 3600);

/// `Instant::now() + timeout`, without the panic-on-overflow of `+`.
#[must_use]
pub fn saturating_deadline(timeout: Duration) -> Instant {
    let now = Instant::now();
    now.checked_add(timeout).or_else(|| now.checked_add(FALLBACK_HORIZON)).unwrap_or(now)
}
