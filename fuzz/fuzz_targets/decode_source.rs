#![no_main]
use libfuzzer_sys::fuzz_target;

// PNG/JPEG/WebP decoding must never panic or allocate beyond the documented limits.
fuzz_target!(|data: &[u8]| {
    let _ = pentool::image::decode_source(data);
});
