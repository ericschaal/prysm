//! WLED as a blocking [`prysm_ports::LedOutput`].
//!
//! Device observations read WLED's JSON API. Starting Screen sync powers
//! the output and clears an explicit WLED override only when the request
//! authorizes takeover. The first DDP frame activates WLED's normal
//! realtime mode. An explicit override while streaming hands display back
//! to WLED.
//!
//! A held stream sends DDP (UDP port 4048) clockwise from the bottom-left
//! corner, with WLED's gamma and brightness. State reads while streaming
//! report display ownership, brightness and LED count. A cancelled or
//! released stream abandons its in-flight observer.
//!
//! Ordinary release applies an idle action only with evidence of control.
//! Explicit Off is a separate acknowledged command. A waiting monitor may
//! read WLED until it becomes reachable again or its owning request is cut.
mod address;
mod classify;
mod ddp;
mod discover;
mod dither;
mod http;
mod lifecycle;
mod observe;
mod output;
mod settings;

pub use output::{IDLE_LIMIT, OPEN_LIMIT, STOP_WAIT, Wled};
pub use settings::{LedCounts, MAX_LEDS, WledSettings};

#[cfg(test)]
mod tests;
