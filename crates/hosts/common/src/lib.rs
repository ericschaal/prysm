//! Embedded portal bundles and session stop bounds for the desktop and TV hosts.
//! The `desktop` feature adds the shared desktop entry point and adapter list.
#[cfg(feature = "desktop")]
mod args;
pub mod assets;
#[cfg(feature = "desktop")]
mod desktop;
#[cfg(feature = "desktop")]
pub use desktop::{Host, INTERRUPTED, Program, desktop_adapters, main};
#[cfg(feature = "desktop")]
pub use prysm_runtime::STOP_DEADLINE;

use prysm_output_wled::{IDLE_LIMIT, STOP_WAIT};
use prysm_ports::SourceStop;
use prysm_runtime::TICK;
use std::time::Duration;

/// Whether every stop of a session with `source` and a WLED output ends
/// within `deadline`, whatever it meets. An open or an idle of the output in
/// flight ends at once when the stop comes, so a stop waits for the pull in
/// flight (its timeout, [`TICK`], then the rest of a frame or of an end, as
/// `source` says), the source's close and the stop's idle
/// ([`IDLE_LIMIT`]); or, after the session ended on its own and while it
/// idles `Off`, two idles ([`STOP_WAIT`]). Every host with a WLED output
/// asserts it for each source it registers.
#[must_use]
pub const fn stops_in_time(source: SourceStop, deadline: Duration) -> bool {
    let pull = if source.frame.as_millis() > source.end.as_millis() {
        source.frame
    } else {
        source.end
    };
    let closing = TICK
        .saturating_add(pull)
        .saturating_add(source.close)
        .saturating_add(IDLE_LIMIT);
    closing.as_millis() < deadline.as_millis() && STOP_WAIT.as_millis() < deadline.as_millis()
}
