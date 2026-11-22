//! The sender's offer as the receiver reads it (capture-remote `Offer::parse`),
//! and a request checked against it.
#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| prysm_capture_remote::fuzzing::offer(data));
