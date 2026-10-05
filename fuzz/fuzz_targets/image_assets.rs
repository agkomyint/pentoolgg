#![no_main]
use libfuzzer_sys::fuzz_target;

// Asset tables, masks, and node geometry from arbitrary JSON must fail cleanly.
fuzz_target!(|data: &[u8]| {
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(data) {
        let _ = pentool::image::validate_assets(&value);
        if let Some(pages) = value["pages"].as_array() {
            for page in pages {
                let _ = pentool::image::validate_masks(page);
            }
        }
        let _ = pentool::scene::validate(&value);
    }
});
