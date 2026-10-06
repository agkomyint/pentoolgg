//! Regression coverage for the v0.7.1 retest report.
use ::image::GenericImageView;
use serde_json::Value;
use std::{fs, path::PathBuf, process::Command};

struct Workspace(PathBuf);

impl Workspace {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "pentool-v072-{label}-{}-{}",
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

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(args)
        .output()
        .unwrap()
}

fn ok(args: &[&str]) -> Value {
    let out = run(args);
    assert!(
        out.status.success(),
        "{args:?}: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
}

fn tiny_png() -> Vec<u8> {
    let image = ::image::RgbaImage::from_pixel(8, 8, ::image::Rgba([200, 30, 30, 255]));
    let mut out = std::io::Cursor::new(Vec::new());
    ::image::DynamicImage::ImageRgba8(image)
        .write_to(&mut out, ::image::ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

fn white_pixels(path: &str) -> usize {
    ::image::open(path)
        .unwrap()
        .to_rgba8()
        .pixels()
        .filter(|p| p.0[0] > 200 && p.0[1] > 200 && p.0[2] > 200)
        .count()
}

fn text_doc(ws: &Workspace, name: &str, content: &str, weight: &str, with_image: bool) -> String {
    let doc = ws.path(name);
    ok(&["new", &doc, "--width", "480", "--height", "100"]);
    ok(&["canvas", &doc, "--background", "#222222"]);
    ok(&[
        "text",
        &doc,
        "put",
        "t1",
        "--layer",
        "layer-1",
        "--content",
        content,
        "--x",
        "10",
        "--y",
        "60",
        "--size",
        "36",
        "--weight",
        weight,
        "--fill",
        "#ffffff",
    ]);
    if with_image {
        let png = ws.path("tiny.png");
        fs::write(&png, tiny_png()).unwrap();
        ok(&[
            "image", "add", &doc, "pic", "--file", &png, "--layer", "layer-1", "--x", "440", "--y",
            "10", "--width", "30", "--height", "30", "--embed",
        ]);
    }
    doc
}

#[test]
fn weight_survives_image_scene_export() {
    let ws = Workspace::new("weight");
    let mut counts = Vec::new();
    for (weight, with_image) in [("400", true), ("800", true), ("800", false)] {
        let doc = text_doc(
            &ws,
            &format!("w{weight}{with_image}.pen"),
            "HELLO",
            weight,
            with_image,
        );
        let png = ws.path("w.png");
        ok(&["export", &doc, &png]);
        counts.push(white_pixels(&png));
    }
    assert!(counts[1] > counts[0] * 6 / 5, "800 not bolder: {counts:?}");
    assert!(
        counts[1].abs_diff(counts[2]) < 120,
        "image changed weight: {counts:?}"
    );
}

#[test]
fn missing_glyph_does_not_change_surrounding_font() {
    let ws = Workspace::new("glyph");
    let mut crops = Vec::new();
    for (index, content) in ["same file to bytes", "same file \u{2192} bytes"]
        .into_iter()
        .enumerate()
    {
        let doc = text_doc(&ws, &format!("g{index}.pen"), content, "400", false);
        let png = ws.path(&format!("g{index}.png"));
        ok(&["export", &doc, &png]);
        crops.push(::image::open(&png).unwrap().view(0, 0, 100, 100).to_image());
    }
    let differing = crops[0]
        .pixels()
        .zip(crops[1].pixels())
        .filter(|(a, b)| a != b)
        .count();
    assert_eq!(differing, 0, "text before the arrow changed font");
}

#[test]
fn image_scene_pdf_keeps_a_text_layer() {
    let ws = Workspace::new("pdf");
    let doc = text_doc(&ws, "p.pen", "Searchable words", "400", true);
    let pdf = ws.path("p.pdf");
    ok(&["export", &doc, &pdf]);
    let bytes = fs::read(&pdf).unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("3 Tr"), "no invisible text layer");
    assert!(text.contains("(Searchable words) Tj"));
    assert!(text.contains("/BaseFont /Helvetica"));
}
