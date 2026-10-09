//! v0.11.0 item 15: photographer editor (studio previews, overlays and edits).
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
            "pentool-studio-{tag}-{}-{}",
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

use pentool::photo::{png, studio};

impl Workspace {
    fn path(&self) -> std::path::PathBuf {
        self.0.join("catalog.pen")
    }

    fn preview(&self, raw: &Value, space: &str, edge: u32, overlay: &str) -> studio::Preview {
        self.try_preview(raw, space, edge, overlay, false).unwrap()
    }

    fn try_preview(
        &self,
        raw: &Value,
        space: &str,
        edge: u32,
        overlay: &str,
        uncropped: bool,
    ) -> anyhow::Result<studio::Preview> {
        studio::preview(
            raw,
            &self.path(),
            &studio::PreviewRequest {
                photo: "a",
                variant: "master",
                edge,
                space,
                overlay: studio::Overlay::parse(overlay)?,
                uncropped,
            },
        )
    }

    /// Apply a studio edit in memory; the caller decides what to keep.
    fn edit(&self, raw: &mut Value, op: Value) -> anyhow::Result<Value> {
        studio::edit(raw, &self.path(), &op)
    }
}

fn develop<'a>(raw: &'a Value, photo: &str, variant: &str) -> &'a Value {
    let photo = raw["photography"]["photos"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == photo)
        .unwrap();
    &photo["variants"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["id"] == variant)
        .unwrap()["develop"]
}

fn chunk(bytes: &[u8], kind: &[u8; 4]) -> Option<Vec<u8>> {
    png::chunks(bytes)
        .unwrap()
        .into_iter()
        .find(|c| &c.0 == kind)
        .map(|c| c.1)
}

#[test]
fn previews_fit_the_edge_and_are_tagged_for_their_space() {
    let ws = Workspace::new("tags");
    let raw = ws.doc();
    let p3 = ws.preview(&raw, "display-p3", 32, "none");
    assert_eq!(chunk(&p3.png, b"cICP"), Some(vec![12, 13, 0, 1]));
    assert!(chunk(&p3.png, b"iCCP").is_some());
    assert_eq!(p3.report["delivery"]["space"], "display-p3");
    assert_eq!(
        (p3.report["width"].as_u64(), p3.report["height"].as_u64()),
        (Some(32), Some(8))
    );
    assert_eq!(p3.report["developed_width"], WIDTH);
    let decoded = png::read(&p3.png).unwrap();
    assert_eq!((decoded.raster.width, decoded.raster.height), (32, 8));

    let srgb = ws.preview(&raw, "srgb", 4096, "none");
    assert_eq!(chunk(&srgb.png, b"cICP"), Some(vec![1, 13, 0, 1]));
    assert_eq!(srgb.report["width"], WIDTH, "previews never enlarge");
    let histogram = &srgb.report["delivery"]["histogram"];
    let total: u64 = histogram["r"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap())
        .sum();
    assert_eq!(total, u64::from(WIDTH * HEIGHT));

    // Repeatable: the same request renders the same bytes.
    assert_eq!(ws.preview(&raw, "srgb", 4096, "none").png, srgb.png);

    for (space, edge, overlay) in [
        ("rec2020", 64, "none"),
        ("srgb", 0, "none"),
        ("srgb", 5000, "none"),
        ("srgb", 64, "zebra"),
        ("srgb", 64, "mask:"),
    ] {
        let error = ws
            .try_preview(&raw, space, edge, overlay, false)
            .err()
            .unwrap();
        assert!(
            error.to_string().contains("[invalid-input]"),
            "{space} {edge} {overlay}: {error}"
        );
    }
}

