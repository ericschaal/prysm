//! The receiver's text messages as the sender reads them (capture-remote
//! `wire::decode_from_receiver`).
#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        prysm_capture_remote::fuzzing::from_receiver(text);
    }
});
