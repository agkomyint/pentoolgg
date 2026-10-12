//! v0.14.0 task 4: shaping and font engine.
use pentool::shaping::{self, Kerning, ShapeParams};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::Command;

fn temp(name: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir =
        std::env::temp_dir().join(format!("pentool-shaping-{name}-{n}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run(args: &[&str]) -> (bool, Value, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(args)
        .output()
        .unwrap();
    (
        out.status.success(),
        serde_json::from_slice(&out.stdout).unwrap_or(Value::Null),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn shape(args: &[&str]) -> Value {
    let dir = temp("cli");
    let doc = dir.join("d.pen");
    let doc = doc.to_str().unwrap();
    assert!(run(&["new", doc]).0);
    let mut full = vec!["text", doc, "shape"];
    full.extend_from_slice(args);
    let (ok, json, err) = run(&full);
    assert!(ok, "{args:?}: {err}");
    json
}

#[test]
fn shaping_is_deterministic_integer_and_paginated() {
    let a = shape(&["--text", "Hello, world", "--size", "12"]);
    let b = shape(&["--text", "Hello, world", "--size", "12"]);
    assert_eq!(a, b);
    assert_eq!(a["engine"], 1);
    assert_eq!(a["size_lu"], 12 * 1024);
    assert_eq!(a["matches"], 12);
    assert!(a["glyphs"][0][2].is_i64(), "advances are integer LU");
    let total: i64 = a["glyphs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g[2].as_i64().unwrap())
        .sum();
    assert_eq!(a["advance_lu"], total);
    let page = shape(&["--text", "Hello, world", "--limit", "5", "--offset", "10"]);
    assert_eq!(page["returned"], 2);
    assert_eq!(page["has_more"], false);
    let first = shape(&["--text", "Hello, world", "--limit", "5"]);
    assert_eq!(first["returned"], 5);
    assert_eq!(first["has_more"], true);
}

#[test]
fn advances_scale_with_size_tracking_and_horizontal_scale() {
    let base = shape(&["--text", "iii", "--size", "10"])["advance_lu"]
        .as_i64()
        .unwrap();
    let double = shape(&["--text", "iii", "--size", "20"])["advance_lu"]
        .as_i64()
        .unwrap();
    assert!((double - 2 * base).abs() <= 3, "{base} {double}");
    let tracked = shape(&["--text", "iii", "--size", "10", "--tracking", "100"])["advance_lu"]
        .as_i64()
        .unwrap();
    // 100/1000 em at 10 pt = 1 pt = 1024 LU per glyph.
    assert_eq!(tracked - base, 3 * 1024);
    let wide = shape(&["--text", "iii", "--size", "10", "--h-scale", "200"])["advance_lu"]
        .as_i64()
        .unwrap();
    assert!((wide - 2 * base).abs() <= 3);
    let raised = shape(&["--text", "i", "--baseline-shift", "2"]);
    assert_eq!(raised["glyphs"][0][5], 2048);
}

#[test]
fn bidi_runs_are_ordered_and_scripts_split() {
    // Latin, then Hebrew (right to left), then Latin again.
    let result = shape(&["--text", "ab \u{5d0}\u{5d1}\u{5d2} cd"]);
    let runs = result["runs"].as_array().unwrap();
    let hebrew = runs.iter().find(|r| r["script"] == "Hebr").unwrap();
    assert_eq!(hebrew["direction"], "rtl");
    assert_eq!(hebrew["level"], 1);
    assert_eq!(hebrew["start"], 3);
    assert_eq!(hebrew["end"], 6);
    // The bundled Latin face has no Hebrew: disclosed, not hidden.
    assert_eq!(hebrew["missing_glyphs"], 3);
    assert!(result["missing_glyphs"].as_u64().unwrap() >= 3);
    // Glyph rows inside the RTL run come out in visual order (clusters descend).
    let clusters: Vec<u64> = result["glyphs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g[1].as_u64().unwrap())
        .filter(|c| (3..6).contains(c))
        .collect();
    assert_eq!(clusters, [5, 4, 3]);

    // Latin inside a right-to-left paragraph is an embedded level-2 left-to-right run.
    let forced = shape(&["--text", "abc", "--direction", "rtl"]);
    assert_eq!(forced["runs"][0]["direction"], "ltr");
    assert_eq!(forced["runs"][0]["level"], 2);
}

#[test]
fn line_break_opportunities_follow_uax14() {
    let result = shape(&["--text", "one two-three\nfour"]);
    let breaks = result["breaks"].as_array().unwrap();
    assert!(breaks.contains(&json!([4, "allowed"])), "{breaks:?}");
    assert!(breaks.contains(&json!([8, "allowed"])), "after the hyphen");
    assert_eq!(result["paragraphs"], 2);
}

#[test]
fn features_and_axes_are_parsed_and_validated() {
    assert_eq!(
        shaping::parse_features("liga,-kern,small-caps,cv05=2").unwrap(),
        [
            ("liga".to_string(), 1),
            ("kern".to_string(), 0),
            ("smcp".to_string(), 1),
            ("cv05".to_string(), 2)
        ]
    );
    // A later entry overrides an earlier one.
    assert_eq!(
        shaping::parse_features("liga,-liga").unwrap(),
        [("liga".to_string(), 0)]
    );
    assert!(shaping::parse_features("toolong").is_err());
    assert!(shaping::parse_axes("wght").is_err());
    assert!(shaping::parse_axes("wght=bold").is_err());
    assert_eq!(
        shaping::parse_axes("wght=700,wdth=90").unwrap(),
        [("wght".to_string(), 700.0), ("wdth".to_string(), 90.0)]
    );

    let dir = temp("axes");
    let doc = dir.join("d.pen");
    let doc = doc.to_str().unwrap();
    assert!(run(&["new", doc]).0);
    let (ok, _, err) = run(&["text", doc, "shape", "--text", "a", "--axes", "wght=700"]);
    assert!(!ok);
    assert!(err.contains("has no variable axis"), "{err}");
    let (ok, _, err) = run(&["text", doc, "shape", "--text", "a", "--size", "0"]);
    assert!(!ok);
    assert!(err.contains("--size"), "{err}");
    let (ok, _, err) = run(&["text", doc, "shape", "--text", "a", "--kerning", "wide"]);
    assert!(!ok);
    assert!(err.contains("kerning"), "{err}");
    let (ok, _, _) = run(&["text", doc, "shape"]);
    assert!(!ok, "text or story is required");
}

#[test]
fn optical_kerning_differs_from_metrics_and_is_bounded() {
    let raw = json!({"fonts": []});
    let font = shaping::resolve(&raw, "Atkinson Hyperlegible", 400, false).unwrap();
    let metrics = shaping::shape(&font, "AVATAR To", &ShapeParams::default()).unwrap();
    let optical = shaping::shape(
        &font,
        "AVATAR To",
        &ShapeParams {
            kerning: Kerning::Optical,
            ..ShapeParams::default()
        },
    )
    .unwrap();
    let sum = |s: &shaping::Shaped| -> i64 {
        s.runs
            .iter()
            .flat_map(|r| &r.glyphs)
            .map(|g| g.x_advance)
            .sum()
    };
    assert_ne!(sum(&metrics), sum(&optical));
    // No pair moves by more than 0.1 em plus the metric kerning it replaces.
    for (m, o) in metrics.runs[0].glyphs.iter().zip(&optical.runs[0].glyphs) {
        assert!((m.x_advance - o.x_advance).abs() <= 1229 + 600);
    }
}

#[test]
fn missing_family_is_substituted_and_disclosed() {
    let raw = json!({"fonts": []});
    let exact = shaping::resolve(&raw, "Atkinson Hyperlegible", 700, false).unwrap();
    assert_eq!(exact.source, "bundled");
    assert_eq!(exact.weight, 700);
    assert!(exact.substituted.is_none());
    let missing = shaping::resolve(&raw, "Garamond Premier", 400, true).unwrap();
    assert_eq!(missing.family, "Atkinson Hyperlegible");
    assert!(missing.italic);
    assert_eq!(
        missing.substituted.as_deref(),
        Some("family-unavailable:Garamond Premier")
    );
    assert_eq!(missing.sha256.len(), 64);
}

#[test]
fn font_report_lists_requested_fonts_with_substitution() {
    let dir = temp("report");
    let doc = dir.join("d.pen");
    let doc = doc.to_str().unwrap();
    assert!(run(&["new", doc]).0);
    let (ok, _, err) = run(&[
        "text",
        doc,
        "put",
        "t1",
        "--layer",
        "layer-1",
        "--content",
        "hi",
        "--font",
        "Papyrus",
    ]);
    assert!(ok, "{err}");
    let (ok, report, err) = run(&["text", doc, "font-report"]);
    assert!(ok, "{err}");
    assert_eq!(report["requested"], 1);
    assert_eq!(report["substituted"], 1);
    assert_eq!(report["fonts"][0]["requested"]["family"], "Papyrus");
    assert_eq!(report["fonts"][0]["missing"], true);
    assert_eq!(
        report["fonts"][0]["resolved"]["substituted"],
        "family-unavailable:Papyrus"
    );
}

#[test]
fn embedded_fonts_win_and_report_their_document_source() {
    let dir = temp("embed");
    let doc = dir.join("d.pen");
    let doc = doc.to_str().unwrap();
    assert!(run(&["new", doc]).0);
    let font = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets/fonts/AtkinsonHyperlegible-Italic.ttf");
    let (ok, _, err) = run(&["font", doc, "add", "ital", "--file", font.to_str().unwrap()]);
    assert!(ok, "{err}");
    let (ok, shaped, err) = run(&[
        "text",
        doc,
        "shape",
        "--text",
        "x",
        "--italic",
        "--font",
        "Atkinson Hyperlegible",
    ]);
    assert!(ok, "{err}");
    assert_eq!(shaped["font"]["source"], "document:ital");
}

#[test]
fn subsetting_keeps_only_used_glyphs() {
    let raw = json!({"fonts": []});
    let font = shaping::resolve(&raw, "Atkinson Hyperlegible", 400, false).unwrap();
    let shaped = shaping::shape(&font, "Hello", &ShapeParams::default()).unwrap();
    let mut ids: Vec<u32> = shaped
        .runs
        .iter()
        .flat_map(|r| r.glyphs.iter().map(|g| g.id))
        .collect();
    ids.sort_unstable();
    ids.dedup();
    let subset = shaping::subset(&font, &ids).unwrap();
    assert!(
        subset.bytes.len() < font.bytes.len() / 4,
        "{}",
        subset.bytes.len()
    );
    assert_eq!(subset.mapping.len(), ids.len());
    // Every kept glyph keeps its advance width under its new ID.
    let original = rustybuzz::ttf_parser::Face::parse(&font.bytes, font.index).unwrap();
    let reduced = rustybuzz::ttf_parser::Face::parse(&subset.bytes, 0).unwrap();
    for (old, new) in &subset.mapping {
        let was = original.glyph_hor_advance(rustybuzz::ttf_parser::GlyphId(*old as u16));
        let now = reduced.glyph_hor_advance(rustybuzz::ttf_parser::GlyphId(*new));
        assert_eq!(was, now, "glyph {old}");
    }
}
