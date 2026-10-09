//! v0.11.0 item 6: the development stack (tone, presence, curves, color,
//! effects and calibration), `raw develop --exposure` / `--auto-tone`, and the
//! resolved dehaze airlight.
use pentool::photo::adjust::Oklab;
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

/// A 40x30 LinearRaw DNG holding a colorful gradient with deep shadows and
/// clipped-near highlights; `baseline` adds a BaselineExposure tag.
fn dng(baseline: Option<f64>) -> Vec<u8> {
    let mut pixels = Vec::new();
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let t = (x + y * WIDTH) as f64 / (WIDTH * HEIGHT) as f64;
            let level = 40.0 + 60_000.0 * t * t;
            let pixel = [
                level * (0.5 + 0.5 * (x as f64 / WIDTH as f64)),
                level * 0.8,
                level * (1.0 - 0.6 * (y as f64 / HEIGHT as f64)),
            ]
            .map(|v| v.min(65_535.0) as u16);
            pixels.extend(pixel.iter().flat_map(|v| v.to_le_bytes()));
        }
    }
    let mut entries = vec![
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
        (50708, Field::Ascii("Pentool Development Body")),
        (
            50721,
            Field::Srational(vec![
                0.6461, -0.0907, -0.0882, -0.4300, 1.2184, 0.2378, -0.0819, 0.1944, 0.5931,
            ]),
        ),
        (50778, Field::Short(vec![21])),
        (50728, Field::Srational(vec![0.5, 1.0, 0.75])),
    ];
    if let Some(ev) = baseline {
        entries.push((50730, Field::Srational(vec![ev])));
    }
    tiff(entries, &pixels)
}

struct Workspace(PathBuf);

