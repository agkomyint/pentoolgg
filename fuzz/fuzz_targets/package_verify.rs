#![no_main]
use libfuzzer_sys::fuzz_target;

// Hostile archives (including image tables) must fail verification cleanly.
fuzz_target!(|data: &[u8]| {
    let path = std::env::temp_dir().join(format!("pentool-fuzz-{}.penpkg", std::process::id()));
    if std::fs::write(&path, data).is_ok() {
        let _ = pentool::package::verify(&path);
        let _ = std::fs::remove_file(&path);
    }
});
