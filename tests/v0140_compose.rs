//! v0.14.0 task 5: the paragraph composer.
use pentool::compose::{pattern_set, split_lines};
use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;

const TEXT: &str = "The quick brown fox jumps over the lazy dog while international \
organizations consider uncharacteristically extraordinary circumstances, and the \
administrators reconsider their typographical recommendations.";

fn temp(name: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir =
        std::env::temp_dir().join(format!("pentool-compose-{name}-{n}-{}", std::process::id()));
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

fn compose(extra: &[&str]) -> Value {
    let dir = temp("cli");
    let doc = dir.join("d.pen");
    let doc = doc.to_str().unwrap();
    assert!(run(&["new", doc]).0);
    let mut args = vec!["text", doc, "compose"];
    args.extend_from_slice(extra);
    let (ok, json, err) = run(&args);
    assert!(ok, "{extra:?}: {err}");
    json
}

fn lines(v: &Value) -> &Vec<Value> {
    v["lines"].as_array().unwrap()
}

#[test]
fn composition_is_deterministic_and_fits() {
    let args = ["--text", TEXT, "--width", "300", "--size", "12"];
    let a = compose(&args);
    let b = compose(&args);
    assert_eq!(a, b);
    assert_eq!(a["engine"], 1);
    assert_eq!(a["summary"]["overfull"], 0);
    let target = 300 * 1024;
    for line in lines(&a) {
        assert!(line["width_lu"].as_i64().unwrap() <= target, "{line}");
        assert!(line["end"].as_u64() >= line["start"].as_u64());
    }
    // Lines tile the text without overlap.
    let rows = lines(&a);
    for pair in rows.windows(2) {
        assert!(pair[1]["start"].as_u64() >= pair[0]["end"].as_u64());
    }
}

#[test]
fn narrower_columns_need_more_lines() {
    let count = |w: &str| {
        compose(&["--text", TEXT, "--width", w])["summary"]["lines"]
            .as_u64()
            .unwrap()
    };
    assert!(count("200") <= count("120"));
    assert!(count("400") < count("120"));
}

#[test]
fn justified_lines_fill_the_measure_except_the_last() {
    let out = compose(&[
        "--text",
        TEXT,
        "--width",
        "330",
        "--size",
        "11",
        "--align",
        "justify",
        "--hyphenate",
        "true",
    ]);
    let rows = lines(&out);
    assert!(rows.len() > 2);
    let target = 330 * 1024;
    let mut filled = 0;
    for line in &rows[..rows.len() - 1] {
        let w = line["width_lu"].as_i64().unwrap();
        assert!(w <= target + 1, "{line}");
        if (target - w).abs() <= 2 {
            filled += 1;
        }
    }
    assert!(filled >= rows.len() - 2, "{filled}/{} {out}", rows.len());
    let last = rows.last().unwrap();
    assert!(last["width_lu"].as_i64().unwrap() < target);
}

#[test]
fn hyphenation_reduces_overfull_lines_and_marks_hyphens() {
    let off = compose(&[
        "--text",
        "internationalization internationalization internationalization",
        "--width",
        "70",
        "--hyphenate",
        "false",
    ]);
    let on = compose(&[
        "--text",
        "internationalization internationalization internationalization",
        "--width",
        "70",
        "--hyphenate",
        "true",
    ]);
    assert!(off["summary"]["overfull"].as_u64().unwrap() > 0);
    assert_eq!(on["summary"]["overfull"], 0);
    assert!(on["summary"]["hyphens"].as_u64().unwrap() > 0);
    assert!(lines(&on).iter().any(|l| l["hyphenated"] == true));
}

#[test]
fn unknown_language_discloses_missing_patterns() {
    let out = compose(&[
        "--text",
        TEXT,
        "--width",
        "200",
        "--hyphenate",
        "true",
        "--language",
        "xx",
    ]);
    assert_eq!(out["summary"]["hyphenation_unavailable"], true);
    assert_eq!(out["summary"]["hyphens"], 0);
    assert_eq!(pattern_set("en-US"), Some("en-us"));
    assert_eq!(pattern_set("en-GB"), Some("en-gb"));
    assert_eq!(pattern_set("de-CH"), Some("de-1996"));
    assert_eq!(pattern_set("cs"), None);
}

#[test]
fn single_line_composer_fills_greedily() {
    let out = compose(&[
        "--text",
        TEXT,
        "--width",
        "200",
        "--composer",
        "single-line",
    ]);
    assert_eq!(out["composer"], "single-line");
    assert_eq!(out["summary"]["overfull"], 0);
    // First-fit never beats total-fit on demerits, but every line is within the measure.
    for line in lines(&out) {
        assert!(line["natural_lu"].as_i64().unwrap() <= 200 * 1024);
    }
}

#[test]
fn explicit_newlines_start_new_paragraphs() {
    let out = compose(&["--text", "alpha beta\ngamma", "--width", "300"]);
    assert_eq!(out["summary"]["paragraphs"], 2);
    assert_eq!(out["summary"]["lines"], 2);
}

#[test]
fn pagination_is_exact() {
    let all = compose(&["--text", TEXT, "--width", "120"]);
    let total = all["matches"].as_u64().unwrap();
    assert!(total > 4);
    let page = compose(&[
        "--text", TEXT, "--width", "120", "--limit", "2", "--offset", "1",
    ]);
    assert_eq!(page["returned"], 2);
    assert_eq!(page["matches"], total);
    assert_eq!(page["has_more"], true);
    assert_eq!(page["lines"][0], all["lines"][1]);
}

#[test]
fn invalid_input_is_rejected() {
    let dir = temp("bad");
    let doc = dir.join("d.pen");
    let doc = doc.to_str().unwrap();
    assert!(run(&["new", doc]).0);
    let (ok, _, err) = run(&["text", doc, "compose", "--text", "a", "--width", "0"]);
    assert!(!ok);
    assert!(err.contains("--width"), "{err}");
    let (ok, _, _) = run(&["text", doc, "compose", "--width", "100"]);
    assert!(!ok);
    let (ok, _, err) = run(&[
        "text", doc, "compose", "--text", "a", "--width", "100", "--style", "missing",
    ]);
    assert!(!ok);
    assert!(err.contains("missing"), "{err}");
    let (ok, _, err) = run(&[
        "text", doc, "compose", "--text", "a", "--width", "100", "--align", "wobbly",
    ]);
    assert!(!ok);
    assert!(err.contains("alignment"), "{err}");
}

#[test]
fn keep_options_split_lines() {
    // Everything fits.
    assert_eq!(split_lines(4, 6, 2, 2, false), Some(4));
    // A clean split.
    assert_eq!(split_lines(8, 5, 2, 2, false), Some(5));
    // Widow control pulls a line over.
    assert_eq!(split_lines(6, 5, 2, 2, false), Some(4));
    // Orphan control refuses a single line at the bottom.
    assert_eq!(split_lines(6, 1, 2, 2, false), None);
    // Keep together moves the whole paragraph.
    assert_eq!(split_lines(6, 5, 2, 2, true), None);
    // A paragraph too short to satisfy both stays whole.
    assert_eq!(split_lines(3, 2, 2, 2, false), None);
}