impl Workspace {
    fn new(tag: &str, baseline: Option<f64>) -> Self {
        let root = std::env::temp_dir().join(format!(
            "pentool-develop-{tag}-{}-{}",
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
        std::fs::write(work.0.join("capture.dng"), dng(baseline)).unwrap();
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

    fn develop(&self, args: &[&str]) -> Value {
        let mut command = vec!["raw", "develop", "catalog.pen", "hero"];
        command.extend(args);
        let (ok, out, error) = self.run(&command);
        assert!(ok, "{command:?}: {error}");
        out["result"].clone()
    }

    /// `raw develop` that must fail; returns stderr and checks the document
    /// is unchanged.
    fn refuse(&self, args: &[&str]) -> String {
        let before = std::fs::read(self.path()).unwrap();
        let mut command = vec!["raw", "develop", "catalog.pen", "hero"];
        command.extend(args);
        let (ok, _, error) = self.run(&command);
        assert!(!ok, "{command:?} should fail");
        assert_eq!(std::fs::read(self.path()).unwrap(), before, "{command:?}");
        error
    }

    fn master(&self) -> Value {
        read(&self.path())["photography"]["photos"][0]["variants"][0]["develop"].clone()
    }

    fn render(&self) -> Vec<f32> {
        let raw = read(&self.path());
        pentool::photo::catalog::render_photo(&raw, &self.path(), "hero", "master")
            .unwrap()
            .image
            .rgb
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

fn assert_scaled(scaled: &[f32], base: &[f32], factor: f32) {
    assert_eq!(scaled.len(), base.len());
    for (s, b) in scaled.iter().zip(base) {
        assert!(
            (s - b * factor).abs() <= 1e-5 * (b * factor).abs().max(1e-3),
            "{s} vs {b} x {factor}"
        );
    }
}

#[test]
fn exposure_scales_linearly_and_identity_round_trips() {
    let work = Workspace::new("exposure", None);
    let base = work.render();
    work.develop(&["--exposure", "1"]);
    assert_eq!(work.master()["tone"], json!({"exposure": 1}));
    assert_scaled(&work.render(), &base, 2.0);
    work.develop(&["--exposure", "-1.5"]);
    assert_scaled(&work.render(), &base, 0.353_553_4);
    // Back at zero the pixels are bit-identical to the untouched develop.
    work.develop(&["--exposure", "0"]);
    assert_eq!(work.render(), base);
}

#[test]
fn baseline_exposure_is_applied() {
    let plain = Workspace::new("plain", None);
    let boosted = Workspace::new("baseline", Some(1.0));
    assert_scaled(&boosted.render(), &plain.render(), 2.0);
}

#[test]
fn every_control_changes_the_image_deterministically() {
    let work = Workspace::new("controls", None);
    let base = work.render();
    for (key, value) in [
        ("tone.contrast", "40"),
        ("tone.highlights", "-80"),
        ("tone.shadows", "70"),
        ("tone.whites", "50"),
        ("tone.blacks", "-50"),
        ("presence.clarity", "60"),
        ("presence.texture", "-60"),
        ("presence.vibrance", "50"),
        ("presence.saturation", "-40"),
        ("curves.parametric", r#"{"darks": 60, "lights": -40}"#),
        (
            "curves.point",
            r#"{"rgb": [[0, 0], [0.4, 0.55], [1, 1]], "red": [[0, 0.05], [1, 0.9]]}"#,
        ),
        ("hsl.hue", r#"{"orange": 60, "yellow": 40, "green": 40}"#),
        (
            "hsl.saturation",
            r#"{"red": -80, "orange": -80, "yellow": -80, "green": -80}"#,
        ),
        ("hsl.luminance", r#"{"orange": 60, "yellow": 60}"#),
        (
            "grading",
            r#"{"shadows": {"hue": 220, "saturation": 60}, "highlights": {"hue": 40, "saturation": 40}}"#,
        ),
        ("effects.vignette", r#"{"amount": -60, "feather": 30}"#),
        (
            "effects.grain",
            r#"{"amount": 80, "size": 0, "roughness": 100, "seed": 9}"#,
        ),
        (
            "calibration",
            r#"{"red": {"hue": 60, "saturation": 40}, "shadows_tint": 80}"#,
        ),
    ] {
        let assignment = format!("{key}={value}");
        work.develop(&["--set", &assignment]);
        let first = work.render();
        assert_ne!(first, base, "{key} had no effect");
        assert_eq!(work.render(), first, "{key} is not deterministic");
        assert!(first.iter().all(|v| v.is_finite()), "{key}");
        work.develop(&["--unset", key]);
        assert_eq!(work.render(), base, "unsetting {key}");
    }
}

#[test]
fn invalid_settings_are_refused_without_writing() {
    let work = Workspace::new("refuse", None);
    for (args, code) in [
        (vec!["--exposure", "6"], "[invalid-develop]"),
        (vec!["--exposure", "1", "--auto-tone"], "[invalid-input]"),
        (
            vec!["--set", "effects.grain={\"amount\": 20}"],
            "[invalid-develop]",
        ),
        (
            vec!["--set", "curves.point={\"rgb\": [[0, 0]]}"],
            "[invalid-develop]",
        ),
        (vec!["--set", "hsl.hue={\"teal\": 4}"], "[invalid-develop]"),
        (vec!["--set", "grading.balance=200"], "[invalid-develop]"),
    ] {
        let error = work.refuse(&args);
        assert!(error.contains(code), "{args:?}: {error}");
    }
    // Common slips name the setting that works.
    for (args, hint) in [
        (
            vec!["--set", "effects.grain={\"amount\": 20}"],
            "\"seed\": 1",
        ),
        (vec!["--set", "monochrome=true"], "monochrome.enabled=true"),
        (
            vec!["--set", "geometry.crop={\"rect\": [0, 0, 0.5, 0.5]}"],
            "use crop.rect",
        ),
    ] {
        let error = work.refuse(&args);
        assert!(error.contains(hint), "{args:?}: {error}");
    }
}

#[test]
fn auto_tone_resolves_and_records_provenance() {
    let work = Workspace::new("auto", None);
    let result = work.develop(&["--auto-tone"]);
    let tone = work.master()["tone"].clone();
    assert_eq!(result["tone"], tone);
    assert_eq!(
        tone["auto"],
        json!({"algorithm": "auto-tone", "version": 1})
    );
    let exposure = tone["exposure"].as_f64().unwrap();
    assert!(
        (-5.0..=5.0).contains(&exposure) && exposure != 0.0,
        "{tone}"
    );
    // Deterministic: running it again stores the same values.
    work.develop(&["--auto-tone"]);
    assert_eq!(work.master()["tone"], tone);
    // A hand edit of a tone value drops the provenance but keeps the values.
    work.develop(&["--set", "tone.contrast=10"]);
    let edited = work.master()["tone"].clone();
    assert!(edited.get("auto").is_none(), "{edited}");
    assert_eq!(edited["exposure"], tone["exposure"]);
}

#[test]
fn dehaze_resolves_the_airlight_and_render_requires_it() {
    let work = Workspace::new("dehaze", None);
    let base = work.render();
    let result = work.develop(&["--set", "presence.dehaze=50"]);
    let airlight = work.master()["presence"]["dehaze_airlight"].clone();
    assert_eq!(result["dehaze_airlight"], airlight);
    assert_eq!(airlight.as_array().map(Vec::len), Some(3), "{airlight}");
    let hazed = work.render();
    assert_ne!(hazed, base);
    // Unchanged inputs keep the stored airlight; a tone change re-resolves it.
    let result = work.develop(&["--set", "presence.dehaze=60"]);
    assert!(result.get("dehaze_airlight").is_none());
    let result = work.develop(&["--exposure", "-1"]);
    assert!(result.get("dehaze_airlight").is_some());
    work.develop(&["--set", "presence.dehaze=-40"]);
    assert_ne!(work.render(), base);

    // A document edited by hand without the airlight is refused at render.
    let mut raw = read(&work.path());
    raw["photography"]["photos"][0]["variants"][0]["develop"]["presence"] = json!({"dehaze": 30});
    std::fs::write(work.path(), serde_json::to_vec_pretty(&raw).unwrap()).unwrap();
    let error = pentool::photo::catalog::render_photo(&raw, &work.path(), "hero", "master")
        .err()
        .expect("render without an airlight")
        .to_string();
    assert!(
        error.contains("[invalid-develop]") && error.contains("dehaze_airlight"),
        "{error}"
    );
}

#[test]
fn monochrome_renders_neutral_grays() {
    let work = Workspace::new("mono", None);
    work.develop(&[
        "--set",
        "monochrome={\"enabled\": true, \"mix\": {\"red\": 40, \"blue\": -30}}",
    ]);
    let oklab = Oklab::new();
    for p in work.render().chunks_exact(3) {
        let lab = oklab.forward([f64::from(p[0]), f64::from(p[1]), f64::from(p[2])]);
        assert!(lab[1].abs() < 1e-4 && lab[2].abs() < 1e-4, "{p:?} {lab:?}");
    }
}

#[test]
fn grain_follows_its_seed() {
    let work = Workspace::new("grain", None);
    work.develop(&["--set", "effects.grain={\"amount\": 50, \"seed\": 1}"]);
    let one = work.render();
    work.develop(&["--set", "effects.grain.seed=2"]);
    let two = work.render();
    assert_ne!(one, two);
    work.develop(&["--set", "effects.grain.seed=1"]);
    assert_eq!(work.render(), one);
}
