//! v0.11.0 item 8: local adjustments — gradient, radial, range and painted
//! masks with their stage-wise parameters.
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

enum Field {
    Ascii(&'static str),
    Byte(Vec<u8>),
    Short(Vec<u16>),
    Long(Vec<u32>),
    Srational(Vec<f64>),
}

impl Field {
    fn encode(&self) -> (u16, u32, Vec<u8>) {
        match self {
            Field::Byte(v) => (1, v.len() as u32, v.clone()),
            Field::Ascii(s) => {
                let mut bytes = s.as_bytes().to_vec();
                bytes.push(0);
                (2, bytes.len() as u32, bytes)
            }
            Field::Short(v) => (
                3,
                v.len() as u32,
                v.iter().flat_map(|x| x.to_le_bytes()).collect(),
            ),
            Field::Long(v) => (
                4,
                v.len() as u32,
                v.iter().flat_map(|x| x.to_le_bytes()).collect(),
            ),
            Field::Srational(v) => (
                10,
                v.len() as u32,
                v.iter()
                    .flat_map(|x| {
                        let n = (x * 10_000.0).round() as i32;
                        [n.to_le_bytes(), 10_000i32.to_le_bytes()].concat()
                    })
                    .collect(),
            ),
        }
    }
}

/// A little-endian TIFF with one IFD; `pixels` sit at offset 8.
fn tiff(mut entries: Vec<(u16, Field)>, pixels: &[u8]) -> Vec<u8> {
    entries.sort_by_key(|(tag, _)| *tag);
    let mut out = b"II*\0".to_vec();
    let mut ifd = 8 + pixels.len();
    ifd += ifd % 2;
    out.extend((ifd as u32).to_le_bytes());
    out.extend(pixels);
    out.resize(ifd, 0);
    let mut data_at = ifd + 2 + 12 * entries.len() + 4;
    let mut data = Vec::new();
    out.extend((entries.len() as u16).to_le_bytes());
    for (tag, field) in &entries {
        let (kind, count, bytes) = field.encode();
        out.extend(tag.to_le_bytes());
        out.extend(kind.to_le_bytes());
        out.extend(count.to_le_bytes());
        if bytes.len() <= 4 {
            let mut inline = bytes.clone();
            inline.resize(4, 0);
            out.extend(inline);
        } else {
            out.extend((data_at as u32).to_le_bytes());
            data.extend(&bytes);
            data_at += bytes.len();
            if bytes.len() % 2 == 1 {
                data.push(0);
                data_at += 1;
            }
        }
    }
    out.extend(0u32.to_le_bytes());
    out.extend(data);
    out
}

const WIDTH: u32 = 40;
const HEIGHT: u32 = 30;
const HOT: (u32, u32) = (8, 8);

/// Deterministic noise in [-1, 1).
fn noise(x: u32, y: u32, c: u32) -> f64 {
    let mut h =
        (u64::from(x) * 0x9E37_79B9) ^ (u64::from(y) * 0x85EB_CA6B) ^ (u64::from(c) * 0xC2B2_AE35);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    (h % 2000) as f64 / 1000.0 - 1.0
}

/// A noisy 40x30 LinearRaw DNG: a bright left half, a dark right half, a
/// purple fringe column on the edge, a green fringe column beside it and one
/// hot pixel.
fn dng() -> Vec<u8> {
    let mut pixels = Vec::new();
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let level = if x < 20 { 30_000.0 } else { 3_000.0 };
            let mut pixel = [level, level, level];
            if x == 19 {
                pixel = [24_000.0, 4_000.0, 30_000.0];
            }
            if x == 21 {
                pixel = [2_000.0, 9_000.0, 2_000.0];
            }
            let mut pixel: [u16; 3] = std::array::from_fn(|c| {
                (pixel[c] * (1.0 + 0.08 * noise(x, y, c as u32))).clamp(0.0, 65_535.0) as u16
            });
            if (x, y) == HOT {
                pixel = [65_535; 3];
            }
            pixels.extend(pixel.iter().flat_map(|v| v.to_le_bytes()));
        }
    }
    let entries = vec![
        (254, Field::Long(vec![0])),
        (256, Field::Long(vec![WIDTH])),
        (257, Field::Long(vec![HEIGHT])),
        (258, Field::Short(vec![16; 3])),
        (259, Field::Short(vec![1])),
        (262, Field::Short(vec![34892])),
        (273, Field::Long(vec![8])),
        (277, Field::Short(vec![3])),
        (278, Field::Long(vec![HEIGHT])),
        (279, Field::Long(vec![pixels.len() as u32])),
        (50706, Field::Byte(vec![1, 4, 0, 0])),
        (50708, Field::Ascii("Pentool Local Body")),
        (
            50721,
            Field::Srational(vec![
                0.6461, -0.0907, -0.0882, -0.4300, 1.2184, 0.2378, -0.0819, 0.1944, 0.5931,
            ]),
        ),
        (50778, Field::Short(vec![21])),
        (50728, Field::Srational(vec![1.0, 1.0, 1.0])),
    ];
    tiff(entries, &pixels)
}

