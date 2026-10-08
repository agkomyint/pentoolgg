//! v0.11.0 item 7: detail processing — defective pixels, noise reduction,
//! moiré, defringe and capture sharpening.
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
        (50708, Field::Ascii("Pentool Detail Body")),
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
            "pentool-detail-{tag}-{}-{}",
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

fn green(rgb: &[f32], x: u32, y: u32) -> f32 {
    rgb[((y * WIDTH + x) * 3 + 1) as usize]
}

#[test]
fn import_defaults_hold_capture_sharpening_and_color_noise() {
    let work = Workspace::new("defaults");
    assert_eq!(
        work.master()["detail"],
        json!({
            "sharpening": {"amount": 40, "radius": 1, "detail": 25},
            "noise": {"color": 25}
        })
    );
}

#[test]
fn listed_and_automatic_defects_are_repaired() {
    let work = Workspace::new("defects");
    work.develop(&["--unset", "detail"]);
    let base = work.render();
    let neighbor = green(&base, HOT.0 + 1, HOT.1);
    assert!(green(&base, HOT.0, HOT.1) > 1.5 * neighbor);

    work.develop(&["--set", "raw.defective_pixels={\"list\": [[8, 8]]}"]);
    let listed = work.render();
    assert!((green(&listed, HOT.0, HOT.1) / neighbor - 1.0).abs() < 0.15);
    // Nothing away from the listed pixel changes (highlight blending reads a
    // small neighborhood, so the ring around it may).
    for (i, (a, b)) in listed.iter().zip(&base).enumerate() {
        let (x, y) = ((i / 3) as u32 % WIDTH, (i / 3) as u32 / WIDTH);
        if x.abs_diff(HOT.0) > 2 || y.abs_diff(HOT.1) > 2 {
            assert_eq!(a, b, "sample {i}");
        }
    }

    work.develop(&[
        "--set",
        "raw.defective_pixels={\"auto\": true, \"threshold\": 60}",
    ]);
    let automatic = work.render();
    assert!((green(&automatic, HOT.0, HOT.1) / neighbor - 1.0).abs() < 0.15);

    work.develop(&["--set", "raw.defective_pixels={\"list\": [[40, 2]]}"]);
    let error = work.try_render().unwrap_err().to_string();
    assert!(error.contains("[invalid-develop]"), "{error}");
}

#[test]
fn every_detail_control_changes_the_image_deterministically() {
    let work = Workspace::new("controls");
    work.develop(&["--unset", "detail"]);
    let base = work.render();
    for (key, value) in [
        ("detail.noise", r#"{"luminance": 70}"#),
        (
            "detail.noise",
            r#"{"luminance": 70, "luminance_contrast": 80}"#,
        ),
        ("detail.noise", r#"{"color": 60, "color_smoothness": 90}"#),
        ("detail.moire", "80"),
        ("detail.sharpening", r#"{"amount": 120, "radius": 1.5}"#),
        (
            "detail.sharpening",
            r#"{"amount": 120, "radius": 1.5, "detail": 90, "masking": 40}"#,
        ),
        ("lens.defringe", r#"{"purple_amount": 15}"#),
        ("lens.defringe", r#"{"green_amount": 15}"#),
    ] {
        let assignment = format!("{key}={value}");
        work.develop(&["--set", &assignment]);
        let first = work.render();
        assert_ne!(first, base, "{assignment} had no effect");
        assert_eq!(work.render(), first, "{assignment} is not deterministic");
        assert!(first.iter().all(|v| v.is_finite()), "{assignment}");
        work.develop(&["--unset", key]);
        assert_eq!(work.render(), base, "unsetting {key}");
    }
}

/// Sample variance of luminance over the bright half, away from the
/// edge and the hot pixel.
fn bright_variance(rgb: &[f32]) -> f64 {
    let values: Vec<f64> = (2..HEIGHT - 2)
        .flat_map(|y| (12..17).map(move |x| (x, y)))
        .map(|(x, y)| {
            let i = ((y * WIDTH + x) * 3) as usize;
            0.288 * f64::from(rgb[i]) + 0.712 * f64::from(rgb[i + 1])
        })
        .collect();
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    values.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>() / values.len() as f64
}

#[test]
fn noise_reduction_smooths_and_defringe_desaturates() {
    let work = Workspace::new("strength");
    work.develop(&["--unset", "detail"]);
    let base = work.render();
    work.develop(&["--set", "detail.noise={\"luminance\": 80}"]);
    let smooth = work.render();
    let (after, before) = (bright_variance(&smooth), bright_variance(&base));
    assert!(after < 0.5 * before, "{after} vs {before}");

    work.develop(&["--unset", "detail"]);
    work.develop(&["--set", "lens.defringe={\"purple_amount\": 20}"]);
    let defringed = work.render();
    let chroma = |rgb: &[f32]| {
        let i = ((10 * WIDTH + 19) * 3) as usize;
        (rgb[i] - rgb[i + 1]).abs() + (rgb[i + 2] - rgb[i + 1]).abs()
    };
    assert!(chroma(&defringed) < 0.5 * chroma(&base));
}

#[test]
fn invalid_detail_settings_are_refused_without_writing() {
    let work = Workspace::new("refuse");
    for assignment in [
        "detail.sharpening.amount=200",
        "detail.sharpening.radius=0.2",
        "detail.noise.grain=10",
        "detail.moire=-5",
        "detail.extra=1",
    ] {
        let before = std::fs::read(work.path()).unwrap();
        let (ok, _, error) =
            work.run(&["raw", "develop", "catalog.pen", "hero", "--set", assignment]);
        assert!(!ok, "{assignment} should fail");
        assert!(error.contains("[invalid-develop]"), "{assignment}: {error}");
        assert_eq!(std::fs::read(work.path()).unwrap(), before);
    }
}