#[test]
fn clipping_gamut_and_mask_overlays_mark_the_right_pixels() {
    let ws = Workspace::new("overlays");
    let mut raw = ws.doc();
    let plain = ws.preview(&raw, "srgb", 64, "none");
    assert_eq!(plain.report["marked_pixels"], 0);
    assert!(
        plain.report["shadow_clipped_pixels"].as_u64().unwrap() > 0,
        "the ramp starts at black"
    );

    ws.edit(
        &mut raw,
        json!({"op": "develop", "photo": "a", "set": {"tone.exposure": 4}}),
    )
    .unwrap();
    let bright = ws.preview(&raw, "srgb", 64, "clipping");
    let highlights = bright.report["highlight_clipped_pixels"].as_u64().unwrap();
    assert!(highlights > 0);
    assert_eq!(
        bright.report["marked_pixels"].as_u64().unwrap(),
        highlights + bright.report["shadow_clipped_pixels"].as_u64().unwrap()
    );
    let image = png::read(&bright.png).unwrap().raster;
    let pentool::photo::pixels::Samples::Eight(codes) = &image.samples else {
        panic!("8-bit")
    };
    let channels = if image.alpha { 4 } else { 3 };
    assert!(
        codes.chunks_exact(channels).any(|p| p[..3] == [255, 0, 0]),
        "highlights painted red"
    );
    // The overlay never changes the counts or the histogram.
    let plain_bright = ws.preview(&raw, "srgb", 64, "none");
    assert_eq!(
        plain_bright.report["highlight_clipped_pixels"],
        bright.report["highlight_clipped_pixels"]
    );
    assert_eq!(
        plain_bright.report["delivery"]["histogram"],
        bright.report["delivery"]["histogram"]
    );

    let gamut = ws.preview(&raw, "srgb", 64, "gamut");
    assert_eq!(
        gamut.report["marked_pixels"],
        gamut.report["delivery"]["out_of_gamut_pixels"]
    );

    let missing = ws
        .try_preview(&raw, "srgb", 64, "mask:sky", false)
        .err()
        .unwrap();
    assert!(
        missing
            .to_string()
            .contains("no enabled local adjustment sky"),
        "{missing}"
    );
    let local = json!([{"id": "sky", "mask": {"components": [
        {"kind": "linear", "mode": "add", "start": [0.5, 0], "end": [0.5, 1.0]}
    ]}, "params": {"exposure": -1}}]);
    ws.edit(
        &mut raw,
        json!({"op": "develop", "photo": "a", "set": {"local": local}}),
    )
    .unwrap();
    let mask = ws.preview(&raw, "srgb", 64, "mask:sky");
    assert_eq!(mask.report["overlay"], "mask:sky");
    let marked = mask.report["marked_pixels"].as_u64().unwrap();
    assert!(
        marked > 0 && marked <= u64::from(WIDTH * HEIGHT),
        "{marked}"
    );
}

#[test]
fn uncropped_previews_show_the_whole_frame_for_crop_and_brush_editing() {
    let ws = Workspace::new("uncropped");
    let mut raw = ws.doc();
    ws.edit(
        &mut raw,
        json!({"op": "develop", "photo": "a", "set": {"crop.rect": [0, 0, 0.5, 1]}}),
    )
    .unwrap();
    let cropped = ws.preview(&raw, "srgb", 4096, "none");
    assert_eq!(cropped.report["developed_width"], WIDTH / 2);
    let whole = ws.try_preview(&raw, "srgb", 4096, "none", true).unwrap();
    assert_eq!(whole.report["developed_width"], WIDTH);
    assert_eq!(whole.report["uncropped"], true);
    assert_eq!(
        develop(&raw, "a", "master")["crop"]["rect"],
        json!([0, 0, 0.5, 1]),
        "the crop is kept"
    );
}

