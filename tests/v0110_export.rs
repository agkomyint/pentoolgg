//! v0.11.0 item 14: output recipes and batch export.
use serde_json::{json, Value};

/// A TIFF field value.
enum Field {
    Ascii(&'static str),
    Byte(Vec<u8>),
    Short(Vec<u16>),
    Long(Vec<u32>),
    Srational(Vec<f64>),
    Rational(Vec<[u32; 2]>),
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
            Field::Rational(v) => (
                5,
                v.len() as u32,
                v.iter().flatten().flat_map(|x| x.to_le_bytes()).collect(),
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
fn tiff(entries: Vec<(u16, Field)>, pixels: &[u8]) -> Vec<u8> {
    let mut out = b"II*\0".to_vec();
    let mut ifd = 8 + pixels.len();
    ifd += ifd % 2;
    out.extend((ifd as u32).to_le_bytes());
    out.extend(pixels);
    out.resize(ifd, 0);
    ifd_into(&mut out, entries);
    out
}

/// Append an IFD (table then data) at the end of `out`, which must be even.
fn ifd_into(out: &mut Vec<u8>, mut entries: Vec<(u16, Field)>) {
    entries.sort_by_key(|(tag, _)| *tag);
    let ifd = out.len();
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
    if out.len() % 2 == 1 {
        out.push(0);
    }
}

/// Append an IFD and point IFD0's inline LONG `pointer` tag at it.
fn sub_ifd(out: &mut Vec<u8>, pointer: u16, entries: Vec<(u16, Field)>) {
    let at = out.len() as u32;
    ifd_into(out, entries);
    let ifd0 = u32::from_le_bytes(out[4..8].try_into().unwrap()) as usize;
    let count = usize::from(u16::from_le_bytes([out[ifd0], out[ifd0 + 1]]));
    let row = (0..count)
        .map(|i| ifd0 + 2 + 12 * i)
        .find(|row| u16::from_le_bytes([out[*row], out[*row + 1]]) == pointer)
        .unwrap();
    out[row + 8..row + 12].copy_from_slice(&at.to_le_bytes());
}

const WIDTH: u32 = 64;
const HEIGHT: u32 = 16;

/// A 64x16 LinearRaw DNG ramp with EXIF; `private` adds GPS, serial numbers,
/// an owner and an XMP packet with a person in the image.
fn dng(private: bool) -> Vec<u8> {
    let peak = 0.8;
    let mut pixels = Vec::new();
    for _ in 0..HEIGHT {
        for x in 0..WIDTH {
            let level = (f64::from(x * 65_535 / (WIDTH - 1)) * peak) as u16;
            let pixel = [level / 2, level, (u32::from(level) * 3 / 4) as u16];
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
        (50708, Field::Ascii("Pentool Variant Test Body")),
        (
            50721,
            Field::Srational(vec![
                0.6461, -0.0907, -0.0882, -0.4300, 1.2184, 0.2378, -0.0819, 0.1944, 0.5931,
            ]),
        ),
        (50778, Field::Short(vec![21])),
        (50728, Field::Srational(vec![0.5, 1.0, 0.75])),
        (271, Field::Ascii("Pentool")),
        (272, Field::Ascii("Meta One")),
        (34665, Field::Long(vec![0])),
    ];
    let mut entries = entries;
    let mut exif = vec![
        (34855, Field::Short(vec![200])),
        (36867, Field::Ascii("2026:05:01 18:30:00")),
    ];
    if private {
        entries.push((50735, Field::Ascii("BODY-0042")));
        entries.push((34853, Field::Long(vec![0])));
        entries.push((700, Field::Byte(XMP.as_bytes().to_vec())));
        exif.push((42032, Field::Ascii("Jo Owner")));
        exif.push((42033, Field::Ascii("SN-77")));
    }
    let mut out = tiff(entries, &pixels);
    sub_ifd(&mut out, 34665, exif);
    if private {
        sub_ifd(
            &mut out,
            34853,
            vec![
                (1, Field::Ascii("N")),
                (2, Field::Rational(vec![[52, 1], [22, 1], [0, 1]])),
                (3, Field::Ascii("E")),
                (4, Field::Rational(vec![[4, 1], [53, 1], [0, 1]])),
            ],
        );
    }
    out
}

const XMP: &str = "<x:xmpmeta><rdf:Description Iptc4xmpExt:PersonInImage=\"Sam\"/></x:xmpmeta>";

struct Workspace(std::path::PathBuf);

impl Workspace {
    /// A catalog with photo `a`, whose source carries camera, timestamp, GPS,
    /// serial and owner metadata, and photo `b` with camera metadata only.
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "pentool-export-{tag}-{}-{}",
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
        for (id, private) in [("a", true), ("b", false)] {
            let file = format!("{id}.dng");
            std::fs::write(work.0.join(&file), dng(private)).unwrap();
            work.ok(&["raw", "add", "catalog.pen", id, "--file", &file]);
        }
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

    fn ok(&self, args: &[&str]) -> Value {
        let (ok, out, error) = self.run(args);
        assert!(ok, "{args:?}: {error}");
        out
    }

    /// Run a failing command; the document must be byte-for-byte unchanged.
    fn refuse(&self, args: &[&str]) -> String {
        let before = self.bytes();
        let (ok, _, error) = self.run(args);
        assert!(!ok, "{args:?} succeeded");
        assert_eq!(self.bytes(), before, "{args:?} changed the document");
        error
    }

    fn bytes(&self) -> Vec<u8> {
        std::fs::read(self.0.join("catalog.pen")).unwrap()
    }

    fn doc(&self) -> Value {
        serde_json::from_slice(&self.bytes()).unwrap()
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl Workspace {
    fn exists(&self, path: &str) -> bool {
        self.0.join(path).exists()
    }

    fn read(&self, path: &str) -> Vec<u8> {
        std::fs::read(self.0.join(path)).unwrap()
    }

    /// File names in a directory, sorted.
    fn list(&self, dir: &str) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.0.join(dir))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn recipe(&self, name: &str, recipe: Value) -> Value {
        self.ok(&[
            "photo",
            "recipe",
            "set",
            "catalog.pen",
            name,
            &recipe.to_string(),
        ])
    }

    fn export(&self, recipe: &str, out: &str, extra: &[&str]) -> Value {
        let mut args = vec![
            "photo",
            "export",
            "catalog.pen",
            "--selection",
            "a,b",
            "--recipe",
            recipe,
            "--out",
            out,
        ];
        args.extend_from_slice(extra);
        self.ok(&args)
    }
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(bytes))
}

/// The marker segments of a JPEG up to the scan.
fn segments(bytes: &[u8]) -> Vec<(u8, Vec<u8>)> {
    let mut out = Vec::new();
    let mut at = 2;
    while bytes[at] == 0xFF && bytes[at + 1] != 0xDA {
        let length = usize::from(u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]));
        out.push((bytes[at + 1], bytes[at + 4..at + 2 + length].to_vec()));
        at += 2 + length;
    }
    out
}

/// Horizontal sampling factor byte of the luma component of a baseline JPEG.
fn luma_sampling(bytes: &[u8]) -> u8 {
    let (_, sof) = segments(bytes)
        .into_iter()
        .find(|(m, _)| *m == 0xC0)
        .expect("SOF0");
    sof[7]
}

fn categories(bytes: &[u8]) -> Vec<String> {
    let mut out: Vec<String> = pentool::photo::metadata::read_source(bytes)
        .unwrap()
        .into_iter()
        .map(|i| i.category.to_string())
        .collect();
    out.sort();
    out.dedup();
    out
}

#[test]
fn built_in_recipe_exports_tagged_jpegs_deterministically() {
    let work = Workspace::new("jpeg");
    work.ok(&[
        "photo",
        "describe",
        "catalog.pen",
        "a,b",
        "--copyright",
        "(c) 2026 Pentool",
    ]);
    let before = work.bytes();
    let report = work.export("web-gallery", "out", &[]);
    assert_eq!(work.bytes(), before, "export changed the document");
    assert_eq!(report["count"], 2);
    assert_eq!(report["recipe"]["name"], "web-gallery");
    assert_eq!(report["recipe"]["built_in"], true);
    // Nothing but the outputs is left behind (no staging directory).
    assert_eq!(work.list("out"), ["a-master.jpg", "b-master.jpg"]);
    let mut total = 0;
    for output in report["outputs"].as_array().unwrap() {
        let bytes = work.read(output["path"].as_str().unwrap());
        total += bytes.len() as u64;
        assert_eq!(output["bytes"], bytes.len());
        assert_eq!(output["sha256"], sha256(&bytes));
        // 64x16 is below the 2048 long edge and is not enlarged.
        assert_eq!(
            (output["width"].clone(), output["height"].clone()),
            (json!(64), json!(16))
        );
        let image = image::load_from_memory(&bytes).unwrap();
        assert_eq!((image.width(), image.height()), (64, 16));
        let segments = segments(&bytes);
        let (_, jfif) = &segments[0];
        assert!(jfif.starts_with(b"JFIF\0"));
        assert_eq!(jfif[7], 1, "density is in dots per inch");
        assert_eq!(u16::from_be_bytes([jfif[8], jfif[9]]), 72);
        assert!(segments
            .iter()
            .any(|(m, body)| *m == 0xE2 && body.starts_with(b"ICC_PROFILE\0")));
        assert_eq!(luma_sampling(&bytes), 0x22, "4:2:0");
        // The copyright policy writes copyright and creator only.
        assert_eq!(categories(&bytes), ["copyright"]);
        assert!(!bytes.windows(5).any(|w| w == b"SN-77"));
    }
    assert_eq!(report["bytes"], total);

    let again = work.export("web-gallery", "again", &[]);
    for (x, y) in report["outputs"]
        .as_array()
        .unwrap()
        .iter()
        .zip(again["outputs"].as_array().unwrap())
    {
        assert_eq!(x["sha256"], y["sha256"], "exports are deterministic");
    }
}

#[test]
fn document_recipes_resize_and_choose_format_depth_and_chroma() {
    let work = Workspace::new("formats");
    work.recipe(
        "half",
        json!({"format": "png", "color_space": "srgb", "bit_depth": 16, "ppi": 254,
            "resize": {"mode": "percent", "value": 50}}),
    );
    let report = work.export("half", "png", &[]);
    assert_eq!(work.list("png"), ["a-master.png", "b-master.png"]);
    let bytes = work.read("png/a-master.png");
    let image = image::load_from_memory(&bytes).unwrap();
    assert_eq!((image.width(), image.height()), (32, 8));
    assert_eq!(image.color(), image::ColorType::Rgb16);
    let at = bytes.windows(4).position(|w| w == b"pHYs").unwrap();
    assert_eq!(
        u32::from_be_bytes(bytes[at + 4..at + 8].try_into().unwrap()),
        10_000
    );
    assert_eq!(bytes[at + 12], 1);
    assert_eq!(report["outputs"][0]["delivery"]["depth"], 16);

    work.recipe(
        "master",
        json!({"format": "tiff", "color_space": "srgb", "bit_depth": 16,
            "resize": {"mode": "width", "value": 128, "enlarge": true},
            "metadata": {"policy": "public"}}),
    );
    work.export("master", "tif", &[]);
    let bytes = work.read("tif/a-master.tif");
    let tiff = pentool::photo::tiff::Tiff::parse(&bytes).unwrap();
    let ifd = tiff.ifd0();
    let uint = |tag| ifd.uint(&tiff, tag).unwrap().unwrap();
    assert_eq!((uint(256), uint(257)), (128, 32));
    assert_eq!((uint(259), uint(317)), (8, 2), "deflate with a predictor");
    assert_eq!(ifd.uints(&tiff, 258, 4).unwrap().unwrap(), [16, 16, 16]);
    assert!(ifd.has(34675), "ICC profile");
    assert_eq!(ifd.numbers(&tiff, 282, 1).unwrap().unwrap(), [72.0]);
    let strip = &bytes[uint(273) as usize..(uint(273) + uint(279)) as usize];
    let mut samples = Vec::new();
    std::io::Read::read_to_end(&mut flate2::read::ZlibDecoder::new(strip), &mut samples).unwrap();
    assert_eq!(samples.len(), 128 * 32 * 3 * 2);
    // Undo the predictor; the ramp brightens left to right.
    let mut row: Vec<u16> = samples[..128 * 6]
        .chunks_exact(2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .collect();
    for i in 3..row.len() {
        row[i] = row[i].wrapping_add(row[i - 3]);
    }
    assert!(
        row[3 * 127 + 1] > row[1] + 20_000,
        "{} {}",
        row[1],
        row[3 * 127 + 1]
    );
    let kept = categories(&bytes);
    assert!(kept.contains(&"camera".to_string()), "{kept:?}");
    assert!(!kept.contains(&"gps".to_string()) && !kept.contains(&"serials".to_string()));

    work.recipe(
        "full",
        json!({"format": "jpeg", "color_space": "srgb", "chroma": "444", "quality": 70,
            "resize": {"mode": "long-edge", "value": 32}}),
    );
    work.export("full", "jpg", &[]);
    let bytes = work.read("jpg/b-master.jpg");
    assert_eq!(luma_sampling(&bytes), 0x11, "4:4:4");
    let image = image::load_from_memory(&bytes).unwrap();
    assert_eq!((image.width(), image.height()), (32, 8));
    assert_eq!(
        categories(&bytes),
        Vec::<String>::new(),
        "default policy is none"
    );
}

#[test]
fn names_collide_loudly_and_dry_run_plans_what_runs() {
    let work = Workspace::new("names");
    work.recipe(
        "flat",
        json!({"format": "png", "color_space": "srgb", "naming": "{recipe}"}),
    );
    let error = work.refuse(&[
        "photo",
        "export",
        "catalog.pen",
        "--selection",
        "a,b",
        "--recipe",
        "flat",
        "--out",
        "out",
    ]);
    assert!(
        error.contains("[conflict]") && error.contains("flat.png"),
        "{error}"
    );
    assert!(!work.exists("out"));

    work.recipe(
        "seq",
        json!({"format": "png", "color_space": "srgb", "naming": "{seq:3} {name}/{rating}"}),
    );
    let plan = work.export("seq", "out", &["--dry-run"]);
    assert_eq!(plan["dry_run"], true);
    assert!(!work.exists("out"), "a dry run writes nothing");
    assert!(plan["estimated_bytes"].as_u64().unwrap() > 0);
    let run = work.export("seq", "out", &[]);
    assert_eq!(work.list("out"), ["001_a_0.png", "002_b_0.png"]);
    for (planned, done) in plan["outputs"]
        .as_array()
        .unwrap()
        .iter()
        .zip(run["outputs"].as_array().unwrap())
    {
        for key in ["photo", "variant", "path", "width", "height"] {
            assert_eq!(planned[key], done[key], "{key}");
        }
        assert_eq!(planned["exists"], false);
    }

    let error = work.refuse(&[
        "photo",
        "export",
        "catalog.pen",
        "--selection",
        "a,b",
        "--recipe",
        "seq",
        "--out",
        "out",
    ]);
    assert!(
        error.contains("[policy-denied]") && error.contains("--overwrite"),
        "{error}"
    );
    let replanned = work.export("seq", "out", &["--dry-run", "--overwrite"]);
    assert_eq!(replanned["outputs"][0]["exists"], true);
    work.export("seq", "out", &["--overwrite"]);
    assert_eq!(work.list("out"), ["001_a_0.png", "002_b_0.png"]);

    // Every variant, or a named one.
    work.ok(&["photo", "variant", "add", "catalog.pen", "a", "bw"]);
    let all = work.export("web-gallery", "all", &["--all-variants"]);
    assert_eq!(all["count"], 3);
    assert_eq!(
        work.list("all"),
        ["a-bw.jpg", "a-master.jpg", "b-master.jpg"]
    );
    let error = work.refuse(&[
        "photo",
        "export",
        "catalog.pen",
        "--selection",
        "a,b",
        "--recipe",
        "social",
        "--out",
        "bw",
        "--variant",
        "bw",
    ]);
    assert!(
        error.contains("[missing-resource]") && error.contains("no variant bw"),
        "{error}"
    );
    assert!(!work.exists("bw"));
}

#[test]
fn print_recipes_fit_crop_or_pad_but_never_crop_silently() {
    let work = Workspace::new("print");
    let base = [
        "photo",
        "export",
        "catalog.pen",
        "--selection",
        "a",
        "--recipe",
        "photo-lab",
    ];
    let error = work.refuse(&[&base[..], &["--out", "lab"]].concat());
    assert!(error.contains("--print"), "{error}");
    // 4x1 in at 300 ppi matches the 64x16 photo's aspect.
    let exact = work.ok(&[&base[..], &["--out", "lab", "--print", "4x1in"]].concat());
    assert_eq!(exact["outputs"][0]["width"], 1200);
    assert_eq!(exact["outputs"][0]["height"], 300);
    let bytes = work.read("lab/a-master.jpg");
    assert_eq!(luma_sampling(&bytes), 0x11);
    assert_eq!(u16::from_be_bytes([bytes[14], bytes[15]]), 300);

    let error = work.refuse(&[&base[..], &["--out", "sq", "--print", "2x2in"]].concat());
    assert!(
        error.contains("different aspect") && error.contains("crop or pad"),
        "{error}"
    );
    assert!(!work.exists("sq"));
    work.ok(&[
        &base[..],
        &["--out", "crop", "--print", "2x2in", "--fit", "crop"],
    ]
    .concat());
    let crop = image::load_from_memory(&work.read("crop/a-master.jpg"))
        .unwrap()
        .to_rgb8();
    assert_eq!(crop.dimensions(), (600, 600));
    work.ok(&[
        &base[..],
        &["--out", "pad", "--print", "5.08x5.08cm", "--fit", "pad"],
    ]
    .concat());
    let pad = image::load_from_memory(&work.read("pad/a-master.jpg"))
        .unwrap()
        .to_rgb8();
    assert_eq!(pad.dimensions(), (600, 600));
    // Padding is white; the photo sits in the middle band.
    assert!(
        pad.get_pixel(300, 5).0.iter().all(|c| *c > 245),
        "{:?}",
        pad.get_pixel(300, 5)
    );
    assert!(
        pad.get_pixel(5, 300).0.iter().all(|c| *c < 200),
        "{:?}",
        pad.get_pixel(5, 300)
    );
    assert!(crop.get_pixel(300, 5).0.iter().any(|c| *c < 245));

    let error = work.refuse(&[
        "photo",
        "export",
        "catalog.pen",
        "--selection",
        "a",
        "--recipe",
        "social",
        "--out",
        "x",
        "--print",
        "6x4in",
    ]);
    assert!(error.contains("does not"), "{error}");
}

#[test]
fn recipes_are_validated_listed_and_removed_in_transactions() {
    let work = Workspace::new("recipes");
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/fixtures/photo-v7.pen");
    let listed = work.ok(&["photo", "recipe", "list", fixture.to_str().unwrap()]);
    let names: Vec<&str> = listed["recipes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "web-gallery",
            "social",
            "archive-master",
            "photo-lab",
            "client-proof"
        ]
    );

    for (name, recipe, needle) in [
        (
            "web-gallery",
            json!({"format": "png", "color_space": "srgb"}),
            "built in",
        ),
        (
            "bad",
            json!({"format": "png", "color_space": "srgb", "quality": 9}),
            "quality",
        ),
        (
            "bad",
            json!({"format": "jpeg", "color_space": "srgb", "bit_depth": 16}),
            "8-bit",
        ),
        (
            "bad",
            json!({"format": "png", "color_space": "srgb", "naming": "{captured:YYYYMMDD}"}),
            "timestamps",
        ),
        (
            "bad",
            json!({"format": "png", "color_space": "srgb", "resize": {"mode": "print", "fit": "stretch"}}),
            "fit",
        ),
    ] {
        let error = work.refuse(&[
            "photo",
            "recipe",
            "set",
            "catalog.pen",
            name,
            &recipe.to_string(),
        ]);
        assert!(
            error.contains("[invalid-input]") && error.contains(needle),
            "{error}"
        );
    }
    let set = work.recipe(
        "dated",
        json!({"format": "png", "color_space": "srgb", "naming": "{captured:YYYYMMDD}-{photo}",
            "metadata": {"policy": "none", "include": ["timestamps"]}}),
    );
    assert_eq!(set["result"]["replaced"], false);
    work.export("dated", "dated", &[]);
    assert_eq!(work.list("dated"), ["20260501-a.png", "20260501-b.png"]);
    work.ok(&["photo", "recipe", "remove", "catalog.pen", "dated"]);
    assert!(work.doc()["photography"].get("recipes").is_none());
    let error = work.refuse(&["photo", "recipe", "remove", "catalog.pen", "dated"]);
    assert!(error.contains("[missing-resource]"), "{error}");

    // A malformed recipe written by hand is reported when the document is used.
    let mut doc = work.doc();
    doc["photography"]["recipes"] =
        json!({"hand": {"format": "jpeg", "color_space": "srgb", "chroma": "411"}});
    std::fs::write(
        work.0.join("catalog.pen"),
        serde_json::to_vec_pretty(&doc).unwrap(),
    )
    .unwrap();
    let error = work.refuse(&[
        "photo",
        "export",
        "catalog.pen",
        "--selection",
        "a",
        "--recipe",
        "social",
        "--out",
        "x",
    ]);
    assert!(
        error.contains("[malformed-resource]") && error.contains("hand"),
        "{error}"
    );
    assert!(!work.exists("x"));

    // --out must be a directory.
    std::fs::write(work.0.join("file"), b"x").unwrap();
    let mut doc = work.doc();
    doc["photography"]
        .as_object_mut()
        .unwrap()
        .remove("recipes");
    std::fs::write(
        work.0.join("catalog.pen"),
        serde_json::to_vec_pretty(&doc).unwrap(),
    )
    .unwrap();
    let error = work.refuse(&[
        "photo",
        "export",
        "catalog.pen",
        "--selection",
        "a",
        "--recipe",
        "social",
        "--out",
        "file",
    ]);
    assert!(error.contains("not a directory"), "{error}");
}
