//! CLI help-text coverage: flags must document their semantics.
use std::process::Command;

fn help_text(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    )
}

#[test]
fn analyze_help_documents_compare_semantics() {
    let help = help_text(&["analyze", "--help"]);
    assert!(
        help.contains("effects-on vs effects-off"),
        "compare flag must document its self-comparison semantics: {help}"
    );
}

#[test]
fn scene3d_form_add_help_lists_every_starter_preset() {
    let help = help_text(&["scene3d", "doc.pen", "form", "add", "--help"]);
    for preset in [
        "perfume-bottle",
        "cosmetic-jar",
        "cosmetic-tube",
        "cylinder-container",
        "shipping-carton",
        "product-card",
        "device-frame",
        "stand-up-pouch",
    ] {
        assert!(help.contains(preset), "missing preset {preset}: {help}");
    }
}

#[test]
fn scene3d_and_pack_help_name_their_subcommands() {
    let scene3d = help_text(&["scene3d", "--help"]);
    for word in ["uv", "graphic", "pack", "form"] {
        assert!(
            scene3d.contains(word),
            "scene3d help lacks {word}: {scene3d}"
        );
    }
    let pack = help_text(&["pack", "--help"]);
    for word in ["templates", "preflight", "export-bundle", "verify-bundle"] {
        assert!(pack.contains(word), "pack help lacks {word}: {pack}");
    }
}
