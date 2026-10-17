#![no_main]
use libfuzzer_sys::fuzz_target;

// Arbitrary bytes must be accepted or rejected by the operation parser and
// validator without panicking; an accepted operation re-parses to the same id.
fuzz_target!(|data: &[u8]| {
    pentool::collab::evidence::fuzz_op(data);
});
