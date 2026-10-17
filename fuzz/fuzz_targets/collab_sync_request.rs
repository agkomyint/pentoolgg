#![no_main]
use libfuzzer_sys::fuzz_target;

// The sync `ops` request decoder must bound and reject arbitrary bodies
// without panicking, and everything it admits must reach the op parser safely.
fuzz_target!(|data: &[u8]| {
    pentool::collab::evidence::fuzz_sync_request(data);
});
