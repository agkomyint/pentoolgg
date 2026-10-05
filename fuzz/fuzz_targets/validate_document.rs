#![no_main]
use libfuzzer_sys::fuzz_target;

// Arbitrary JSON must be accepted or rejected by the scene validator without panicking.
fuzz_target!(|data: &[u8]| {
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(data) {
        let _ = pentool::scene::validate(&value);
        let _ = pentool::scene::migrate_to_v5(value);
    }
});
