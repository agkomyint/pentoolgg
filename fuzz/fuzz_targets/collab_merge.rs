#![no_main]
use libfuzzer_sys::fuzz_target;

// Replay of an arbitrary operation set must not panic and must converge to
// identical bytes under any delivery order or duplication.
fuzz_target!(|data: &[u8]| {
    pentool::collab::evidence::fuzz_replay(data);
});
