//! The sender's text messages as the receiver reads them (capture-remote
//! `wire::decode_from_sender`), and the refusal reason's cut.
#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        prysm_capture_remote::fuzzing::from_sender(text);
    }
});
