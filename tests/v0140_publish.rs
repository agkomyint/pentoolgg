//! v0.14.0 tasks 15-16: composed layout preflight and publishing exports.
use serde_json::{json, Value};
use std::path::Path;

fn document(frame_height: f64) -> Value {
    json!({
        "color_management": {"aliases": {}, "engine": 1, "legacy_rgb": "srgb",
                             "profiles": {}, "working_space": "srgb:linear"},
        "components": [],
        "compositing": {"color_space": "srgb8", "engine": 1},
        "fonts": [],
        "format": "pentool",
        "image_assets": {},
        "layout": {
            "engine": 1,
            "spreads": [{"facing": false, "id": "sp1", "pages": ["page-1"]}],
            "hyperlinks": [{"id": "l1", "url": "https://example.com"}],
            "stories": [{"id": "s1", "paragraphs": [
                {"style": "heading-1", "runs": [{"text": "Chapter One"}]},
                {"runs": [
                    {"text": "Rivers carry "},
                    {"link": "l1", "runs": [{"text": "more than water"}]},
                    {"text": " and the history of a place along many banks and across generations of travellers."}
                ]}
            ]}]
        },
        "name": "Test",
        "pages": [{
            "canvas": {"background": "#ffffff", "height": 792, "width": 612},
            "id": "page-1",
            "layers": [{"id": "layer-1", "locked": false, "name": "Layer 1", "visible": true,
                "nodes": [{"columns": 1, "gutter": 12, "height": frame_height, "id": "f1",
                           "inset": [0, 0, 0, 0], "kind": "text-frame", "next": null,
                           "story": "s1", "width": 200.0, "x": 50.0, "y": 60.0}]}],
            "name": "Page 1"
        }],
        "styles": {},
        "version": 9
    })
}

fn scratch() -> std::path::PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "pentool-publish-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(dir: &Path, name: &str, doc: &Value) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, serde_json::to_vec_pretty(doc).unwrap()).unwrap();
    path
}

#[test]
fn preflight_measures_overset_with_the_composer() {
    let dir = scratch();
    let ok = write(dir.as_path(), "ok.pen", &document(400.0));
    let report = pentool::layout_preflight::run(
        &document(400.0),
        &ok,
        &pentool::layout_preflight::Options::default(),
    )
    .unwrap();
    assert_eq!(report["ok"], true, "{report}");
    let tight = document(20.0);
    let path = write(dir.as_path(), "tight.pen", &tight);
    let report = pentool::layout_preflight::run(
        &tight,
        &path,
        &pentool::layout_preflight::Options::default(),
    )
    .unwrap();
    assert_eq!(report["ok"], false);
    let issue = &report["issues"][0];
    assert_eq!(issue["code"], "text-overset");
    assert_eq!(issue["story"], "s1");
    assert_eq!(issue["frame"], "f1");
    assert!(issue["offset"].as_u64().unwrap() > 0);
}

#[test]
fn pdf_has_real_text_links_and_outline_and_epub_has_navigation() {
    let dir = scratch();
    let doc = document(400.0);
    let path = write(dir.as_path(), "doc.pen", &doc);
    let pdf = dir.as_path().join("out.pdf");
    let report = pentool::publish::export_pdf(
        &doc,
        &path,
        &pdf,
        &pentool::publish::PdfOptions {
            scale: 1.0,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(report["tagged"], true);
    assert_eq!(report["bookmarks"], 1);
    assert_eq!(report["links"], 1);
    let bytes = std::fs::read(&pdf).unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("/StructTreeRoot") && text.contains("/Outlines"));
    assert!(text.contains("/URI (https://example.com)"));
    assert!(text.contains("/ToUnicode"));

    let epub = dir.as_path().join("out.epub");
    let report = pentool::publish::export_epub(
        &[(&doc, path.as_path())],
        &epub,
        &pentool::publish::EpubOptions::default(),
    )
    .unwrap();
    assert_eq!(report["layout"], "reflowable");
    let mut archive = zip::ZipArchive::new(std::fs::File::open(&epub).unwrap()).unwrap();
    assert_eq!(archive.by_index(0).unwrap().name(), "mimetype");
    let mut story = String::new();
    std::io::Read::read_to_string(
        &mut archive.by_name("OEBPS/story-0-s1.xhtml").unwrap(),
        &mut story,
    )
    .unwrap();
    assert!(story.contains("<h1 id=\"p0\"") && story.contains("href=\"https://example.com\""));
}

#[test]
fn fixed_layout_epub_report_carries_fixed_limits() {
    let dir = scratch();
    let doc = document(400.0);
    let path = write(dir.as_path(), "doc.pen", &doc);
    let epub = dir.as_path().join("fixed.epub");
    let report = pentool::publish::export_epub(
        &[(&doc, path.as_path())],
        &epub,
        &pentool::publish::EpubOptions {
            fixed: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(report["layout"], "fixed");
    let limits: Vec<&str> = report["limits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_str().unwrap())
        .collect();
    assert!(
        limits.iter().all(|l| !l.contains("reflowable")),
        "fixed report must not carry reflowable limits: {limits:?}"
    );
    assert!(
        limits.iter().any(|l| l.contains("fixed output")),
        "limits must describe the fixed layout: {limits:?}"
    );
    // Fixed pages are rasterized artwork plus positioned text; links are gone.
    let mut archive = zip::ZipArchive::new(std::fs::File::open(&epub).unwrap()).unwrap();
    assert!(archive.by_name("OEBPS/images/d0-p0.png").is_ok());
    let mut page = String::new();
    std::io::Read::read_to_string(
        &mut archive.by_name("OEBPS/page-0-0.xhtml").unwrap(),
        &mut page,
    )
    .unwrap();
    assert!(!page.contains("<a "), "fixed pages drop link runs: {page}");
}
