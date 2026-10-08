//! v0.11.0 item 10: wide-gamut and HDR delivery. Stage 11 (profile look and
//! tone curve, shoulder, gamut mapping), tagged PNG output (`iCCP`, `cICP`,
//! `mDCV`, `cLLI`) and `photo inspect`.
use pentool::photo::png;
use serde_json::{json, Value};

/// A TIFF field value.
enum Field {
    Ascii(&'static str),
    Byte(Vec<u8>),
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

const WIDTH: u32 = 64;
const HEIGHT: u32 = 16;

/// A 64x16 LinearRaw DNG: a horizontal ramp from black to sensor white;
/// `tone_curve` embeds a `ProfileToneCurve`.
fn dng(tone_curve: bool) -> Vec<u8> {
    let mut pixels = Vec::new();
    for _ in 0..HEIGHT {
        for x in 0..WIDTH {
            let level = (x * 65_535 / (WIDTH - 1)) as u16;
            let pixel = [level / 2, level, (u32::from(level) * 3 / 4) as u16];
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
        (50708, Field::Ascii("Pentool Delivery Test Body")),
        (
            50721,
            Field::Srational(vec![
                0.6461, -0.0907, -0.0882, -0.4300, 1.2184, 0.2378, -0.0819, 0.1944, 0.5931,
            ]),
        ),
        (50778, Field::Short(vec![21])),
        (50728, Field::Srational(vec![0.5, 1.0, 0.75])),
    ];
    if tone_curve {
        entries.push((
            50940,
            Field::Float(vec![0.0, 0.0, 0.1, 0.05, 0.5, 0.6, 1.0, 1.0]),
        ));
    }
    tiff(entries, &pixels)
}

struct Workspace(std::path::PathBuf);

impl Workspace {
    fn new(tag: &str, dng: &[u8]) -> Self {
        let root = std::env::temp_dir().join(format!(
            "pentool-delivery-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let work = Workspace(root);
        let raw = pentool::scene::new_document(40, 30);
        std::fs::write(
            work.0.join("catalog.pen"),
            serde_json::to_vec_pretty(&raw).unwrap(),
        )
        .unwrap();
        std::fs::write(work.0.join("capture.dng"), dng).unwrap();
        let (ok, _, error) =
            work.run(&["raw", "add", "catalog.pen", "hero", "--file", "capture.dng"]);
        assert!(ok, "{error}");
        work
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

    /// `photo render` to `out` with extra options; the report and the file.
    fn render(&self, out: &str, options: &[&str]) -> (Value, Vec<u8>) {
        let mut command = vec!["photo", "render", "catalog.pen", "hero", "--out", out];
        command.extend(options);
        let (ok, report, error) = self.run(&command);
        assert!(ok, "{command:?}: {error}");
        (report, std::fs::read(self.0.join(out)).unwrap())
    }

    fn develop(&self, args: &[&str]) {
        let mut command = vec!["raw", "develop", "catalog.pen", "hero"];
        command.extend(args);
        let (ok, _, error) = self.run(&command);
        assert!(ok, "{command:?}: {error}");
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn chunk<'a>(chunks: &'a [png::Chunk], kind: &[u8; 4]) -> Option<&'a [u8]> {
    chunks
        .iter()
        .find(|(k, _)| k == kind)
        .map(|(_, data)| data.as_slice())
}

fn be32(bytes: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap())
}

#[test]
fn sdr_render_is_tagged_and_reports_its_delivery() {
    let work = Workspace::new("sdr", &dng(false));
    let (report, bytes) = work.render("hero.png", &[]);
    let delivery = &report["delivery"];
    assert_eq!(delivery["space"], "srgb");
    assert_eq!(delivery["transfer"], "srgb");
    assert_eq!(delivery["depth"], 8);
    assert_eq!(delivery["intent"], "perceptual");
    assert_eq!(delivery["hdr"], Value::Null);
    assert_eq!(delivery["cicp"], json!([1, 13, 0, 1]));
    assert_eq!(delivery["icc"], true);
    assert_eq!(delivery["pixels"], WIDTH * HEIGHT);
    assert_eq!(report["clipped_pixels"], delivery["clipped_pixels"]);
    for channel in ["r", "g", "b"] {
        let bins = delivery["histogram"][channel].as_array().unwrap();
        assert_eq!(bins.len(), 64);
        let total: u64 = bins.iter().map(|v| v.as_u64().unwrap()).sum();
        assert_eq!(total, u64::from(WIDTH * HEIGHT));
    }
    let outside = delivery["outside_gamut"].as_object().unwrap();
    assert_eq!(outside.len(), 5);
    let chunks = png::chunks(&bytes).unwrap();
    assert_eq!(chunk(&chunks, b"cICP"), Some(&[1u8, 13, 0, 1][..]));
    let iccp = chunk(&chunks, b"iCCP").unwrap();
    assert!(iccp.starts_with(b"pentool\0\0"));
    assert!(chunk(&chunks, b"cLLI").is_none() && chunk(&chunks, b"mDCV").is_none());
    // Deterministic bytes.
    let (_, again) = work.render("again.png", &[]);
    assert_eq!(again, bytes);

    // Wide gamut and linear variants.
    let (p3, bytes) = work.render(
        "p3.png",
        &[
            "--space",
            "display-p3",
            "--depth",
            "16",
            "--intent",
            "relative-colorimetric",
        ],
    );
    assert_eq!(p3["delivery"]["intent"], "relative-colorimetric");
    assert_eq!(p3["depth"], 16);
    let chunks = png::chunks(&bytes).unwrap();
    assert_eq!(chunk(&chunks, b"cICP"), Some(&[12u8, 13, 0, 1][..]));
    let (linear, _) = work.render("linear.png", &["--space", "srgb:linear"]);
    assert_eq!(linear["delivery"]["cicp"], json!([1, 8, 0, 1]));
    let (prophoto, bytes) = work.render("prophoto.png", &["--space", "prophoto"]);
    assert_eq!(prophoto["delivery"]["cicp"], Value::Null);
    assert!(chunk(&png::chunks(&bytes).unwrap(), b"iCCP").is_some());
}

#[test]
fn hdr_pq_and_hlg_carry_measured_light_levels() {
    let work = Workspace::new("hdr", &dng(false));
    work.develop(&["--exposure", "2"]);
    let (report, bytes) = work.render("pq.png", &["--hdr", "pq", "--headroom", "2"]);
    let delivery = &report["delivery"];
    assert_eq!(delivery["space"], "rec2020");
    assert_eq!(delivery["transfer"], "pq");
    assert_eq!(delivery["depth"], 16);
    assert_eq!(delivery["icc"], false);
    assert_eq!(delivery["hdr"]["peak_nits"], 812.0);
    assert_eq!(delivery["hdr"]["reference_white_nits"], 203.0);
    // IHDR: 16-bit RGB.
    assert_eq!(&bytes[24..26], &[16, 2]);
    let chunks = png::chunks(&bytes).unwrap();
    assert_eq!(chunk(&chunks, b"cICP"), Some(&[9u8, 16, 0, 1][..]));
    assert!(chunk(&chunks, b"iCCP").is_none());
    let cll = delivery["max_cll"].as_f64().unwrap();
    let fall = delivery["max_fall"].as_f64().unwrap();
    // The ramp is pushed two stops: highlights pass SDR white, under the peak.
    assert!(cll > 203.0 && cll < 812.0, "{delivery}");
    assert!(fall > 0.0 && fall <= cll);
    let clli = chunk(&chunks, b"cLLI").unwrap();
    assert!((f64::from(be32(clli, 0)) / 10_000.0 - cll).abs() < 1e-3);
    assert!((f64::from(be32(clli, 4)) / 10_000.0 - fall).abs() < 1e-3);
    let mdcv = chunk(&chunks, b"mDCV").unwrap();
    assert_eq!(mdcv.len(), 24);
    // Rec. 2020 red (0.708, 0.292) and D65 white in 0.00002 units.
    assert_eq!(&mdcv[..4], &[0x8a, 0x48, 0x39, 0x08]);
    assert_eq!(&mdcv[12..16], &[0x3d, 0x13, 0x40, 0x42]);
    assert_eq!(be32(mdcv, 16), 812 * 10_000);
    assert_eq!(be32(mdcv, 20), 1);

    let (hlg, bytes) = work.render("hlg.png", &["--hdr", "hlg", "--headroom", "1"]);
    assert_eq!(hlg["delivery"]["transfer"], "hlg");
    let chunks = png::chunks(&bytes).unwrap();
    assert_eq!(chunk(&chunks, b"cICP"), Some(&[9u8, 18, 0, 1][..]));
    assert_eq!(be32(chunk(&chunks, b"mDCV").unwrap(), 16), 1000 * 10_000);
}

#[test]
fn inspect_measures_without_writing_and_matches_render() {
    let work = Workspace::new("inspect", &dng(false));
    let options = ["--space", "rec2020", "--depth", "16"];
    let (rendered, _) = work.render("hero.png", &options);
    let mut command = vec!["photo", "inspect", "catalog.pen", "hero"];
    command.extend(options);
    let (ok, inspected, error) = work.run(&command);
    assert!(ok, "{error}");
    assert_eq!(inspected["delivery"], rendered["delivery"]);
    assert_eq!(inspected["width"], WIDTH);
    assert_eq!(inspected["height"], HEIGHT);
    let (ok, coarse, error) = work.run(&["photo", "inspect", "catalog.pen", "hero", "--bins", "4"]);
    assert!(ok, "{error}");
    assert_eq!(
        coarse["delivery"]["histogram"]["r"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    let files: Vec<_> = std::fs::read_dir(&work.0)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.ends_with(".png"))
        .collect();
    assert_eq!(files, ["hero.png"], "inspect writes nothing");
    let (ok, _, error) = work.run(&["photo", "inspect", "catalog.pen", "hero", "--bins", "0"]);
    assert!(!ok && error.contains("[invalid-input]"), "{error}");
}

#[test]
fn invalid_delivery_options_are_refused_before_writing() {
    let work = Workspace::new("refuse", &dng(false));
    for options in [
        vec!["--hdr", "pq", "--depth", "8"],
        vec!["--hdr", "pq", "--space", "srgb"],
        vec!["--hdr", "hlg", "--headroom", "3"],
        vec!["--hdr", "pq", "--headroom", "5"],
        vec!["--hdr", "dolby"],
        vec!["--headroom", "1"],
        vec!["--intent", "saturation"],
        vec!["--space", "cmyk"],
        vec!["--depth", "12"],
    ] {
        let mut command = vec!["photo", "render", "catalog.pen", "hero", "--out", "bad.png"];
        command.extend(&options);
        let (ok, _, error) = work.run(&command);
        assert!(!ok, "{options:?}");
        assert!(error.contains("[invalid-input]"), "{options:?}: {error}");
        assert!(!work.0.join("bad.png").exists(), "{options:?}");
    }
}

#[test]
fn profile_tone_curve_shapes_the_rendering_and_matrix_only_drops_it() {
    let plain = Workspace::new("plain", &dng(false));
    let (_, without) = plain.render("hero.png", &["--depth", "16"]);
    let curved = Workspace::new("curved", &dng(true));
    let (_, with) = curved.render("hero.png", &["--depth", "16"]);
    assert_ne!(with, without, "the embedded tone curve applies in stage 11");
    let decode = |bytes: &[u8]| png::read(bytes).unwrap().raster;
    curved.develop(&["--camera-profile", "matrix-only"]);
    let (_, matrix) = curved.render("matrix.png", &["--depth", "16"]);
    plain.develop(&["--camera-profile", "matrix-only"]);
    let (_, plain_matrix) = plain.render("matrix.png", &["--depth", "16"]);
    assert_eq!(decode(&matrix), decode(&plain_matrix));
}
