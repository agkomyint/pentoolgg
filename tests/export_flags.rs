//! Export flag validation at the CLI level.
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

struct Workspace(PathBuf);
impl Workspace {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "pentool-export-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn export_rejects_page_combined_with_all_pages() {
    let ws = Workspace::new("flag-conflict");
    let doc = ws.path("multi.pen");
    fs::copy("examples/multipage-ai-chat.pen", &doc).unwrap();
    let out = ws.path("out");
    let result = run(&[
        "export",
        &doc,
        &out,
        "--page",
        "mobile",
        "--all-pages",
        "--format",
        "png",
    ]);
    assert!(
        !result.status.success(),
        "--page with --all-pages should fail"
    );
    let message = format!(
        "{}{}",
        String::from_utf8_lossy(&result.stderr),
        String::from_utf8_lossy(&result.stdout)
    );
    assert!(
        message.contains("--page cannot be combined with --all-pages"),
        "unexpected error: {message}"
    );
}