#[test]
fn studio_edits_reach_every_panel_through_the_shared_engines() {
    let ws = Workspace::new("edits");
    let mut raw = ws.doc();
    // Every slider path the develop panel writes is a valid develop setting.
    let sliders = json!({
        "tone.exposure": 0.5, "tone.contrast": 10, "tone.highlights": -20, "tone.shadows": 20,
        "tone.whites": 5, "tone.blacks": -5, "presence.texture": 10, "presence.clarity": 10,
        "presence.dehaze": 5, "presence.vibrance": 10, "presence.saturation": -10,
        "detail.sharpening.amount": 40, "detail.noise.luminance": 20, "detail.noise.color": 25,
        "geometry.rotate": 1.5, "geometry.vertical": 10, "geometry.horizontal": -10,
        "lens.distortion": 5, "effects.vignette.amount": -20, "effects.grain.amount": 10,
        "effects.grain.seed": 7,
        "crop.rect": [0.1, 0.1, 0.8, 0.8], "crop.aspect": "4:3"
    });
    ws.edit(
        &mut raw,
        json!({"op": "develop", "photo": "a", "set": sliders}),
    )
    .unwrap();
    let settings = develop(&raw, "a", "master");
    assert_eq!(settings["tone"]["exposure"], 0.5);
    assert_eq!(settings["effects"]["grain"]["amount"], 10);
    ws.edit(
        &mut raw,
        json!({"op": "develop", "photo": "a", "unset": ["tone.exposure", "crop"]}),
    )
    .unwrap();
    assert!(develop(&raw, "a", "master")["tone"]
        .get("exposure")
        .is_none());
    assert!(develop(&raw, "a", "master").get("crop").is_none());
    for wb in [
        json!({"mode": "as-shot"}),
        json!({"mode": "suggest"}),
        json!({"mode": "temperature", "temperature": 4800, "tint": 5}),
    ] {
        ws.edit(
            &mut raw,
            json!({"op": "develop", "photo": "a", "white_balance": wb}),
        )
        .unwrap();
    }
    assert_eq!(
        develop(&raw, "a", "master")["white_balance"]["temperature"],
        4800
    );
    ws.edit(
        &mut raw,
        json!({"op": "develop", "photo": "a", "auto_tone": true}),
    )
    .unwrap();

    // Culling.
    ws.edit(
        &mut raw,
        json!({"op": "rate", "selection": "a,b", "rating": 4, "pick": "pick", "label": "green"}),
    )
    .unwrap();
    ws.edit(
        &mut raw,
        json!({"op": "rate", "selection": "b", "pick": "reject"}),
    )
    .unwrap();
    let search = pentool::photo::organize::search(&raw, "pick:pick", 10, 0).unwrap();
    assert_eq!(search["matches"], 1);
    assert_eq!(search["photos"][0]["label"], "green");
    ws.edit(
        &mut raw,
        json!({"op": "keyword", "selection": "a", "add": ["dusk", "city"]}),
    )
    .unwrap();
    ws.edit(
        &mut raw,
        json!({"op": "keyword", "selection": "a", "remove": ["city"]}),
    )
    .unwrap();
    let detail = studio::detail(&raw, "a").unwrap();
    assert_eq!(detail["photo"]["keywords"], json!(["dusk"]));
    assert_eq!(
        (detail["width"].as_u64(), detail["height"].as_u64()),
        (Some(64), Some(16))
    );

    // Variants, snapshots and copy/sync.
    ws.edit(
        &mut raw,
        json!({"op": "variant", "photo": "a", "id": "mono", "from": "master"}),
    )
    .unwrap();
    ws.edit(
        &mut raw,
        json!({"op": "develop", "photo": "a", "variant": "mono", "set": {"tone.exposure": -1}}),
    )
    .unwrap();
    ws.edit(
        &mut raw,
        json!({"op": "snapshot", "photo": "a", "variant": "mono", "id": "before"}),
    )
    .unwrap();
    ws.edit(
        &mut raw,
        json!({"op": "develop", "photo": "a", "variant": "mono", "set": {"tone.exposure": 2}}),
    )
    .unwrap();
    ws.edit(
        &mut raw,
        json!({"op": "restore", "photo": "a", "snapshot": "before"}),
    )
    .unwrap();
    assert_eq!(develop(&raw, "a", "mono")["tone"]["exposure"], -1);
    let detail = studio::detail(&raw, "a").unwrap();
    assert_eq!(detail["local"].as_array().unwrap().len(), 2);
    let detail_before = develop(&raw, "b", "master")["detail"].clone();
    assert_ne!(develop(&raw, "a", "mono")["detail"], detail_before);
    ws.edit(
        &mut raw,
        json!({"op": "sync", "source": "a/mono", "to": "b", "groups": ["tone"]}),
    )
    .unwrap();
    assert_eq!(develop(&raw, "b", "master")["tone"]["exposure"], -1);
    assert_eq!(
        develop(&raw, "b", "master")["detail"],
        detail_before,
        "only the chosen groups sync"
    );
    pentool::scene::validate(&raw).unwrap();
}

