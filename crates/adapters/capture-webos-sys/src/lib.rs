//! LG webOS VTCapture FFI (the Luna permission check lives in
//! `prysm-webos-luna-sys`), ported from hyperion-webos (see `LICENSE.hyperion`;
//! layouts from tv-native-apis, see `LICENSE.tv-native-apis`), and the HAL GAL
//! graphics-plane (TV UI) capture, whose surface layout matches
//! hyperion-webos's `halgal.h` (`LICENSE.hyperion`).
//!
//! The two safe types each own one native capture tap: [`VtCapture`] (video,
//! optionally blended with menus) and [`UiCapture`] (the UI plane alone, for
//! when there is no video). Neither is `Send`, so each tap is opened, used and
//! released on one thread (HAL GAL binds an EGL context to it). Each holds a
//! `PhantomData<*const ()>` for that, so it stays `!Send` whatever its other
//! fields become (its pointer fields make it `!Send` too). These guard the
//! property against a regression; they cannot tell which field provides it:
//!
//! ```compile_fail
//! fn send<T: Send>() {}
//! send::<prysm_capture_webos_sys::VtCapture>();
//! ```
//! ```compile_fail
//! fn send<T: Send>() {}
//! send::<prysm_capture_webos_sys::UiCapture>();
//! ```
//!
//! The libraries are loaded once per process (with
//! `prysm_webos_luna_sys::load_once`), on the thread of the first tap; later
//! taps may be opened on other threads. Each call is a single native
//! operation; watchdogs, retries, pacing, tap selection and pixel conversion
//! belong to the caller (`prysm-capture-webos`). After a failed driver call a
//! [`VtCapture`] refuses every call but release with [`Error::Failed`]. C++ exceptions from
//! VTCapture are caught in `native/vtcapture.cpp` and never cross into Rust.
//! Vendor memory (driver planes, the `/dev/gfx` mapping) may be written while
//! it is read, so it is read only through pointers, never as a Rust slice.
//! `libvtcapture` and `libhalgal` stay loaded for the life of the process.
//!
//! Requires root and the TV's `libvtcapture.so`, `libluna-service2.so`,
//! `libhalgal.so.1` and `/dev/gfx`. On any other system both `open`s fail
//! with [`Error::Load`].
mod ffi;
mod halgal;
mod operation;
mod vtcapture;

#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub use halgal::fuzz_surface_check;
pub use halgal::{UiCapture, UiFrame};
pub use operation::Operation;
pub use vtcapture::{Acquired, Error, Options, VideoFrame, VtCapture};
