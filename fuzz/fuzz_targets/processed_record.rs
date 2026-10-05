#![no_main]
use libfuzzer_sys::fuzz_target;

// Cache records are untrusted bytes: reject without panic or oversized allocation.
fuzz_target!(|data: &[u8]| {
    let _ = pentool::image::parse_processed_record(data);
});