struct Workspace(PathBuf);

impl Workspace {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "pentool-local-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let work = Workspace(root);
        let raw = pentool::scene::new_document(40, 30);
        std::fs::write(work.path(), serde_json::to_vec_pretty(&raw).unwrap()).unwrap();
        std::fs::write(work.0.join("capture.dng"), dng()).unwrap();
        let (ok, _, error) =
            work.run(&["raw", "add", "catalog.pen", "hero", "--file", "capture.dng"]);
        assert!(ok, "{error}");
        work
    }

    fn path(&self) -> PathBuf {
        self.0.join("catalog.pen")
    }

    fn run(&self, args: &[&str]) -> (bool, Value, String) {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_pentool"))
            .args(args)
            .current_dir(&self.0)
            .output()
            .unwrap();
        let stdout = serde_json::from_slice(&output.stdout).unwrap_or(Value::Null);
        (
            output.status.success(),
            stdout,
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    }

    fn develop(&self, args: &[&str]) {
        let mut command = vec!["raw", "develop", "catalog.pen", "hero"];
        command.extend(args);
        let (ok, _, error) = self.run(&command);
        assert!(ok, "{command:?}: {error}");
    }

    fn master(&self) -> Value {
        read(&self.path())["photography"]["photos"][0]["variants"][0]["develop"].clone()
    }

    fn try_render(&self) -> anyhow::Result<Vec<f32>> {
        let raw = read(&self.path());
        Ok(
            pentool::photo::catalog::render_photo(&raw, &self.path(), "hero", "master")?
                .image
                .rgb,
        )
    }

    fn render(&self) -> Vec<f32> {
        self.try_render().unwrap()
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn read(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn rgb(rgb: &[f32], x: u32, y: u32) -> [f32; 3] {
    let i = ((y * WIDTH + x) * 3) as usize;
    [rgb[i], rgb[i + 1], rgb[i + 2]]
}

fn sum(rgb: [f32; 3]) -> f32 {
    rgb[0] + rgb[1] + rgb[2]
}

const LEFT: &str = r#"{"kind": "linear", "mode": "add", "start": [0.25, 0.5], "end": [0.5, 0.5]}"#;

fn local(id: &str, component: &str, params: &str) -> String {
    format!(
        r#"local=[{{"id": "{id}", "mask": {{"components": [{component}]}}, "params": {params}}}]"#
    )
}

#[test]
fn a_linear_mask_brightens_one_side_and_ignores_the_crop() {
    let work = Workspace::new("linear");
    let base = work.render();
    work.develop(&["--set", &local("left", LEFT, r#"{"exposure": 1}"#)]);
    let lit = work.render();
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let (a, b) = (rgb(&lit, x, y), rgb(&base, x, y));
            if x < 10 {
                assert!(sum(a) > 1.8 * sum(b), "({x}, {y})");
            } else if x >= 20 {
                assert_eq!(a, b, "({x}, {y})");
            }
        }
    }
    // The mask lives in frame coordinates: cropping to the right half shows
    // none of it, cropping to the left half keeps it.
    work.develop(&["--set", "crop.rect=[0.5,0,0.5,1]"]);
    let masked = work.render();
    work.develop(&["--unset", "local"]);
    assert_eq!(work.render(), masked);
    work.develop(&["--set", "crop.rect=[0,0,0.5,1]"]);
    let plain = work.render();
    work.develop(&["--set", &local("left", LEFT, r#"{"exposure": 1}"#)]);
    assert_ne!(work.render(), plain);
}

#[test]
fn every_local_parameter_changes_the_image_deterministically() {
    let work = Workspace::new("params");
    let base = work.render();
    let everywhere = r#"{"kind": "radial", "mode": "add", "center": [0.5, 0.5], "radius": [2, 2]}"#;
    for params in [
        r#"{"exposure": 1}"#,
        r#"{"contrast": 60}"#,
        r#"{"highlights": -60}"#,
        r#"{"shadows": 60}"#,
        r#"{"whites": 50}"#,
        r#"{"temperature": 50}"#,
        r#"{"tint": 50}"#,
        r#"{"texture": 80}"#,
        r#"{"clarity": 80}"#,
        r#"{"dehaze": 60}"#,
        r#"{"hue": 40}"#,
        r#"{"saturation": 60}"#,
        r#"{"sharpness": 80}"#,
        r#"{"sharpness": -80}"#,
        r#"{"noise": 90}"#,
        r#"{"moire": 90}"#,
        r#"{"defringe": 90}"#,
        r#"{"color": {"hue": 200, "saturation": 60}}"#,
    ] {
        work.develop(&["--set", &local("all", everywhere, params)]);
        let first = work.render();
        assert_ne!(first, base, "{params} had no effect");
        assert_eq!(work.render(), first, "{params} is not deterministic");
        assert!(first.iter().all(|v| v.is_finite()), "{params}");
        let disabled = local("all", everywhere, params)
            .replace(r#""id": "all""#, r#""id": "all", "enabled": false"#);
        work.develop(&["--set", &disabled]);
        assert_eq!(work.render(), base, "disabling {params}");
        work.develop(&["--unset", "local"]);
        assert_eq!(work.render(), base, "unsetting {params}");
    }
    // Blacks reach only deep shadows, which this capture lacks until it is
    // darkened.
    work.develop(&["--set", "tone.exposure=-4"]);
    let dark = work.render();
    work.develop(&["--set", &local("all", everywhere, r#"{"blacks": -50}"#)]);
    assert_ne!(work.render(), dark);
}

#[test]
fn a_local_dehaze_resolves_the_airlight() {
    let work = Workspace::new("dehaze");
    assert!(work.master()["presence"].get("dehaze_airlight").is_none());
    work.develop(&["--set", &local("haze", LEFT, r#"{"dehaze": 50}"#)]);
    assert!(work.master()["presence"]["dehaze_airlight"].is_array());
    work.render();
}

#[test]
fn a_luminance_range_selects_the_bright_half() {
    let work = Workspace::new("range");
    let base = work.render();
    let range = r#"{"kind": "range-luminance", "mode": "add", "min": 0.6, "max": 1}"#;
    work.develop(&["--set", &local("bright", range, r#"{"exposure": -1}"#)]);
    let dimmed = work.render();
    for y in 0..HEIGHT {
        for x in (2..17).chain(23..WIDTH) {
            let (a, b) = (rgb(&dimmed, x, y), rgb(&base, x, y));
            if x < 20 && (x, y) != HOT {
                assert!(sum(a) < 0.75 * sum(b), "({x}, {y})");
            } else if x >= 20 {
                assert_eq!(a, b, "({x}, {y})");
            }
        }
    }
    let raw = read(&work.path());
    let report = pentool::photo::catalog::render_photo(&raw, &work.path(), "hero", "master")
        .unwrap()
        .report;
    let coverage = report["local"][0]["coverage"].as_f64().unwrap();
    assert!((0.3..0.6).contains(&coverage), "{coverage}");
}

const STROKE: &str = "[[4,5,1],[16,5,1]]";
const BRUSH: &str = r#"{"size": 6, "hardness": 1}"#;

#[test]
fn painted_masks_are_raster_tiles_and_erase_restores() {
    let work = Workspace::new("paint");
    let empty = r#"{"kind": "brush", "mode": "add", "width": 40, "height": 30, "tiles": {}}"#;
    let dodge = local("dodge", empty, r#"{"exposure": 1}"#);
    work.develop(&["--set", &dodge]);
    let base = work.render();
    let paint = |extra: &[&str]| {
        let mut args = vec![
            "photo",
            "mask",
            "paint",
            "catalog.pen",
            "hero",
            "--adjustment",
            "dodge",
            "--samples",
            STROKE,
        ];
        args.extend(extra);
        if !extra.contains(&"--brush") {
            args.extend(["--brush", BRUSH]);
        }
        work.run(&args)
    };

    let before = std::fs::read(work.path()).unwrap();
    let (ok, preview, error) = paint(&["--component", "0", "--dry-run"]);
    let preview = &preview["result"];
    assert!(ok, "{error}");
    assert_eq!(std::fs::read(work.path()).unwrap(), before);

    let (ok, result, error) = paint(&["--component", "0"]);
    let result = &result["result"];
    assert!(ok, "{error}");
    assert_eq!(result["plane"], json!([40, 30]));
    assert_eq!(result["component"], 0);
    assert_eq!(result["dabs"], preview["dabs"]);
    let raw = read(&work.path());
    let component = &work.master()["local"][0]["mask"]["components"][0];
    let tiles = component["tiles"].as_object().unwrap();
    assert!(!tiles.is_empty());
    for digest in tiles.values() {
        assert!(raw["raster_tiles"].get(digest.as_str().unwrap()).is_some());
    }
    let painted = work.render();
    assert!(sum(rgb(&painted, 10, 5)) > 1.8 * sum(rgb(&base, 10, 5)));
    for y in 12..HEIGHT {
        for x in 0..WIDTH {
            assert_eq!(rgb(&painted, x, y), rgb(&base, x, y), "({x}, {y})");
        }
    }

    // Without --component a new brush component is added.
    let (ok, result, error) = paint(&[]);
    assert!(ok, "{error}");
    assert_eq!(result["result"]["component"], 1);
    work.develop(&["--set", &dodge]);
    assert_eq!(work.render(), base);

    let (ok, _, error) = paint(&["--component", "0"]);
    assert!(ok, "{error}");
    let (ok, _, error) = paint(&[
        "--component",
        "0",
        "--erase",
        "--brush",
        r#"{"size": 14, "hardness": 1}"#,
    ]);
    assert!(ok, "{error}");
    assert_eq!(work.render(), base);
}

#[test]
fn painting_refuses_bad_targets_without_writing() {
    let work = Workspace::new("paint-refuse");
    work.develop(&["--set", &local("grad", LEFT, r#"{"exposure": 1}"#)]);
    for (extra, code) in [
        (vec!["--adjustment", "missing"], "[missing-resource]"),
        (
            vec!["--adjustment", "grad", "--component", "0"],
            "[invalid-input]",
        ),
        (
            vec!["--adjustment", "grad", "--component", "3"],
            "[invalid-input]",
        ),
    ] {
        let before = std::fs::read(work.path()).unwrap();
        let mut args = vec![
            "photo",
            "mask",
            "paint",
            "catalog.pen",
            "hero",
            "--samples",
            STROKE,
        ];
        args.extend(extra);
        let (ok, _, error) = work.run(&args);
        assert!(!ok);
        assert!(error.contains(code), "{error}");
        assert_eq!(std::fs::read(work.path()).unwrap(), before);
    }
}

#[test]
fn invalid_local_adjustments_are_refused_without_writing() {
    let work = Workspace::new("refuse");
    let radial = r#"{"kind": "radial", "mode": "add", "center": [0.5, 0.5], "radius": [0.2, 0.2]}"#;
    let seventeen = vec![radial; 17].join(", ");
    let zero = "0".repeat(64);
    let missing_tile = format!(
        r#"{{"kind": "brush", "mode": "add", "width": 40, "height": 30, "tiles": {{"0,0": "sha256:{zero}"}}}}"#
    );
    let subtract_first =
        r#"{"kind": "radial", "mode": "subtract", "center": [0.5, 0.5], "radius": [0.2, 0.2]}"#;
    let depth = r#"{"kind": "depth", "mode": "add"}"#;
    let mask = format!(r#"{{"kind": "mask", "mode": "add", "resource": "sha256:{zero}"}}"#);
    let degenerate =
        r#"{"kind": "linear", "mode": "add", "start": [0.25, 0.5], "end": [0.25, 0.5]}"#;
    let twice = format!(
        r#"local=[{{"id": "a", "mask": {{"components": [{radial}]}}, "params": {{"exposure": 1}}}}, {{"id": "a", "mask": {{"components": [{radial}]}}, "params": {{"exposure": 1}}}}]"#
    );
    let exposure = r#"{"exposure": 1}"#;
    for (assignment, code) in [
        (local("a", depth, exposure), "[unsupported-capability]"),
        (local("a", &missing_tile, exposure), "[missing-resource]"),
        (local("a", &seventeen, exposure), "[invalid-develop]"),
        (local("a", subtract_first, exposure), "[invalid-develop]"),
        (twice, "[invalid-develop]"),
        (local("a", &mask, exposure), "[missing-resource]"),
        (local("a", degenerate, exposure), "[invalid-develop]"),
        (
            local("a", radial, r#"{"exposure": 6}"#),
            "[invalid-develop]",
        ),
        (local("a", radial, "{}"), "[invalid-develop]"),
        (local("a", radial, r#"{"grain": 10}"#), "[invalid-develop]"),
        (
            local("a", radial, r#"{"color": {"hue": 20}}"#),
            "[invalid-develop]",
        ),
    ] {
        let before = std::fs::read(work.path()).unwrap();
        let (ok, _, error) = work.run(&[
            "raw",
            "develop",
            "catalog.pen",
            "hero",
            "--set",
            &assignment,
        ]);
        assert!(!ok, "{assignment} should fail");
        assert!(error.contains(code), "{assignment}: {error}");
        assert_eq!(std::fs::read(work.path()).unwrap(), before);
    }
}
