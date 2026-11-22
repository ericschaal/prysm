//! The capture stream's frame messages (capture-remote `wire::decode_frame`);
//! the invariants are asserted in `prysm_capture_remote::fuzzing::frame`.
#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| prysm_capture_remote::fuzzing::frame(data));
