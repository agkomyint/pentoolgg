//! v0.11.0 item 11: variants, snapshots and synchronized settings.
use serde_json::{json, Value};

/// A TIFF field value.
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

const WIDTH: u32 = 64;
const HEIGHT: u32 = 16;

/// A 64x16 LinearRaw DNG: a horizontal ramp from black to `peak` of sensor white.
fn dng(peak: f64) -> Vec<u8> {
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
    ];
    tiff(entries, &pixels)
}

struct Workspace(std::path::PathBuf);

impl Workspace {
    /// A catalog with photos `a` (bright), `b` (dark) and `c` (mid).
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "pentool-variants-{tag}-{}-{}",
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
        for (id, peak) in [("a", 1.0), ("b", 0.2), ("c", 0.5)] {
            let file = format!("{id}.dng");
            std::fs::write(work.0.join(&file), dng(peak)).unwrap();
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

    fn develop_of(&self, photo: &str, variant: &str) -> Value {
        self.photo(photo)["variants"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["id"] == variant)
            .unwrap()["develop"]
            .clone()
    }

    fn develop(&self, photo: &str, args: &[&str]) -> Value {
        let mut command = vec!["raw", "develop", "catalog.pen", photo];
        command.extend(args);
        self.ok(&command)
    }

    fn sync(&self, args: &[&str]) -> Value {
        let mut command = vec!["photo", "settings", "sync", "catalog.pen", "a"];
        command.extend(args);
        self.ok(&command)
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn variants_copy_settings_without_duplicating_sources() {
    let work = Workspace::new("variants");
    work.develop("a", &["--set", "tone.contrast=20"]);
    let assets = work.doc()["photography"]["assets"].clone();
    let size = work.bytes().len();
    let out = work.ok(&[
        "photo",
        "variant",
        "add",
        "catalog.pen",
        "a",
        "warm",
        "--name",
        "Warm",
    ]);
    assert_eq!(out["result"]["from"], json!({"variant": "master"}));
    assert_eq!(work.develop_of("a", "warm"), work.develop_of("a", "master"));
    assert_eq!(work.doc()["photography"]["assets"], assets);
    assert!(
        work.bytes().len() < size + 2048,
        "a variant must not copy source bytes"
    );
    // Variants edit independently.
    work.develop("a", &["--variant", "warm", "--set", "tone.contrast=-30"]);
    assert_eq!(work.develop_of("a", "master")["tone"]["contrast"], 20);
    assert_eq!(work.develop_of("a", "warm")["tone"]["contrast"], -30);
    work.ok(&[
        "photo",
        "variant",
        "rename",
        "catalog.pen",
        "a",
        "warm",
        "--name",
        "Cool",
    ]);
    assert_eq!(work.photo("a")["variants"][1]["name"], "Cool");
    work.ok(&["photo", "variant", "rename", "catalog.pen", "a", "warm"]);
    assert!(work.photo("a")["variants"][1].get("name").is_none());
    for (args, code) in [
        (vec!["add", "catalog.pen", "a", "warm"], "[conflict]"),
        (vec!["add", "catalog.pen", "a", "_bad"], "[invalid-input]"),
        (
            vec!["add", "catalog.pen", "a", "x", "--from", "nope"],
            "[missing-resource]",
        ),
        (
            vec!["add", "catalog.pen", "a", "x", "--from-snapshot", "nope"],
            "[missing-resource]",
        ),
        (vec!["add", "catalog.pen", "zz", "x"], "[missing-resource]"),
        (
            vec!["remove", "catalog.pen", "a", "master"],
            "[invalid-input]",
        ),
        (
            vec!["remove", "catalog.pen", "a", "nope"],
            "[missing-resource]",
        ),
    ] {
        let mut command = vec!["photo", "variant"];
        command.extend(args.iter().copied());
        let error = work.refuse(&command);
        assert!(error.contains(code), "{command:?}: {error}");
    }
    let before = work.bytes();
    work.ok(&[
        "photo",
        "variant",
        "remove",
        "catalog.pen",
        "a",
        "warm",
        "--dry-run",
    ]);
    assert_eq!(work.bytes(), before);
    work.ok(&["photo", "variant", "remove", "catalog.pen", "a", "warm"]);
    assert_eq!(work.photo("a")["variants"].as_array().unwrap().len(), 1);
}

#[test]
fn snapshots_restore_into_their_variant() {
    let work = Workspace::new("snapshots");
    work.develop("a", &["--set", "tone.contrast=20"]);
    work.ok(&["photo", "variant", "add", "catalog.pen", "a", "alt"]);
    let out = work.ok(&[
        "photo",
        "snapshot",
        "add",
        "catalog.pen",
        "a/alt",
        "before",
        "--name",
        "Before",
    ]);
    assert_eq!(out["result"]["variant"], "alt");
    let saved = work.develop_of("a", "alt");
    work.develop("a", &["--variant", "alt", "--set", "tone.contrast=-50"]);
    assert_ne!(work.develop_of("a", "alt"), saved);
    // A variant with snapshots cannot be removed.
    let error = work.refuse(&["photo", "variant", "remove", "catalog.pen", "a", "alt"]);
    assert!(
        error.contains("[conflict]") && error.contains("before"),
        "{error}"
    );
    let out = work.ok(&["photo", "snapshot", "restore", "catalog.pen", "a", "before"]);
    assert_eq!(out["result"]["changed"], true);
    assert_eq!(work.develop_of("a", "alt"), saved);
    assert_eq!(work.photo("a")["snapshots"][0]["develop"], saved);
    // A new variant can start from a snapshot.
    work.ok(&[
        "photo",
        "variant",
        "add",
        "catalog.pen",
        "a",
        "again",
        "--from-snapshot",
        "before",
    ]);
    assert_eq!(work.develop_of("a", "again"), saved);
    let error = work.refuse(&["photo", "snapshot", "add", "catalog.pen", "a/alt", "before"]);
    assert!(error.contains("[conflict]"), "{error}");
    let error = work.refuse(&["photo", "snapshot", "add", "catalog.pen", "a/nope", "s"]);
    assert!(error.contains("[missing-resource]"), "{error}");
    work.ok(&["photo", "snapshot", "remove", "catalog.pen", "a", "before"]);
    assert!(work.photo("a").get("snapshots").is_none());
    work.ok(&["photo", "variant", "remove", "catalog.pen", "a", "alt"]);
    let error = work.refuse(&["photo", "snapshot", "restore", "catalog.pen", "a", "before"]);
    assert!(error.contains("[missing-resource]"), "{error}");
}

#[test]
fn sync_copies_selected_groups_and_reports_each_target() {
    let work = Workspace::new("sync");
    work.develop(
        "a",
        &["--set", "tone.contrast=20", "--set", "crop.aspect=\"1:1\""],
    );
    work.develop("a", &["--set", "effects.vignette.amount=-30"]);
    work.develop("b", &["--set", "presence.vibrance=40"]);
    work.develop("c", &["--set", "crop.aspect=\"2:1\""]);
    let source = work.develop_of("a", "master");
    let before = work.bytes();
    let dry = work.sync(&["--to", "b,c/master", "--except", "crop", "--dry-run"]);
    assert_eq!(work.bytes(), before);
    let targets = dry["result"]["targets"].as_array().unwrap();
    assert_eq!(targets.len(), 2);
    assert_eq!(targets[0]["photo"], "b");
    assert_eq!(targets[1]["variant"], "master");
    let changed: Vec<&str> = targets[0]["changed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g.as_str().unwrap())
        .collect();
    assert!(
        changed.contains(&"tone") && changed.contains(&"presence"),
        "{changed:?}"
    );
    assert!(!changed.contains(&"crop"), "{changed:?}");
    let out = work.sync(&["--to", "b,c/master", "--except", "crop"]);
    assert_eq!(out["result"]["targets"], dry["result"]["targets"]);
    for photo in ["b", "c"] {
        let develop = work.develop_of(photo, "master");
        assert_eq!(develop["tone"], source["tone"]);
        assert_eq!(develop["effects"], source["effects"]);
        // A group the source does not set is removed from the target.
        assert!(develop.get("presence").is_none(), "{develop}");
    }
    assert_eq!(work.develop_of("c", "master")["crop"]["aspect"], "2:1");
    // --groups limits the copy.
    work.develop(
        "a",
        &["--set", "tone.contrast=55", "--set", "presence.clarity=10"],
    );
    work.sync(&["--to", "b", "--groups", "presence"]);
    let b = work.develop_of("b", "master");
    assert_eq!(b["presence"]["clarity"], 10);
    assert_eq!(b["tone"]["contrast"], 20);
    // @ids.json targets.
    std::fs::write(work.0.join("ids.json"), r#"["c"]"#).unwrap();
    work.sync(&["--to", "@ids.json", "--groups", "tone"]);
    assert_eq!(work.develop_of("c", "master")["tone"]["contrast"], 55);
    for (args, code) in [
        (vec!["--to", "b,zz"], "[missing-resource]"),
        (vec!["--to", "b/nope"], "[missing-resource]"),
        (vec!["--to", "a"], "[invalid-input]"),
        (vec!["--to", "b,b"], "[invalid-input]"),
        (vec!["--to", "rating>=3"], "[unsupported-capability]"),
        (vec!["--to", "b", "--groups", "bogus"], "[invalid-input]"),
        (vec!["--to", "b", "--groups", "process"], "[invalid-input]"),
        (
            vec!["--to", "b", "--groups", "tone", "--except", "tone"],
            "[invalid-input]",
        ),
    ] {
        let mut command = vec!["photo", "settings", "sync", "catalog.pen", "a"];
        command.extend(args.iter().copied());
        let error = work.refuse(&command);
        assert!(error.contains(code), "{command:?}: {error}");
    }
}

#[test]
fn sync_copies_resolved_values_or_reanalyses_per_photo() {
    let work = Workspace::new("auto");
    work.develop("a", &["--auto-tone"]);
    let tone = work.develop_of("a", "master")["tone"].clone();
    // Without --auto-per-photo the resolved values and provenance copy as values.
    work.sync(&["--to", "b", "--groups", "tone"]);
    assert_eq!(work.develop_of("b", "master")["tone"], tone);
    // With it, each target gets its own analysis, the same as developing it.
    let out = work.sync(&["--to", "b,c", "--groups", "tone", "--auto-per-photo"]);
    let b = work.develop_of("b", "master")["tone"].clone();
    assert_eq!(out["result"]["targets"][0]["resolved"]["tone"], b);
    assert_ne!(b["exposure"], tone["exposure"], "{b} vs {tone}");
    assert_eq!(b["auto"], json!({"algorithm": "auto-tone", "version": 1}));
    let c = work.develop_of("c", "master")["tone"].clone();
    work.develop("c", &["--auto-tone"]);
    assert_eq!(work.develop_of("c", "master")["tone"], c);
    // An active dehaze airlight is measured again on each target.
    work.develop("a", &["--set", "presence.dehaze=40"]);
    let source = work.develop_of("a", "master")["presence"]["dehaze_airlight"].clone();
    let out = work.sync(&["--to", "b", "--groups", "presence"]);
    let airlight = work.develop_of("b", "master")["presence"]["dehaze_airlight"].clone();
    assert_eq!(
        out["result"]["targets"][0]["resolved"]["dehaze_airlight"],
        airlight
    );
    assert_ne!(airlight, source);
    work.ok(&["photo", "render", "catalog.pen", "b", "--out", "b.png"]);
}
