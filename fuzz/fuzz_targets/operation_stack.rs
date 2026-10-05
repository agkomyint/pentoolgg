#![no_main]
use libfuzzer_sys::fuzz_target;

// Hostile operation stacks must validate or fail cleanly.
fuzz_target!(|data: &[u8]| {
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(data) {
        if let Some(stack) = value.as_array() {
            let _ = pentool::imageops::validate_stack(stack, 64, 64);
        }
    }
});
