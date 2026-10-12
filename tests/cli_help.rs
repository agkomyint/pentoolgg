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
