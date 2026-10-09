//! v0.11.0 item 13: metadata categories, redaction, export policies and the
//! privacy report.
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
            "pentool-metadata-{tag}-{}-{}",
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

    fn photo(&self, id: &str) -> Value {
        self.doc()["photography"]["photos"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == id)
            .unwrap()
            .clone()
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl Workspace {
    /// Render photo `a` with metadata arguments; returns the report and the
    /// metadata keys read back from the PNG.
    fn render(&self, out: &str, metadata: &[&str]) -> (Value, Vec<u8>, Vec<String>) {
        let mut args = vec!["photo", "render", "catalog.pen", "a", "--out", out];
        args.extend_from_slice(metadata);
        let report = self.ok(&args);
        let bytes = std::fs::read(self.0.join(out)).unwrap();
        let keys = pentool::photo::metadata::read_source(&bytes)
            .unwrap()
            .into_iter()
            .map(|i| format!("{}.{}", i.category, i.key))
            .collect();
        (report, bytes, keys)
    }
}

fn has(keys: &[String], key: &str) -> bool {
    keys.iter().any(|k| k == key)
}

#[test]
fn metadata_is_grouped_by_category_and_private_categories_are_redacted() {
    let work = Workspace::new("show");
    let shown = work.ok(&["photo", "metadata", "catalog.pen", "a"]);
    assert_eq!(shown["redacted"], json!(["gps", "serials", "identity"]));
    let categories = &shown["categories"];
    assert_eq!(categories["gps"], json!({"redacted": true, "fields": 4}));
    assert_eq!(
        categories["serials"],
        json!({"redacted": true, "fields": 2})
    );
    assert_eq!(
        categories["identity"],
        json!({"redacted": true, "fields": 2})
    );
    assert_eq!(categories["camera"]["make"], "Pentool");
    assert_eq!(categories["camera"]["iso"], 200);
    assert_eq!(
        categories["timestamps"]["date_time_original"],
        "2026:05:01 18:30:00"
    );
    let text = serde_json::to_string(&shown).unwrap();
    assert!(!text.contains("SN-77") && !text.contains("Jo Owner"));

    let revealed = work.ok(&[
        "photo",
        "metadata",
        "catalog.pen",
        "a",
        "--reveal",
        "gps",
        "--reveal",
        "serials",
    ]);
    assert_eq!(
        revealed["categories"]["gps"]["latitude"],
        json!([52, 22, 0])
    );
    assert_eq!(revealed["categories"]["gps"]["longitude_ref"], "E");
    assert_eq!(revealed["categories"]["serials"]["body_serial"], "SN-77");
    assert_eq!(revealed["redacted"], json!(["identity"]));

    let plain = work.ok(&["photo", "metadata", "catalog.pen", "b"]);
    assert_eq!(plain["redacted"], json!([]));
    assert!(plain["categories"].get("gps").is_none());

    for bad in [
        vec![
            "photo",
            "metadata",
            "catalog.pen",
            "a",
            "--reveal",
            "camera",
        ],
        vec!["photo", "metadata", "catalog.pen", "a", "--reveal", "faces"],
        vec!["photo", "metadata", "catalog.pen", "zz"],
    ] {
        let error = work.refuse(&bad);
        assert!(
            error.contains("[invalid-input]") || error.contains("[missing-resource]"),
            "{bad:?}: {error}"
        );
    }
}

#[test]
fn describe_sets_and_clears_descriptive_fields_in_one_transaction() {
    let work = Workspace::new("describe");
    let out = work.ok(&[
        "photo",
        "describe",
        "catalog.pen",
        "a,b",
        "--title",
        "Harbor",
        "--creator",
        "Ana Photo",
        "--copyright",
        "© 2026 Ana Photo",
        "--city",
        "Oslo",
        "--country",
        "Norway",
    ]);
    assert_eq!(out["result"]["changed"], 2);
    let a = work.photo("a");
    assert_eq!(a["title"], "Harbor");
    assert_eq!(a["location"], json!({"city": "Oslo", "country": "Norway"}));

    work.ok(&[
        "photo",
        "describe",
        "catalog.pen",
        "b",
        "--title",
        "",
        "--city",
        "",
        "--country",
        "",
    ]);
    let b = work.photo("b");
    assert!(b.get("title").is_none() && b.get("location").is_none());
    assert_eq!(b["creator"], "Ana Photo");

    let before = work.bytes();
    work.ok(&[
        "photo",
        "describe",
        "catalog.pen",
        "a",
        "--caption",
        "Dusk",
        "--dry-run",
    ]);
    assert_eq!(work.bytes(), before);

    for bad in [
        vec!["photo", "describe", "catalog.pen", "a"],
        vec![
            "photo",
            "describe",
            "catalog.pen",
            "a",
            "--title",
            "bad\u{7}bell",
        ],
        vec!["photo", "describe", "catalog.pen", "zz", "--title", "x"],
        vec![
            "photo",
            "describe",
            "catalog.pen",
            "a",
            "--caption",
            &"x".repeat(9000),
        ],
    ] {
        work.refuse(&bad);
    }
}

#[test]
fn export_policies_write_only_the_chosen_categories() {
    let work = Workspace::new("export");
    work.ok(&[
        "photo",
        "describe",
        "catalog.pen",
        "a",
        "--title",
        "Harbor",
        "--creator",
        "Ana Photo",
        "--copyright",
        "© 2026 Ana Photo",
        "--city",
        "Oslo",
    ]);
    work.ok(&["photo", "keyword", "catalog.pen", "a", "--add", "blue hour"]);

    let (report, bytes, keys) = work.render("none.png", &[]);
    assert_eq!(report["metadata"]["policy"], "none");
    assert_eq!(report["metadata"]["categories"], json!([]));
    assert!(keys.is_empty(), "{keys:?}");
    let chunks = pentool::photo::png::chunks(&bytes).unwrap();
    assert!(chunks
        .iter()
        .all(|(kind, _)| kind != b"eXIf" && kind != b"iTXt"));

    let (report, bytes, keys) = work.render("copyright.png", &["--metadata", "copyright"]);
    assert_eq!(
        report["metadata"]["categories"],
        json!(["copyright", "creator"])
    );
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("© 2026 Ana Photo") && text.contains("Ana Photo"));
    assert!(!text.contains("Harbor") && !text.contains("Oslo"));
    assert!(!has(&keys, "camera.make"), "{keys:?}");

    let (report, bytes, keys) = work.render("public.png", &["--metadata", "public"]);
    assert_eq!(
        report["metadata"]["categories"],
        json!([
            "camera",
            "copyright",
            "creator",
            "description",
            "keywords",
            "location",
            "software"
        ])
    );
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("blue hour") && text.contains("Oslo") && text.contains("Harbor"));
    assert!(
        has(&keys, "camera.make") && has(&keys, "camera.iso"),
        "{keys:?}"
    );
    for private in [
        "timestamps.date_time_original",
        "gps.latitude",
        "serials.body_serial",
        "serials.camera_serial",
        "identity.camera_owner",
        "identity.xmp:PersonInImage",
    ] {
        assert!(!has(&keys, private), "{private} in {keys:?}");
    }
    assert!(!text.contains("SN-77") && !text.contains("Jo Owner") && !text.contains("Sam"));

    let (_, again, _) = work.render("public-again.png", &["--metadata", "public"]);
    assert_eq!(again, bytes, "metadata embedding is deterministic");

    let (_, _, keys) = work.render(
        "dated.png",
        &[
            "--metadata",
            "public",
            "--metadata-include",
            "timestamps",
            "--metadata-exclude",
            "camera",
        ],
    );
    assert!(has(&keys, "timestamps.date_time_original") && !has(&keys, "camera.make"));

    let (report, bytes, keys) = work.render("all.png", &["--metadata", "all-including-private"]);
    assert!(report["metadata"]["categories"]
        .as_array()
        .unwrap()
        .contains(&json!("gps")));
    for private in [
        "gps.latitude",
        "serials.body_serial",
        "serials.camera_serial",
        "identity.camera_owner",
    ] {
        assert!(has(&keys, private), "{private} missing from {keys:?}");
    }
    // Source XMP is never copied; the packet is pentool's own.
    assert!(!has(&keys, "identity.xmp:PersonInImage"));
    assert!(!String::from_utf8_lossy(&bytes).contains("Sam"));

    for bad in [
        vec!["--metadata", "everything"],
        vec!["--metadata", "none", "--metadata-include", "faces"],
        vec!["--metadata-include", "gps", "--metadata-exclude", "gps"],
    ] {
        let mut args = vec!["photo", "render", "catalog.pen", "a", "--out", "bad.png"];
        args.extend(bad);
        let error = work.refuse(&args);
        assert!(error.contains("[invalid-input]"), "{error}");
        assert!(!work.0.join("bad.png").exists());
    }
}

