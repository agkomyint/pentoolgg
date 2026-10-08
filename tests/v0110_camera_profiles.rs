//! v0.11.0 item 4: camera profiles and white balance (`photo profile add`,
//! `raw develop`).
use serde_json::{json, Value};
use std::path::Path;

/// A TIFF field value.
enum Field {
    Byte(Vec<u8>),
    Ascii(&'static str),
    Short(Vec<u16>),
    Long(Vec<u32>),
    Srational(Vec<f64>),
    Float(Vec<f32>),
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
            Field::Float(v) => (
                11,
                v.len() as u32,
                v.iter().flat_map(|x| x.to_le_bytes()).collect(),
            ),
        }
    }
}

/// A little-endian TIFF-structured file with one IFD. `pixels` sit at offset 8.
fn tiff(magic: &[u8; 4], mut entries: Vec<(u16, Field)>, pixels: &[u8]) -> Vec<u8> {
    entries.sort_by_key(|(tag, _)| *tag);
    let mut out = magic.to_vec();
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

const MODEL: &str = "Pentool Profile Test Body";
const COLOR_A: [f64; 9] = [
    0.6455, -0.0938, -0.0832, -0.4688, 1.2401, 0.2552, -0.1067, 0.2142, 0.6648,
];
const COLOR_D65: [f64; 9] = [
    0.6461, -0.0907, -0.0882, -0.4300, 1.2184, 0.2378, -0.0819, 0.1944, 0.5931,
];
const FORWARD: [f64; 9] = [
    0.7976, 0.1352, 0.0313, 0.2880, 0.7119, 0.0001, 0.0000, 0.0000, 0.8251,
];

/// An 8x8 LinearRaw DNG filled with one camera-RGB neutral, (0.5, 1, 0.75).
fn dng(samples: u16) -> Vec<u8> {
    let (width, height) = (8u32, 8u32);
    let pixel: Vec<u16> = if samples == 3 {
        vec![20_000, 40_000, 30_000]
    } else {
        vec![40_000]
    };
    let pixels: Vec<u8> = (0..width * height)
        .flat_map(|_| pixel.iter().flat_map(|v| v.to_le_bytes()))
        .collect();
    let mut entries = vec![
        (254, Field::Long(vec![0])),
        (256, Field::Long(vec![width])),
        (257, Field::Long(vec![height])),
        (258, Field::Short(vec![16; samples as usize])),
        (259, Field::Short(vec![1])),
        (262, Field::Short(vec![34892])),
        (273, Field::Long(vec![8])),
        (277, Field::Short(vec![samples])),
        (278, Field::Long(vec![height])),
        (279, Field::Long(vec![pixels.len() as u32])),
        (50706, Field::Byte(vec![1, 4, 0, 0])),
        (50708, Field::Ascii(MODEL)),
    ];
    if samples == 3 {
        entries.extend([
            (50721, Field::Srational(COLOR_A.to_vec())),
            (50722, Field::Srational(COLOR_D65.to_vec())),
            (50778, Field::Short(vec![17])),
            (50779, Field::Short(vec![21])),
            (50728, Field::Srational(vec![0.5, 1.0, 0.75])),
        ]);
    } else {
        entries.push((50721, Field::Srational(vec![0.3, 1.0, 0.8])));
    }
    tiff(b"II*\0", entries, &pixels)
}

/// A dual-illuminant `.dcp` with forward matrices and a small hue/sat map.
fn dcp(model: Option<&'static str>, name: &'static str) -> Vec<u8> {
    let map: Vec<f32> = (0..4).flat_map(|i| [2.0 * i as f32, 1.0, 1.0]).collect();
    let mut entries = vec![
        (50721, Field::Srational(COLOR_A.to_vec())),
        (50722, Field::Srational(COLOR_D65.to_vec())),
        (50778, Field::Short(vec![17])),
        (50779, Field::Short(vec![21])),
        (50964, Field::Srational(FORWARD.to_vec())),
        (50965, Field::Srational(FORWARD.to_vec())),
        (50936, Field::Ascii(name)),
        (50937, Field::Long(vec![2, 2, 1])),
        (50938, Field::Float(map.clone())),
        (50939, Field::Float(map)),
        (50941, Field::Long(vec![0])),
    ];
    if let Some(model) = model {
        entries.push((50708, Field::Ascii(model)));
    }
    tiff(b"IIRC", entries, &[])
}

struct Workspace(std::path::PathBuf);

impl Workspace {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "pentool-profile-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        Workspace(root)
    }

    /// A catalog holding photo `hero` with the given DNG.
    fn catalog(&self, dng: &[u8]) -> std::path::PathBuf {
        let path = self.0.join("catalog.pen");
        let raw = pentool::scene::new_document(40, 30);
        std::fs::write(&path, serde_json::to_vec_pretty(&raw).unwrap()).unwrap();
        std::fs::write(self.0.join("capture.dng"), dng).unwrap();
        let (ok, _, error) =
            self.run(&["raw", "add", "catalog.pen", "hero", "--file", "capture.dng"]);
        assert!(ok, "{error}");
        path
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
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn read(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn master(path: &Path) -> Value {
    read(path)["photography"]["photos"][0]["variants"][0]["develop"].clone()
}

fn close(value: &Value, expected: f64, tolerance: f64) -> bool {
    value
        .as_f64()
        .is_some_and(|v| (v - expected).abs() <= tolerance)
}

#[test]
fn white_balance_modes_store_and_resolve() {
    let work = Workspace::new("wb");
    let document = work.catalog(&dng(3));

    let as_shot = work.develop(&["--as-shot"]);
    assert_eq!(as_shot["white_balance"], json!({"mode": "as-shot"}));
    assert_eq!(as_shot["monochrome"], false);
    assert_eq!(as_shot["camera_profile"], "embedded");
    let neutral = &as_shot["resolved"]["neutral"];
    for (value, expected) in neutral.as_array().unwrap().iter().zip([0.5, 1.0, 0.75]) {
        assert!(close(value, expected, 1e-6), "{neutral}");
    }
    let shot_temperature = as_shot["resolved"]["temperature"].as_f64().unwrap();
    let shot_tint = as_shot["resolved"]["tint"].as_f64().unwrap();
    assert!((2000.0..=50_000.0).contains(&shot_temperature), "{as_shot}");

    // Kelvin and tint round-trip through the neutral search.
    let kelvin = work.develop(&["--temperature", "5000", "--tint", "10"]);
    assert_eq!(
        master(&document)["white_balance"],
        json!({"mode": "temperature", "temperature": 5000, "tint": 10})
    );
    assert!(
        close(&kelvin["resolved"]["temperature"], 5000.0, 1.0),
        "{kelvin}"
    );
    assert!(close(&kelvin["resolved"]["tint"], 10.0, 0.2), "{kelvin}");
    let warmer = work.develop(&["--temperature", "3000"]);
    assert!(
        close(&warmer["resolved"]["temperature"], 3000.0, 1.0),
        "{warmer}"
    );
    assert!(close(&warmer["resolved"]["tint"], 0.0, 0.2), "{warmer}");

    // A sampled gray patch recovers the capture neutral and records where.
    let sampled = work.develop(&["--sample", "0.5,0.5,0.1"]);
    assert_eq!(
        master(&document)["white_balance"],
        json!({"mode": "neutral", "neutral": [0.5, 1, 0.75],
               "sampled": {"x": 0.5, "y": 0.5, "radius": 0.1}})
    );
    assert!(close(
        &sampled["resolved"]["temperature"],
        shot_temperature,
        1.0
    ));
    assert!(close(&sampled["resolved"]["tint"], shot_tint, 0.2));

    let explicit = work.develop(&["--neutral", "0.5,1,0.75"]);
    assert_eq!(explicit["resolved"], sampled["resolved"]);
    assert_eq!(
        master(&document)["white_balance"],
        json!({"mode": "neutral", "neutral": [0.5, 1, 0.75]})
    );

    // Suggest stores the gray-world result as kelvin with its provenance.
    let suggested = work.develop(&["--suggest"]);
    let stored = master(&document)["white_balance"].clone();
    assert_eq!(stored["mode"], "temperature");
    assert_eq!(
        stored["auto"],
        json!({"algorithm": "gray-world", "version": 1})
    );
    assert!(
        close(&stored["temperature"], shot_temperature, 1.0),
        "{stored}"
    );
    assert!(close(&stored["tint"], shot_tint, 1.0), "{stored}");
    assert_eq!(suggested["white_balance"], stored);
    // Deterministic: suggesting again changes nothing.
    let before = std::fs::read(&document).unwrap();
    let again = work.develop(&["--suggest", "--dry-run"]);
    assert_eq!(again["white_balance"], stored);
    assert_eq!(std::fs::read(&document).unwrap(), before);

    // Matrix-only keeps the white, and the choice is stored.
    let matrix = work.develop(&["--camera-profile", "matrix-only", "--as-shot"]);
    assert_eq!(matrix["camera_profile"], "matrix-only");
    assert_eq!(master(&document)["raw"]["camera_profile"], "matrix-only");
    assert_eq!(
        matrix["resolved"]["neutral"],
        as_shot["resolved"]["neutral"]
    );
}

#[test]
fn raw_develop_refuses_bad_settings_without_mutation() {
    let work = Workspace::new("refuse");
    let document = work.catalog(&dng(3));
    let unknown = format!("sha256:{}", "0".repeat(64));
    let before = std::fs::read(&document).unwrap();
    for (args, code) in [
        (vec!["--temperature", "1000"], "[invalid-develop]"),
        (
            vec!["--temperature", "5000", "--tint", "200"],
            "[invalid-develop]",
        ),
        (vec!["--neutral", "1,0,1"], "[invalid-develop]"),
        (vec!["--neutral", "1,2"], "[invalid-input]"),
        (vec!["--sample", "2,0.5,0.1"], "[invalid-input]"),
        (vec!["--sample", "0.5,0.5,0"], "[invalid-input]"),
        (vec!["--camera-profile", "neutral"], "[invalid-develop]"),
        (vec!["--camera-profile", &unknown], "[missing-resource]"),
        (vec!["--variant", "nope", "--as-shot"], "[missing-resource]"),
        (vec![], "[invalid-input]"),
        (vec!["--as-shot", "--if-revision", "stale"], ""),
    ] {
        let mut command = vec!["raw", "develop", "catalog.pen", "hero"];
        command.extend(args.iter().copied());
        let (ok, _, error) = work.run(&command);
        assert!(!ok, "{command:?} succeeded");
        assert!(error.contains(code), "{command:?}: {error}");
        assert_eq!(std::fs::read(&document).unwrap(), before, "{command:?}");
    }
    let (ok, _, _) = work.run(&[
        "raw",
        "develop",
        "catalog.pen",
        "hero",
        "--as-shot",
        "--suggest",
    ]);
    assert!(!ok, "white balance flags are exclusive");
    assert_eq!(std::fs::read(&document).unwrap(), before);
}

#[test]
fn camera_profiles_are_verified_stored_and_selected() {
    let work = Workspace::new("dcp");
    let document = work.catalog(&dng(3));
    std::fs::write(work.0.join("studio.dcp"), dcp(Some(MODEL), "Studio")).unwrap();
    std::fs::write(work.0.join("other.dcp"), dcp(Some("Another Body"), "Other")).unwrap();
    std::fs::write(work.0.join("anonymous.dcp"), dcp(None, "Anonymous")).unwrap();
    std::fs::write(work.0.join("capture.tif"), &dng(3)[..]).unwrap();

    let before = std::fs::read(&document).unwrap();
    let add = |file: &str, extra: &[&str]| {
        let mut command = vec!["photo", "profile", "add", "catalog.pen", "--file", file];
        command.extend(extra);
        work.run(&command)
    };
    let (ok, dry, error) = add("studio.dcp", &["--dry-run"]);
    assert!(ok, "{error}");
    assert_eq!(std::fs::read(&document).unwrap(), before);
    let (ok, added, error) = add("studio.dcp", &[]);
    assert!(ok, "{error}");
    assert_eq!(added["result"], dry["result"]);
    let result = &added["result"];
    assert_eq!(result["name"], "Studio");
    assert_eq!(result["unique_camera_model"], MODEL);
    assert_eq!(result["calibrations"], 2);
    assert_eq!(result["hue_sat_map"], true);
    assert_eq!(result["deduplicated"], false);
    assert_eq!(result["upgraded"], false);
    assert_eq!(result["force_model"], false);
    let digest = result["profile"].as_str().unwrap().to_string();
    let record = &read(&document)["photography"]["profiles"][&digest];
    assert_eq!(record["kind"], "camera");
    assert_eq!(record["imported_from"], "dcp");
    assert_eq!(record["embed_policy"], 0);
    assert_eq!(record["storage"]["kind"], "embedded");
    assert_eq!(record["byte_length"], dcp(Some(MODEL), "Studio").len());

    let (ok, again, error) = add("studio.dcp", &[]);
    assert!(ok, "{error}");
    assert_eq!(again["result"]["deduplicated"], true);
    assert_eq!(again["result"]["profile"], digest.as_str());

    // Malformed profiles are refused without mutation.
    let before = std::fs::read(&document).unwrap();
    for (file, code) in [
        ("anonymous.dcp", "[malformed-resource]"),
        ("capture.tif", "[malformed-resource]"),
        ("missing.dcp", "[missing-resource]"),
    ] {
        let (ok, _, error) = add(file, &[]);
        assert!(!ok, "{file} accepted");
        assert!(error.contains(code), "{file}: {error}");
        assert_eq!(std::fs::read(&document).unwrap(), before, "{file}");
    }

    // A matching profile is selected; the as-shot white still resolves.
    let developed = work.develop(&["--camera-profile", &digest, "--as-shot"]);
    assert_eq!(developed["camera_profile"], json!({"profile": digest}));
    assert_eq!(
        master(&document)["raw"]["camera_profile"],
        json!({"profile": digest})
    );
    let neutral = &developed["resolved"]["neutral"];
    for (value, expected) in neutral.as_array().unwrap().iter().zip([0.5, 1.0, 0.75]) {
        assert!(close(value, expected, 1e-6), "{neutral}");
    }

    // A profile for another camera needs --force-model, which is recorded.
    let (ok, other, error) = add("other.dcp", &[]);
    assert!(ok, "{error}");
    let other = other["result"]["profile"].as_str().unwrap().to_string();
    let before = std::fs::read(&document).unwrap();
    let (ok, _, error) = work.run(&[
        "raw",
        "develop",
        "catalog.pen",
        "hero",
        "--camera-profile",
        &other,
    ]);
    assert!(
        !ok && error.contains("[invalid-develop]") && error.contains("--force-model"),
        "{error}"
    );
    assert_eq!(std::fs::read(&document).unwrap(), before);
    let (ok, forced, error) = add("other.dcp", &["--force-model"]);
    assert!(ok, "{error}");
    assert_eq!(forced["result"]["deduplicated"], true);
    assert_eq!(forced["result"]["force_model"], true);
    assert_eq!(
        read(&document)["photography"]["profiles"][&other]["force_model"],
        true
    );
    let developed = work.develop(&["--camera-profile", &other]);
    assert_eq!(developed["camera_profile"], json!({"profile": other}));
}

#[test]
fn profile_add_upgrades_a_v4_document() {
    let work = Workspace::new("upgrade");
    let path = work.0.join("catalog.pen");
    let raw = pentool::scene::new_document(40, 30);
    std::fs::write(&path, serde_json::to_vec_pretty(&raw).unwrap()).unwrap();
    std::fs::write(work.0.join("studio.dcp"), dcp(Some(MODEL), "Studio")).unwrap();
    let (ok, added, error) = work.run(&[
        "photo",
        "profile",
        "add",
        "catalog.pen",
        "--file",
        "studio.dcp",
    ]);
    assert!(ok, "{error}");
    assert_eq!(added["result"]["upgraded"], true);
    let document = read(&path);
    assert_eq!(document["version"], 7);
    assert_eq!(document["photography"]["photos"], json!([]));
}

#[test]
fn monochrome_sources_refuse_measured_white_balance() {
    let work = Workspace::new("mono");
    let document = work.catalog(&dng(1));
    let before = std::fs::read(&document).unwrap();
    for args in [
        vec!["--suggest"],
        vec!["--sample", "0.5,0.5,0.1"],
        vec!["--neutral", "1,1,1"],
    ] {
        let mut command = vec!["raw", "develop", "catalog.pen", "hero"];
        command.extend(args.iter().copied());
        let (ok, _, error) = work.run(&command);
        assert!(
            !ok && error.contains("[invalid-develop]"),
            "{command:?}: {error}"
        );
        assert_eq!(std::fs::read(&document).unwrap(), before);
    }
    let developed = work.develop(&["--as-shot"]);
    assert_eq!(developed["monochrome"], true);
}
