use std::{fs, process::Command};

struct Workspace(std::path::PathBuf);

impl Workspace {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "pentool-psd-export-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn export_writes_a_flattened_psd_that_the_importer_can_read() {
    let workspace = Workspace::new();
    let output = workspace.0.join("page.psd");
    let result = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "export",
            "docs/fixtures/v4-scene.pen",
            output.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "PSD export failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let bytes = fs::read(output).unwrap();
    assert_eq!(&bytes[..4], b"8BPS");
    let imported = pentool::psd_import::import(&bytes, "page.psd").unwrap();
    pentool::scene::validate(&imported.document).unwrap();
    assert!(imported
        .document
        .get("image_assets")
        .and_then(serde_json::Value::as_object)
        .is_some_and(|assets| !assets.is_empty()));
    assert!(imported.report.counts.approximated > 0);
}

#[test]
fn export_layers_writes_named_raster_layers_that_the_importer_can_read() {
    let workspace = Workspace::new();
    let output = workspace.0.join("page-layered.psd");
    let result = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "export",
            "docs/fixtures/v4-scene.pen",
            output.to_str().unwrap(),
            "--layers",
        ])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "layered PSD export failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let bytes = fs::read(output).unwrap();
    let imported = pentool::psd_import::import(&bytes, "page-layered.psd").unwrap();
    pentool::scene::validate(&imported.document).unwrap();
    assert!(
        serde_json::to_string(&imported.document)
            .unwrap()
            .contains("Content"),
        "the PSD layer name should survive the import round trip"
    );
}