#[test]
fn malformed_studio_edits_are_refused_by_name() {
    let ws = Workspace::new("refuse");
    let raw = ws.doc();
    for (op, expected) in [
        (json!({"op": "teleport"}), "[invalid-input]"),
        (json!({"photo": "a"}), "[invalid-input]"),
        (
            json!({"op": "develop", "photo": "a", "set": {"tone.exposure": 9}}),
            "[invalid-develop]",
        ),
        (json!({"op": "develop", "photo": "a", "sett": {}}), "sett"),
        (
            json!({"op": "develop", "photo": "zz", "set": {"tone.exposure": 1}}),
            "zz",
        ),
        (
            json!({"op": "rate", "selection": "a", "rating": 7}),
            "rating",
        ),
        (
            json!({"op": "rate", "selection": "a", "label": "teal"}),
            "label must be one of",
        ),
        (
            json!({"op": "restore", "photo": "a", "snapshot": "nope"}),
            "nope",
        ),
        (
            json!({"op": "paint", "photo": "a", "adjustment": "ghost", "samples": [{"x": 0.5, "y": 0.5}], "size": 0.1}),
            "ghost",
        ),
        (
            json!({"op": "paint", "photo": "a", "adjustment": "x", "samples": [], "size": 0.1}),
            "[invalid-input]",
        ),
    ] {
        let mut copy = raw.clone();
        let error = ws.edit(&mut copy, op.clone()).err().unwrap().to_string();
        assert!(error.contains(expected), "{op}: {error}");
    }
}

#[test]
fn brush_strokes_create_and_extend_masks_on_the_uncropped_frame() {
    let ws = Workspace::new("paint");
    let mut raw = ws.doc();
    let stroke = json!([{"x": 0.1, "y": 0.5}, {"x": 0.3, "y": 0.5, "pressure": 0.8}]);
    let created = ws
        .edit(&mut raw, json!({"op": "paint", "photo": "a", "create": {"id": "dodge", "params": {"exposure": 1}},
            "samples": stroke, "size": 0.1, "brush": {"hardness": 0.3}}))
        .unwrap();
    assert_eq!(created["created"], true);
    assert_eq!(created["component"], 0);
    let local = &develop(&raw, "a", "master")["local"];
    assert_eq!(local.as_array().unwrap().len(), 1);
    assert_eq!(local[0]["id"], "dodge");
    assert_eq!(local[0]["params"]["exposure"], 1);
    let components = local[0]["mask"]["components"].as_array().unwrap();
    assert_eq!(components.len(), 1);
    assert_eq!(components[0]["kind"], "brush");

    let before = ws.preview(&raw, "srgb", 64, "mask:dodge").report["marked_pixels"]
        .as_u64()
        .unwrap();
    assert!(before > 0);
    ws.edit(
        &mut raw,
        json!({"op": "paint", "photo": "a", "adjustment": "dodge",
        "samples": [{"x": 0.7, "y": 0.5}, {"x": 0.9, "y": 0.5}], "size": 0.1}),
    )
    .unwrap();
    let after = ws.preview(&raw, "srgb", 64, "mask:dodge").report["marked_pixels"]
        .as_u64()
        .unwrap();
    assert!(after > before, "{before} -> {after}");
    ws.edit(
        &mut raw,
        json!({"op": "paint", "photo": "a", "adjustment": "dodge", "erase": true,
        "samples": [{"x": 0.0, "y": 0.5}, {"x": 1.0, "y": 0.5}], "size": 0.5}),
    )
    .unwrap();
    let erased = ws.preview(&raw, "srgb", 64, "mask:dodge").report["marked_pixels"]
        .as_u64()
        .unwrap();
    assert!(erased < after, "{after} -> {erased}");

    // A duplicate ID is refused before any stroke is painted.
    let mut copy = raw.clone();
    let error = ws
        .edit(
            &mut copy,
            json!({"op": "paint", "photo": "a", "create": {"id": "dodge"},
            "samples": [{"x": 0.5, "y": 0.5}], "size": 0.1}),
        )
        .err()
        .unwrap();
    assert!(error.to_string().contains("dodge"), "{error}");
}

