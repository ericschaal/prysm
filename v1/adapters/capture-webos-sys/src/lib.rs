//! LG webOS VTCapture FFI (the Luna permission check lives in
//! `prysm-webos-luna-sys`), ported from hyperion-webos (see `LICENSE.hyperion`;
//! layouts from tv-native-apis, see `LICENSE.tv-native-apis`).
//!
//! The one safe type, [`VtCapture`], owns one native capture tap. It is not
//! `Send`: LG's library constructors require that loading, capture and release
//! all happen on the thread that opened it. Each call is a single native
//! operation; watchdogs, retries, pacing and pixel conversion belong to the
//! caller (`prysm-capture-webos`). C++ exceptions are caught in
//! `native/vtcapture.cpp` and never cross into Rust.
//!
//! Requires root and the TV's `libvtcapture.so` and `libluna-service2.so`.
//! On any other system [`VtCapture::open`] fails with [`Error::Load`].
mod ffi;
mod vtcapture;

pub use vtcapture::{Acquired, Error, Options, VtCapture};