#[test]
fn privacy_report_and_package_pack_flag_private_sources() {
    let work = Workspace::new("report");
    let report = work.ok(&["photo", "privacy-report", "catalog.pen"]);
    assert_eq!(report["sources"], 2);
    assert_eq!(report["flagged"], 1);
    let flagged = &report["private"][0];
    assert_eq!(flagged["photos"], json!(["a"]));
    assert_eq!(
        flagged["categories"],
        json!({"gps": 4, "identity": 2, "serials": 2})
    );
    assert_eq!(flagged["storage"], "embedded");
    assert_eq!(report["unreadable"], json!([]));

    let kit = work.0.join("kit");
    work.ok(&[
        "package",
        "init",
        kit.to_str().unwrap(),
        "--name",
        "photo-kit",
    ]);
    let mut raw = work.doc();
    raw["asset"] = json!({"schema": 1, "id": "photos", "name": "Photos", "asset_version": "0.1.0", "kind": "component"});
    std::fs::write(
        kit.join("assets/photos.pen"),
        serde_json::to_vec_pretty(&raw).unwrap(),
    )
    .unwrap();
    let (ok, packed, error) = work.run(&[
        "package",
        "pack",
        kit.to_str().unwrap(),
        "--output",
        "kit.penpkg",
    ]);
    assert!(ok, "{error}");
    let warnings = packed["privacy"].as_array().unwrap();
    assert_eq!(warnings.len(), 1);
    let warning = warnings[0].as_str().unwrap();
    assert!(warning.contains("assets/photos.pen") && warning.contains("gps, identity, serials"));
    assert!(error.contains("warning:") && error.contains("privacy-report"));
}
