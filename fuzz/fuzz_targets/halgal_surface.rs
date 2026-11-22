//! HAL GAL's surface check (capture-webos-sys `UiCapture::check`); the
//! invariants are asserted in `fuzz_surface_check`.
#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| prysm_capture_webos_sys::fuzz_surface_check(data));
