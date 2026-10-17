#![no_main]
use libfuzzer_sys::fuzz_target;

// The log reader, chain verifier and recovery must not panic on arbitrary
// bytes or on a valid chain tampered at input-chosen positions.
fuzz_target!(|data: &[u8]| {
    pentool::collab::evidence::fuzz_log(data);
});
