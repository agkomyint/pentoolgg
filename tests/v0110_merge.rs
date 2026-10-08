//! v0.11.0 item 9: HDR and panorama merges — alignment, exposure
//! normalization, deghosting, projection, seam blending, attribution and
//! non-destructive derived output.
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

/// A neutral uint16 LinearRaw DNG whose raw values are `value(x, y)`,
/// clipped at the 65535 white level.
fn dng(width: u32, height: u32, value: impl Fn(u32, u32) -> f64) -> Vec<u8> {
    let mut pixels = Vec::new();
    for y in 0..height {
        for x in 0..width {
            let v = value(x, y).clamp(0.0, 65_535.0).round() as u16;
            for _ in 0..3 {
                pixels.extend(v.to_le_bytes());
            }
        }
    }
    let entries = vec![
        (254, Field::Long(vec![0])),
        (256, Field::Long(vec![width])),
        (257, Field::Long(vec![height])),
        (258, Field::Short(vec![16; 3])),
        (259, Field::Short(vec![1])),
        (262, Field::Short(vec![34892])),
        (273, Field::Long(vec![8])),
        (277, Field::Short(vec![3])),
        (278, Field::Long(vec![height])),
        (279, Field::Long(vec![pixels.len() as u32])),
        (50706, Field::Byte(vec![1, 4, 0, 0])),
        (50708, Field::Ascii("Pentool Merge Body")),
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

const W: u32 = 96;
const H: u32 = 72;
/// Raw value of unit radiance at unit exposure.
const BASE: f64 = 25_000.0;

/// Smooth scene radiance from 1/16 to 4, defined everywhere.
fn radiance(x: f64, y: f64) -> f64 {
    let s = 0.5 + 0.25 * (x / 5.0).sin() * (y / 7.0).cos() + 0.25 * ((x + 2.0 * y) / 11.0).sin();
    (2.0f64).powf(6.0 * s - 4.0)
}

/// Deterministic value noise in [0, 1) per 5-pixel cell.
fn cell(x: i64, y: i64) -> f64 {
    let mut h = (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 29;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 32;
    (h % 10_000) as f64 / 10_000.0
}

/// A textured panorama scene, 240x80.
fn texture(x: u32, y: u32) -> f64 {
    let (cx, cy) = (i64::from(x / 5), i64::from(y / 5));
    0.05 + 0.6 * cell(cx, cy) + 0.15 * (f64::from(x) / 9.0).sin().abs()
}

struct Workspace(PathBuf);

impl Workspace {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "pentool-merge-{tag}-{}-{}",
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
        work
    }

    fn path(&self) -> PathBuf {
        self.0.join("catalog.pen")
    }

    fn add(&self, id: &str, bytes: Vec<u8>) {
        let file = format!("{id}.dng");
        std::fs::write(self.0.join(&file), bytes).unwrap();
        let (ok, _, error) = self.run(&["raw", "add", "catalog.pen", id, "--file", &file]);
        assert!(ok, "{error}");
    }

    /// Three brackets at 1/4, 1 and 4 times the exposure; `edit` changes
    /// each bracket's radiance (index, x, y, radiance).
    fn brackets(&self, edit: impl Fn(usize, f64, f64, f64) -> f64) {
        for (index, exposure) in [0.25, 1.0, 4.0].into_iter().enumerate() {
            self.add(
                &format!("b{}", index + 1),
                dng(W, H, |x, y| {
                    let (x, y) = (f64::from(x), f64::from(y));
                    edit(index, x, y, radiance(x, y)) * exposure * BASE
                }),
            );
        }
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

    fn merge(&self, args: &[&str]) -> Value {
        let mut command = vec!["photo"];
        command.extend(args);
        let (ok, out, error) = self.run(&command);
        assert!(ok, "{command:?}: {error}");
        out["result"].clone()
    }

    fn refuse(&self, args: &[&str], code: &str) -> String {
        let before = std::fs::read(self.path()).unwrap();
        let mut command = vec!["photo"];
        command.extend(args);
        let (ok, _, error) = self.run(&command);
        assert!(!ok, "{command:?} should fail");
        assert!(error.contains(code), "{command:?}: {error}");
        assert_eq!(
            std::fs::read(self.path()).unwrap(),
            before,
            "{command:?} changed the document"
        );
        error
    }

    fn render(&self, id: &str) -> pentool::photo::pixels::Working {
        let raw = read(&self.path());
        pentool::photo::catalog::render_photo(&raw, &self.path(), id, "master")
            .unwrap()
            .image
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

fn luma(image: &pentool::photo::pixels::Working, x: u32, y: u32) -> f64 {
    let i = ((y * image.width + x) * 3) as usize;
    f64::from(image.rgb[i] + image.rgb[i + 1] + image.rgb[i + 2])
}

/// `luma / radiance` over the interior, sorted.
fn ratios(image: &pentool::photo::pixels::Working, region: impl Fn(u32, u32) -> bool) -> Vec<f64> {
    let mut out = Vec::new();
    for y in 4..image.height - 4 {
        for x in 4..image.width - 4 {
            if region(x, y) {
                out.push(luma(image, x, y) / radiance(f64::from(x), f64::from(y)));
            }
        }
    }
    out.sort_by(f64::total_cmp);
    out
}

fn median(values: &[f64]) -> f64 {
    values[values.len() / 2]
}

fn asset<'a>(raw: &'a Value, photo: &str) -> &'a Value {
    let photos = raw["photography"]["photos"].as_array().unwrap();
    let source = photos.iter().find(|p| p["id"] == photo).unwrap()["source"]
        .as_str()
        .unwrap();
    &raw["photography"]["assets"][source]
}

#[test]
fn hdr_merges_brackets_into_scene_linear_radiance() {
    let work = Workspace::new("hdr");
    work.brackets(|_, _, _, r| r);
    let result = work.merge(&["merge-hdr", "catalog.pen", "b1", "b2", "b3", "--id", "hdr"]);
    assert_eq!(result["operation"], "merge-hdr");
    assert_eq!(result["size"], json!([W, H]));
    assert_eq!(result["transparent"], false);
    let alignment = &result["alignment"];
    assert_eq!(alignment["reference"], 2);
    for (index, expected) in [0.25, 1.0, 4.0].into_iter().enumerate() {
        let input = &alignment["inputs"][index];
        assert_eq!(input["shift"], json!([0, 0]), "{input}");
        let exposure = input["exposure"].as_f64().unwrap();
        assert!((exposure / expected - 1.0).abs() < 0.02, "{input}");
        let source = if index == 1 { "reference" } else { "measured" };
        assert_eq!(input["exposure_source"], source);
    }

    // The derived source records attribution and facts that match its bytes.
    let raw = read(&work.path());
    let record = asset(&raw, "hdr");
    assert_eq!(record["kind"], "derived");
    assert_eq!(record["derived"]["operation"], "merge-hdr");
    assert_eq!(record["derived"]["algorithm"], 1);
    let inputs: Vec<&Value> = ["b1", "b2", "b3"]
        .iter()
        .map(|id| {
            let photos = raw["photography"]["photos"].as_array().unwrap();
            &photos.iter().find(|p| p["id"] == *id).unwrap()["source"]
        })
        .collect();
    assert_eq!(record["derived"]["inputs"], json!(inputs));
    assert_eq!(
        record["derived"]["settings"],
        json!({"deghost": "off", "reference": "auto", "scale": 1})
    );
    assert_eq!(record["storage"]["kind"], "embedded");
    // Every input remains untouched.
    for id in ["b1", "b2", "b3"] {
        assert_eq!(asset(&raw, id)["kind"], "raw");
    }

    // The reference clips its highlights; the merge follows radiance everywhere.
    // Raw 65535 is radiance 2.6 in the reference.
    let clipped = |x: u32, y: u32| radiance(f64::from(x), f64::from(y)) > 3.0;
    let reference = work.render("b2");
    let merged = work.render("hdr");
    let (reference_mid, merged_mid) = (
        median(&ratios(&reference, |x, y| !clipped(x, y))),
        median(&ratios(&merged, |x, y| !clipped(x, y))),
    );
    let reference_bright = median(&ratios(&reference, clipped)) / reference_mid;
    let merged_bright = median(&ratios(&merged, clipped)) / merged_mid;
    assert!(
        reference_bright < 0.9,
        "the reference should clip: {reference_bright}"
    );
    assert!(
        (merged_bright - 1.0).abs() < 0.05,
        "merged highlights {merged_bright}"
    );
    let all = ratios(&merged, |_, _| true);
    let spread = all[all.len() * 99 / 100] / all[all.len() / 100];
    assert!(spread < 1.12, "merged spread {spread}");
    // Exposure is normalized to the reference.
    let scale = merged_mid / reference_mid;
    assert!((scale - 1.0).abs() < 0.05, "merged/reference {scale}");

    // Tampered facts are refused.
    let mut tampered = raw.clone();
    let source = tampered["photography"]["photos"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "hdr")
        .unwrap()["source"]
        .as_str()
        .unwrap()
        .to_owned();
    tampered["photography"]["assets"][&source]["pixel_width"] = json!(W + 1);
    assert!(pentool::scene::validate(&tampered).is_err());
    let mut tampered = raw.clone();
    tampered["photography"]["assets"][&source]["derived"]["operation"] = json!("merge-focus");
    assert!(pentool::scene::validate(&tampered).is_err());
}

#[test]
fn hdr_aligns_a_shifted_bracket() {
    let work = Workspace::new("shift");
    // The bright bracket moved: its pixel p + (3, -2) sees the reference's p.
    work.brackets(|index, x, y, r| {
        if index == 2 {
            radiance(x - 3.0, y + 2.0)
        } else {
            r
        }
    });
    let result = work.merge(&["merge-hdr", "catalog.pen", "b1", "b2", "b3", "--id", "hdr"]);
    let inputs = &result["alignment"]["inputs"];
    assert_eq!(inputs[0]["shift"], json!([0, 0]));
    assert_eq!(inputs[2]["shift"], json!([3, -2]));
    let merged = ratios(&work.render("hdr"), |_, _| true);
    assert!(merged[merged.len() * 99 / 100] / merged[merged.len() / 100] < 1.12);
}

#[test]
fn deghosting_rejects_moving_content() {
    let work = Workspace::new("ghost");
    let block = |x: f64, y: f64| (40.0..56.0).contains(&x) && (24.0..40.0).contains(&y);
    // Something bright crossed the dark bracket.
    work.brackets(|index, x, y, r| {
        if index == 0 && block(x, y) {
            r * 4.0
        } else {
            r
        }
    });
    let in_block = |x: u32, y: u32| block(f64::from(x), f64::from(y));
    let outside = |x: u32, y: u32| !in_block(x, y);
    let error = |id: &str| {
        let image = work.render(id);
        let scale = median(&ratios(&image, outside));
        let inside = ratios(&image, in_block);
        inside.iter().map(|r| (r / scale - 1.0).abs()).sum::<f64>() / inside.len() as f64
    };
    work.merge(&[
        "merge-hdr",
        "catalog.pen",
        "b1",
        "b2",
        "b3",
        "--id",
        "ghosted",
    ]);
    let result = work.merge(&[
        "merge-hdr",
        "catalog.pen",
        "b1",
        "b2",
        "b3",
        "--id",
        "clean",
        "--deghost",
        "high",
    ]);
    assert_eq!(result["settings"]["deghost"], "high");
    assert!(
        result["alignment"]["inputs"][0]["ghost_pixels"]
            .as_u64()
            .unwrap()
            >= 200
    );
    let (ghosted, clean) = (error("ghosted"), error("clean"));
    assert!(
        ghosted > 0.2,
        "without deghosting the ghost shows: {ghosted}"
    );
    assert!(clean < 0.06, "deghosting removes it: {clean}");
}

/// Three 112x72 frames of the 240x80 texture, 64 pixels apart and 4 lower each.
fn frames(work: &Workspace) {
    for index in 0..3u32 {
        let (left, top) = (index * 64, index * 4);
        work.add(
            &format!("f{}", index + 1),
            dng(112, 72, |x, y| texture(x + left, y + top) * 40_000.0),
        );
    }
}

#[test]
fn panoramas_stitch_frames_with_transparent_outside_pixels() {
    let work = Workspace::new("pano");
    frames(&work);
    let args = [
        "merge-pano",
        "catalog.pen",
        "f1",
        "f2",
        "f3",
        "--id",
        "pano",
        "--focal",
        "100000",
    ];
    let result = work.merge(&args);
    let alignment = &result["alignment"];
    assert_eq!(alignment["projection"], "cylindrical");
    assert_eq!(alignment["reference"], 2);
    assert_eq!(alignment["focal_source"], "option");
    let translate = |i: usize| -> [f64; 2] {
        let t = &alignment["inputs"][i]["translate"];
        [t[0].as_f64().unwrap(), t[1].as_f64().unwrap()]
    };
    for (i, expected) in [(0, [-64.0, -4.0]), (1, [0.0, 0.0]), (2, [64.0, 4.0])] {
        let t = translate(i);
        assert!(
            (t[0] - expected[0]).abs() < 0.5 && (t[1] - expected[1]).abs() < 0.5,
            "{i}: {t:?}"
        );
    }
    let size = &result["size"];
    assert!((238..=242).contains(&size[0].as_u64().unwrap()), "{size}");
    assert!((78..=82).contains(&size[1].as_u64().unwrap()), "{size}");
    assert_eq!(result["transparent"], true);
    assert_eq!(
        result["settings"],
        json!({"projection": "cylindrical", "seed": 0, "scale": 1, "focal": 100000})
    );

    // The stitched image matches each frame where it is the only source, and
    // the canvas outside every frame is transparent.
    let pano = work.render("pano");
    let middle = work.render("f2");
    let alpha = pano.alpha.as_ref().expect("a panorama with gaps has alpha");
    assert_eq!(alpha[(pano.height as usize - 1) * pano.width as usize], 0.0);
    assert_eq!(alpha[pano.width as usize - 1], 0.0);
    let (mut error, mut count) = (0.0, 0);
    for y in 10..60 {
        for x in 30..80 {
            let a = luma(&middle, x, y);
            let b = luma(&pano, x + 64, y + 4);
            error += (b / a - 1.0).abs();
            count += 1;
        }
    }
    assert!(
        error / f64::from(count) < 0.03,
        "mean relative error {}",
        error / f64::from(count)
    );
    assert!(alpha[(44 * pano.width + 120) as usize] > 0.99);

    // The same inputs and seed give the same bytes.
    let again = work.merge(&[
        "merge-pano",
        "catalog.pen",
        "f1",
        "f2",
        "f3",
        "--id",
        "pano-2",
        "--focal",
        "100000",
    ]);
    assert_eq!(again["asset"], result["asset"]);
    assert_eq!(again["deduplicated"], true);

    // Perspective places the frames with homographies.
    let perspective = work.merge(&[
        "merge-pano",
        "catalog.pen",
        "f1",
        "f2",
        "f3",
        "--id",
        "flat",
        "--projection",
        "perspective",
    ]);
    // Matches lie in the narrow overlap, so check where it maps overlap points.
    let h: Vec<f64> = perspective["alignment"]["inputs"][2]["homography"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap())
        .collect();
    for (x, y) in [(-40.0, -20.0), (-24.0, 0.0), (-40.0, 20.0)] {
        let w = h[6] * x + h[7] * y + h[8];
        let (u, v) = (
            (h[0] * x + h[1] * y + h[2]) / w,
            (h[3] * x + h[4] * y + h[5]) / w,
        );
        assert!(
            (u - x - 64.0).abs() < 1.0 && (v - y - 4.0).abs() < 1.0,
            "{h:?}: ({u}, {v})"
        );
    }
}

#[test]
fn merges_refuse_bad_requests_without_changing_the_document() {
    let work = Workspace::new("refuse");
    work.brackets(|_, _, _, r| r);
    work.add("odd", dng(112, 72, |x, y| texture(x, y) * 30_000.0));
    let hdr = |extra: &[&'static str]| {
        let mut args = vec!["merge-hdr", "catalog.pen", "b1", "b2", "b3", "--id", "hdr"];
        args.extend_from_slice(extra);
        args
    };
    work.refuse(
        &["merge-hdr", "catalog.pen", "b1", "b1", "--id", "hdr"],
        "given twice",
    );
    work.refuse(
        &["merge-hdr", "catalog.pen", "b1", "nope", "--id", "hdr"],
        "[missing-resource]",
    );
    work.refuse(
        &["merge-hdr", "catalog.pen", "b1", "b2", "--id", "b3"],
        "already used",
    );
    work.refuse(
        &["merge-hdr", "catalog.pen", "b1", "odd", "--id", "hdr"],
        "share one frame size",
    );
    work.refuse(&hdr(&["--reference", "4"]), "--reference");
    work.refuse(&hdr(&["--scale", "0"]), "--scale");
    work.refuse(&hdr(&["--deghost", "max"]), "--deghost");
    work.refuse(&hdr(&["--settings", "last"]), "--settings");
    work.refuse(&hdr(&["--external", "../out.dng"]), "[unsafe-path]");
    work.refuse(&hdr(&["--if-revision", "0000"]), "revision mismatch");
    work.refuse(
        &hdr(&["--external", "merged/hdr.dng", "--if-revision", "0000"]),
        "revision mismatch",
    );
    assert!(
        !work.0.join("merged").join("hdr.dng").exists(),
        "a failed merge leaves no file"
    );
    work.refuse(&hdr(&["--external", "b1.dng"]), "already exists");
    work.refuse(
        &[
            "merge-pano",
            "catalog.pen",
            "b1",
            "odd",
            "--id",
            "p",
            "--projection",
            "fisheye",
        ],
        "--projection",
    );
    let (ok, out, error) = work.run(&{
        let mut args = vec!["photo"];
        args.extend(hdr(&["--dry-run"]));
        args
    });
    assert!(ok, "{error}");
    assert_eq!(out["result"]["photo"], "hdr");
    assert!(read(&work.path())["photography"]["photos"]
        .as_array()
        .unwrap()
        .iter()
        .all(|p| p["id"] != "hdr"));
}

#[test]
fn external_output_and_first_input_settings() {
    let work = Workspace::new("external");
    work.brackets(|_, _, _, r| r);
    let (ok, _, error) = work.run(&["raw", "develop", "catalog.pen", "b1", "--exposure", "0.5"]);
    assert!(ok, "{error}");
    let result = work.merge(&[
        "merge-hdr",
        "catalog.pen",
        "b1",
        "b2",
        "b3",
        "--id",
        "hdr",
        "--external",
        "merged/hdr.dng",
        "--settings",
        "first",
        "--scale",
        "0.5",
    ]);
    assert_eq!(result["storage"], "external");
    assert_eq!(result["size"], json!([W / 2, H / 2]));
    assert!(result["settings_copied"]
        .as_array()
        .unwrap()
        .contains(&json!("tone")));
    let file = work.0.join("merged").join("hdr.dng");
    let bytes = std::fs::read(&file).unwrap();
    assert_eq!(result["asset"], json!(pentool::resource::sha256(&bytes)));
    let raw = read(&work.path());
    let record = asset(&raw, "hdr");
    assert_eq!(
        record["storage"],
        json!({"kind": "external", "path": "merged/hdr.dng"})
    );
    let photo = raw["photography"]["photos"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "hdr")
        .unwrap();
    let develop = &photo["variants"][0]["develop"];
    assert_eq!(develop["tone"]["exposure"], 0.5);
    assert!(develop.get("geometry").is_none() && develop.get("lens").is_some());
    let image = work.render("hdr");
    assert_eq!((image.width, image.height), (W / 2, H / 2));
}