struct Server(std::process::Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn serve_exposes_the_studio_with_revision_guarded_edits() {
    let ws = Workspace::new("serve");
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let _server = Server(
        std::process::Command::new(env!("CARGO_BIN_EXE_pentool"))
            .args(["serve", "catalog.pen", "--port", &port.to_string()])
            .current_dir(&ws.0)
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::blocking::Client::new();
    let ready = (0..100).any(|_| {
        let up = client.get(format!("{base}/api/health")).send().is_ok();
        if !up {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        up
    });
    assert!(ready, "server did not start");
    let page = client
        .get(format!("{base}/"))
        .send()
        .unwrap()
        .text()
        .unwrap();
    assert!(page.contains("id=\"photoStudio\"") && page.contains("/photo-panel.js"));
    let script = client.get(format!("{base}/photo-panel.js")).send().unwrap();
    assert!(script.status().is_success());
    let get = |path: &str| -> (u16, Value) {
        let response = client.get(format!("{base}{path}")).send().unwrap();
        (
            response.status().as_u16(),
            serde_json::from_str(&response.text().unwrap()).unwrap(),
        )
    };
    let post = |path: &str, body: Value| -> (u16, Value) {
        let response = client
            .post(format!("{base}{path}"))
            .header("content-type", "application/json")
            .body(body.to_string())
            .send()
            .unwrap();
        (
            response.status().as_u16(),
            serde_json::from_str(&response.text().unwrap()).unwrap(),
        )
    };

    let (status, catalog) = get("/api/photo/catalog?query=&limit=1");
    assert_eq!(status, 200, "{catalog}");
    assert_eq!(
        (catalog["matches"].as_u64(), catalog["returned"].as_u64()),
        (Some(2), Some(1))
    );
    assert_eq!(catalog["has_more"], true);
    let revision = catalog["revision"].as_str().unwrap().to_owned();
    let (status, detail) = get("/api/photo/detail?photo=b");
    assert_eq!(status, 200, "{detail}");
    assert_eq!(detail["photo"]["id"], "b");
    assert_eq!(get("/api/photo/detail?photo=zz").0, 400);

    let (status, shot) = post(
        "/api/photo/preview",
        json!({"photo": "a", "edge": 32, "space": "display-p3", "overlay": "clipping"}),
    );
    assert_eq!(status, 200, "{shot}");
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(shot["png"].as_str().unwrap())
        .unwrap();
    assert_eq!(chunk(&bytes, b"cICP"), Some(vec![12, 13, 0, 1]));
    assert_eq!(shot["report"]["overlay"], "clipping");
    assert_eq!(
        post(
            "/api/photo/preview",
            json!({"photo": "a", "space": "prophoto"})
        )
        .0,
        400
    );

    // Dry run reports without writing.
    let before = ws.bytes();
    let rate = json!({"op": "rate", "selection": "a", "rating": 5});
    let (status, dry) = post(
        "/api/photo/edit",
        json!({"edit": rate, "revision": revision, "dry_run": true}),
    );
    assert_eq!(status, 200, "{dry}");
    assert_eq!(ws.bytes(), before);
    // A guarded edit commits and returns the new revision.
    let (status, done) = post(
        "/api/photo/edit",
        json!({"edit": rate, "revision": revision}),
    );
    assert_eq!(status, 200, "{done}");
    assert_ne!(done["revision"], json!(revision));
    assert_eq!(ws.doc()["photography"]["photos"][0]["rating"], 5);
    // A stale revision is a conflict and leaves the file untouched.
    let after = ws.bytes();
    let (status, stale) = post(
        "/api/photo/edit",
        json!({"edit": {"op": "rate", "selection": "a", "rating": 1}, "revision": revision}),
    );
    assert_eq!(status, 400);
    assert!(
        stale["error"].as_str().unwrap().contains("[conflict]"),
        "{stale}"
    );
    assert_eq!(ws.bytes(), after);
    // An invalid edit leaves it untouched too.
    let (status, bad) = post(
        "/api/photo/edit",
        json!({"edit": {"op": "develop", "photo": "a", "set": {"tone.exposure": 99}}, "revision": done["revision"]}),
    );
    assert_eq!(status, 400, "{bad}");
    assert_eq!(ws.bytes(), after);
    // Undo restores the rating through the shared history.
    assert!(client
        .post(format!("{base}/api/undo"))
        .send()
        .unwrap()
        .status()
        .is_success());
    assert_eq!(
        ws.doc()["photography"]["photos"][0]["rating"]
            .as_u64()
            .unwrap_or(0),
        0
    );
}
