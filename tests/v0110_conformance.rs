//! v0.11.0 item 16: conformance and performance. Hostile sources and
//! profiles, pinned develop and merge outputs, rollback of failing commands,
//! the preview cache, offline reproduction from a package, and the photo
//! benchmark.
use pentool::photo::{catalog, dng::Dng, metadata, png};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};

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

/// A little-endian TIFF (`magic` is `II*\0` or the DCP `IIRC`) with one IFD;
/// `pixels` sit at offset 8.
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

const MODEL: &str = "Pentool Conformance Body";
const COLOR_A: [f64; 9] = [
    0.6455, -0.0938, -0.0832, -0.4688, 1.2401, 0.2552, -0.1067, 0.2142, 0.6648,
];
const COLOR_D65: [f64; 9] = [
    0.6461, -0.0907, -0.0882, -0.4300, 1.2184, 0.2378, -0.0819, 0.1944, 0.5931,
];
const FORWARD: [f64; 9] = [
    0.7976, 0.1352, 0.0313, 0.2880, 0.7119, 0.0001, 0.0000, 0.0000, 0.8251,
];

const W: u32 = 48;
const H: u32 = 32;

/// Smooth textured scene radiance between about 1/16 and 4.
fn radiance(x: u32, y: u32) -> f64 {
    let (fx, fy) = (f64::from(x), f64::from(y));
    let mut h = u64::from(x / 3).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ u64::from(y / 3).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 29;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 32;
    let s = 0.15
        + 0.5 * fx / f64::from(W)
        + 0.2 * fy / f64::from(H)
        + 0.15 * (h % 1000) as f64 / 1000.0;
    (2.0f64).powf(6.0 * s - 4.0)
}

/// A 48x32 LinearRaw DNG of the scene at `exposure`, with the dual-illuminant
/// matrices of [`dcp`]'s camera.
fn dng(exposure: f64) -> Vec<u8> {
    let mut pixels = Vec::new();
    for y in 0..H {
        for x in 0..W {
            let v = (radiance(x, y) * exposure * 12_000.0).clamp(0.0, 65_535.0) as u16;
            for scale in [0.5, 1.0, 0.75] {
                pixels.extend(((f64::from(v) * scale) as u16).to_le_bytes());
            }
        }
    }
    let entries = vec![
        (254, Field::Long(vec![0])),
        (256, Field::Long(vec![W])),
        (257, Field::Long(vec![H])),
        (258, Field::Short(vec![16; 3])),
        (259, Field::Short(vec![1])),
        (262, Field::Short(vec![34892])),
        (273, Field::Long(vec![8])),
        (277, Field::Short(vec![3])),
        (278, Field::Long(vec![H])),
        (279, Field::Long(vec![pixels.len() as u32])),
        (50706, Field::Byte(vec![1, 4, 0, 0])),
        (50708, Field::Ascii(MODEL)),
        (50721, Field::Srational(COLOR_A.to_vec())),
        (50722, Field::Srational(COLOR_D65.to_vec())),
        (50778, Field::Short(vec![17])),
        (50779, Field::Short(vec![21])),
        (50728, Field::Srational(vec![0.5, 1.0, 0.75])),
    ];
    tiff(b"II*\0", entries, &pixels)
}

/// A dual-illuminant `.dcp` for [`MODEL`] with forward matrices and a hue/sat map.
fn dcp() -> Vec<u8> {
    let map: Vec<f32> = (0..4).flat_map(|i| [2.0 * i as f32, 1.0, 1.0]).collect();
    let entries = vec![
        (50708, Field::Ascii(MODEL)),
        (50721, Field::Srational(COLOR_A.to_vec())),
        (50722, Field::Srational(COLOR_D65.to_vec())),
        (50778, Field::Short(vec![17])),
        (50779, Field::Short(vec![21])),
        (50964, Field::Srational(FORWARD.to_vec())),
        (50965, Field::Srational(FORWARD.to_vec())),
        (50936, Field::Ascii("Conformance Standard")),
        (50937, Field::Long(vec![2, 2, 1])),
        (50938, Field::Float(map.clone())),
        (50939, Field::Float(map)),
        (50941, Field::Long(vec![0])),
    ];
    tiff(b"IIRC", entries, &[])
}

fn lens_profile() -> Value {
    json!({
        "pentool_lens_profile": 1,
        "make": "Pentool",
        "model": "Conformance 35mm",
        "focal_range": [24, 70],
        "aperture_range": [2, 16],
        "samples": [
            {"focal": 24, "aperture": 2, "distortion": {"radial": [-0.1, 0, 0]},
             "vignette": {"gain": [0.3, 0, 0, 0, 0]}},
            {"focal": 70, "aperture": 2, "distortion": {"radial": [0.05, 0, 0]},
             "vignette": {"gain": [0.1, 0, 0, 0, 0]}}
        ]
    })
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Whether `message` carries a `[code]` such as `[malformed-resource]`.
fn has_code(message: &str) -> bool {
    message.split('[').skip(1).any(|rest| {
        rest.split_once(']').is_some_and(|(code, _)| {
            !code.is_empty() && code.chars().all(|c| c.is_ascii_lowercase() || c == '-')
        })
    })
}

/// Truncations at every length and, at every offset, the byte inverted and
/// set to 0xFF: deterministic hostile variants of `bytes`.
fn mutations(bytes: &[u8]) -> impl Iterator<Item = Vec<u8>> + '_ {
    let truncated = (0..bytes.len()).map(move |n| bytes[..n].to_vec());
    let flipped = (0..bytes.len()).flat_map(move |at| {
        [0xFFu8, 0x00].into_iter().map(move |mask| {
            let mut copy = bytes.to_vec();
            copy[at] = if mask == 0xFF { 0xFF } else { !copy[at] };
            copy
        })
    });
    truncated.chain(flipped)
}

/// Run `operation` on every mutation of `bytes`: none may panic, and every
/// error must name a code. Returns how many were refused.
fn sweep(what: &str, bytes: &[u8], operation: impl Fn(&[u8]) -> anyhow::Result<()>) -> usize {
    let mut refused = 0;
    for (index, hostile) in mutations(bytes).enumerate() {
        let outcome = catch_unwind(AssertUnwindSafe(|| operation(&hostile)));
        match outcome {
            Err(_) => panic!("{what} mutation {index} panicked"),
            Ok(Err(error)) => {
                let message = format!("{error:#}");
                assert!(has_code(&message), "{what} mutation {index}: {message}");
                refused += 1;
            }
            Ok(Ok(())) => {}
        }
    }
    refused
}

#[test]
fn hostile_dngs_are_refused_by_code_and_never_panic() {
    let fixture = std::fs::read("docs/fixtures/photo-dng-rggb16.dng").unwrap();
    let document = std::env::temp_dir().join("pentool-conformance-unused.pen");
    let refused = sweep("DNG", &fixture, |bytes| {
        let dng = Dng::inspect(bytes)?;
        let _ = dng.raw_facts();
        metadata::read_source(bytes)?;
        let mut raw = pentool::scene::new_document(40, 30);
        catalog::add_raw(
            &mut raw,
            "p",
            "p",
            bytes,
            pentool::image::embedded_storage(bytes),
            catalog::camera_profile("auto")?,
        )?;
        catalog::render_photo(&raw, &document, "p", "master")?;
        Ok(())
    });
    assert!(refused > fixture.len(), "most truncations are refused");
    assert!(!document.exists());
}

#[test]
fn hostile_profiles_and_pngs_are_refused_by_code_and_never_panic() {
    let mut base = pentool::scene::new_document(40, 30);
    let source = dng(1.0);
    catalog::add_raw(
        &mut base,
        "p",
        "p",
        &source,
        pentool::image::embedded_storage(&source),
        catalog::camera_profile("auto").unwrap(),
    )
    .unwrap();

    let profile = dcp();
    let mut valid = base.clone();
    catalog::add_profile(&mut valid, &profile, false).unwrap();
    // Table sizes multiply as 64-bit values without overflowing.
    let huge = tiff(
        b"IIRC",
        vec![
            (50708, Field::Ascii(MODEL)),
            (50721, Field::Srational(COLOR_D65.to_vec())),
            (50778, Field::Short(vec![21])),
            (50937, Field::Long(vec![u32::MAX; 3])),
        ],
        &[],
    );
    let error = catalog::add_profile(&mut base.clone(), &huge, false).unwrap_err();
    assert!(
        format!("{error:#}").contains("[limit-exceeded]"),
        "{error:#}"
    );
    sweep("DCP", &profile, |bytes| {
        let mut raw = base.clone();
        catalog::add_profile(&mut raw, bytes, true).map(|_| ())
    });

    let lens = serde_json::to_vec(&lens_profile()).unwrap();
    catalog::add_lens_profile(&mut base.clone(), &lens).unwrap();
    sweep("lens profile", &lens, |bytes| {
        let mut raw = base.clone();
        catalog::add_lens_profile(&mut raw, bytes).map(|_| ())
    });

    let fixture = std::fs::read("docs/fixtures/photo-rgb16-p3.png").unwrap();
    png::read(&fixture).unwrap();
    sweep("PNG", &fixture, |bytes| png::read(bytes).map(|_| ()));
}

struct Workspace(PathBuf);

impl Workspace {
    fn empty(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "pentool-conformance-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        Workspace(root)
    }

    /// A catalog of three embedded brackets, `b1` to `b3`, at 1/4, 1 and 4
    /// times the exposure.
    fn new(tag: &str) -> Self {
        let work = Self::empty(tag);
        let raw = pentool::scene::new_document(40, 30);
        std::fs::write(work.path(), serde_json::to_vec_pretty(&raw).unwrap()).unwrap();
        for (id, exposure) in [("b1", 0.25), ("b2", 1.0), ("b3", 4.0)] {
            let file = format!("{id}.dng");
            std::fs::write(work.0.join(&file), dng(exposure)).unwrap();
            work.ok(&["raw", "add", "catalog.pen", id, "--file", &file]);
        }
        work
    }

    fn path(&self) -> PathBuf {
        self.0.join("catalog.pen")
    }

    fn run_env(&self, env: &[(&str, &str)], args: &[&str]) -> (bool, Value, String) {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_pentool"));
        command.args(args).current_dir(&self.0);
        command.env_remove("PENTOOL_PHOTO_CACHE_BYTES");
        for (key, value) in env {
            command.env(key, value);
        }
        let output = command.output().unwrap();
        let stdout = serde_json::from_slice(&output.stdout).unwrap_or(Value::Null);
        (
            output.status.success(),
            stdout,
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    }

    fn run(&self, args: &[&str]) -> (bool, Value, String) {
        self.run_env(&[], args)
    }

    fn ok(&self, args: &[&str]) -> Value {
        let (ok, out, error) = self.run(args);
        assert!(ok, "{args:?}: {error}");
        out
    }

    fn bytes(&self) -> Vec<u8> {
        std::fs::read(self.path()).unwrap()
    }

    fn doc(&self) -> Value {
        serde_json::from_slice(&self.bytes()).unwrap()
    }

    fn read(&self, path: &str) -> Vec<u8> {
        std::fs::read(self.0.join(path)).unwrap()
    }

    /// Every file under the workspace with its digest.
    fn tree(&self) -> BTreeMap<String, String> {
        fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                let name = path.strip_prefix(root).unwrap().display().to_string();
                if path.is_dir() {
                    out.insert(format!("{name}/"), String::new());
                    walk(root, &path, out);
                } else {
                    out.insert(name, sha256(&std::fs::read(&path).unwrap()));
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(&self.0, &self.0, &mut out);
        out
    }

    /// Entries of the preview cache directory.
    fn cached(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.0.join(".pentool/cache/photo"))
            .map(|d| {
                d.filter_map(|e| Some(e.ok()?.file_name().to_string_lossy().into_owned()))
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    fn preview(&self, env: &[(&str, &str)], extra: &[&str]) -> (Value, Vec<u8>) {
        let mut args = vec!["photo", "preview", "catalog.pen", "b2", "--out", "p.png"];
        args.extend_from_slice(extra);
        let (ok, report, error) = self.run_env(env, &args);
        assert!(ok, "{args:?}: {error}");
        (report, self.read("p.png"))
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The 16-bit Display P3 develop of the CFA fixture and the HDR merge of
/// the brackets are pinned: engine 1 uses only IEEE-754 basic operations and
/// its own transcendental functions, so every platform must produce these
/// bytes. A change here is a process-version change, never a refresh.
#[test]
fn develops_and_merges_match_their_pinned_digests() {
    const DEVELOP: &str = "6180ba1c52eaf5fac9c08e05e138ff0392af947903f42f2a38fdaf7680bce2c6";
    const MERGE: &str = "sha256:8b902a70b1c518ce5915e4dbac333d81f82f8cb5af32f1f6460bb6877d8a9d3b";
    let work = Workspace::new("golden");
    std::fs::copy("docs/fixtures/photo-dng-rggb16.dng", work.0.join("f.dng")).unwrap();
    work.ok(&["raw", "add", "catalog.pen", "f", "--file", "f.dng"]);
    work.ok(&[
        "raw",
        "develop",
        "catalog.pen",
        "f",
        "--temperature",
        "5200",
        "--tint",
        "8",
        "--exposure",
        "0.3",
        "--set",
        "tone.contrast=15",
        "--set",
        "tone.highlights=-30",
        "--set",
        "presence.clarity=12",
        "--set",
        "presence.vibrance=20",
        "--set",
        r#"detail.sharpening={"amount":40,"radius":1,"detail":25,"masking":0}"#,
        "--set",
        r#"effects.grain={"amount":20,"size":25,"roughness":50,"seed":7}"#,
        "--set",
        r#"effects.vignette={"amount":-20,"midpoint":50,"roundness":0,"feather":50,"highlights":0}"#,
        "--set",
        "lens.distortion=8",
        "--set",
        r#"local=[{"id":"center","mask":{"components":[{"kind":"radial","mode":"add","center":[0.5,0.5],"radius":[0.3,0.3]}]},"params":{"exposure":0.4}}]"#,
    ]);
    let mut digests = Vec::new();
    for _ in 0..2 {
        work.ok(&[
            "photo",
            "render",
            "catalog.pen",
            "f",
            "--out",
            "golden.png",
            "--space",
            "display-p3",
            "--depth",
            "16",
        ]);
        digests.push(sha256(&work.read("golden.png")));
    }
    assert_eq!(digests[0], digests[1], "repeated develops are identical");
    let merged = work.ok(&[
        "photo",
        "merge-hdr",
        "catalog.pen",
        "b1",
        "b2",
        "b3",
        "--id",
        "hdr",
    ]);
    let merge = merged["result"]["asset"].as_str().unwrap().to_owned();
    assert_eq!(
        (digests[0].as_str(), merge.as_str()),
        (DEVELOP, MERGE),
        "pinned engine-1 outputs changed"
    );
}

#[test]
fn failing_photo_commands_leave_every_file_unchanged() {
    let work = Workspace::new("rollback");
    std::fs::write(work.0.join("garbage.dcp"), b"IIRC\x08\0\0\0\xff\xff").unwrap();
    std::fs::write(work.0.join("garbage.json"), b"{\"pentool_lens_profile\": 1").unwrap();
    let mut hostile = dng(1.0);
    hostile.truncate(hostile.len() / 2);
    std::fs::write(work.0.join("hostile.dng"), hostile).unwrap();
    std::fs::copy(
        "docs/fixtures/photo-raw-cr2-header.cr2",
        work.0.join("camera.cr2"),
    )
    .unwrap();
    let revision = format!("sha256:{}", "0".repeat(64));
    let failing: Vec<Vec<&str>> = vec![
        vec!["raw", "develop", "catalog.pen", "b1", "--exposure", "9"],
        vec![
            "raw",
            "develop",
            "catalog.pen",
            "b1",
            "--set",
            "tone.contrast=500",
        ],
        vec![
            "raw",
            "develop",
            "catalog.pen",
            "b1",
            "--set",
            "nonsense.key=1",
        ],
        vec![
            "raw",
            "develop",
            "catalog.pen",
            "b1",
            "--exposure",
            "1",
            "--if-revision",
            &revision,
        ],
        vec!["raw", "develop", "catalog.pen", "nope", "--exposure", "1"],
        vec!["raw", "add", "catalog.pen", "x", "--file", "missing.dng"],
        vec!["raw", "add", "catalog.pen", "x", "--file", "hostile.dng"],
        vec!["raw", "add", "catalog.pen", "x", "--file", "camera.cr2"],
        vec!["raw", "add", "catalog.pen", "b1", "--file", "b1.dng"],
        vec![
            "photo",
            "merge-hdr",
            "catalog.pen",
            "b1",
            "nope",
            "--id",
            "h",
        ],
        vec![
            "photo",
            "merge-hdr",
            "catalog.pen",
            "b1",
            "b2",
            "--id",
            "h",
            "--external",
            "../escape.dng",
        ],
        vec![
            "photo",
            "merge-pano",
            "catalog.pen",
            "b1",
            "b2",
            "--id",
            "b3",
        ],
        vec!["photo", "variant", "remove", "catalog.pen", "b1", "master"],
        vec!["photo", "variant", "add", "catalog.pen", "b1", "master"],
        vec!["photo", "snapshot", "restore", "catalog.pen", "b1", "nope"],
        vec![
            "photo",
            "settings",
            "sync",
            "catalog.pen",
            "b1",
            "--to",
            "nope",
        ],
        vec![
            "photo",
            "profile",
            "add",
            "catalog.pen",
            "--file",
            "garbage.dcp",
        ],
        vec![
            "photo",
            "profile",
            "add",
            "catalog.pen",
            "--lens",
            "garbage.json",
        ],
        vec![
            "photo",
            "mask",
            "paint",
            "catalog.pen",
            "b1",
            "--adjustment",
            "nope",
            "--samples",
            "[]",
        ],
        vec!["photo", "rate", "catalog.pen", "b1", "--rating", "9"],
        vec!["photo", "keyword", "catalog.pen", "nope", "--add", "x"],
        vec!["photo", "describe", "catalog.pen", "nope", "--title", "x"],
        vec![
            "photo",
            "recipe",
            "set",
            "catalog.pen",
            "bad",
            r#"{"format":"gif"}"#,
        ],
        vec![
            "photo",
            "export",
            "catalog.pen",
            "--selection",
            "b1",
            "--recipe",
            "nope",
            "--out",
            "out",
        ],
        vec![
            "photo",
            "export",
            "catalog.pen",
            "--selection",
            "b1,nope",
            "--recipe",
            "web-gallery",
            "--out",
            "out",
        ],
        vec!["photo", "render", "catalog.pen", "nope", "--out", "r.png"],
        vec!["photo", "preview", "catalog.pen", "nope", "--out", "p.png"],
        vec![
            "photo",
            "preview",
            "catalog.pen",
            "b1",
            "--out",
            "p.png",
            "--overlay",
            "mask:nope",
        ],
    ];
    let before = work.tree();
    let document = work.bytes();
    for args in &failing {
        let (ok, _, error) = work.run(args);
        assert!(!ok, "{args:?} should fail");
        assert!(
            has_code(&error) || error.contains("revision mismatch"),
            "{args:?} names no code: {error}"
        );
        assert_eq!(work.bytes(), document, "{args:?} changed the document");
        assert_eq!(work.tree(), before, "{args:?} left files behind");
    }
}

#[test]
fn preview_cache_hits_are_identical_bounded_and_disposable() {
    let work = Workspace::new("cache");
    let document = work.bytes();
    let (first, cold) = work.preview(&[], &[]);
    assert_eq!(first["cache"], "miss");
    let (second, warm) = work.preview(&[], &[]);
    assert_eq!(second["cache"], "hit");
    assert_eq!(warm, cold, "a hit returns the rendered bytes");
    assert_eq!(second["size"], first["size"]);
    assert_eq!(work.cached().len(), 1);
    assert!(work.cached()[0].ends_with(".png"));

    // The key covers the request and the develop settings.
    let (p3, _) = work.preview(&[], &["--space", "display-p3"]);
    assert_eq!(p3["cache"], "miss");
    work.ok(&["raw", "develop", "catalog.pen", "b2", "--exposure", "0.5"]);
    let (developed, brighter) = work.preview(&[], &[]);
    assert_eq!(developed["cache"], "miss");
    assert_ne!(brighter, cold);
    assert_eq!(work.cached().len(), 3);
    let document = {
        assert_ne!(work.bytes(), document);
        work.bytes()
    };

    // Corrupted or renamed entries are misses, never wrong pictures.
    let dir = work.0.join(".pentool/cache/photo");
    for name in work.cached() {
        let path = dir.join(&name);
        let mut bytes = std::fs::read(&path).unwrap();
        let middle = bytes.len() / 2;
        bytes[middle] ^= 0x55;
        std::fs::write(&path, bytes).unwrap();
    }
    let (again, repaired) = work.preview(&[], &[]);
    assert_eq!(again["cache"], "miss");
    assert_eq!(repaired, brighter);
    let names = work.cached();
    let current = names
        .iter()
        .find(|n| {
            let bytes = std::fs::read(dir.join(n)).unwrap();
            png::chunks(&bytes).is_ok()
        })
        .unwrap()
        .clone();
    for name in &names {
        if *name != current {
            std::fs::copy(dir.join(&current), dir.join(name)).unwrap();
        }
    }
    let (renamed, _) = work.preview(&[], &["--space", "display-p3"]);
    assert_eq!(
        renamed["cache"], "miss",
        "an entry under another key is refused"
    );

    // Bypass, the size bound, and an unusable cache never fail a command.
    work.ok(&["photo", "cache", "clear", "catalog.pen"]);
    let (off, bypassed) = work.preview(&[], &["--no-cache"]);
    assert_eq!(off["cache"], "off");
    assert_eq!(bypassed, brighter);
    assert!(work.cached().is_empty());
    let (unbounded, _) = work.preview(&[("PENTOOL_PHOTO_CACHE_BYTES", "0")], &[]);
    assert_eq!(unbounded["cache"], "miss");
    assert!(work.cached().is_empty(), "a zero bound keeps nothing");
    work.preview(&[], &[]);
    let one = std::fs::metadata(dir.join(&work.cached()[0]))
        .unwrap()
        .len();
    let bound = (one + one / 2).to_string();
    let bounded = [("PENTOOL_PHOTO_CACHE_BYTES", bound.as_str())];
    let (newest, _) = work.preview(&bounded, &["--edge", "24"]);
    assert_eq!(newest["cache"], "miss");
    assert_eq!(work.cached().len(), 1, "the oldest entry was evicted");
    let (kept, _) = work.preview(&bounded, &["--edge", "24"]);
    assert_eq!(kept["cache"], "hit", "the newest entry is kept");

    std::fs::write(dir.join("notes.txt"), b"not a cache entry").unwrap();
    let cleared = work.ok(&["photo", "cache", "clear", "catalog.pen"]);
    assert_eq!(cleared["removed_entries"], 1);
    assert_eq!(work.cached(), ["notes.txt"]);
    std::fs::remove_dir_all(work.0.join(".pentool/cache")).unwrap();
    std::fs::write(work.0.join(".pentool/cache"), b"a file, not a directory").unwrap();
    let (blocked, unblocked) = work.preview(&[], &[]);
    assert_eq!(blocked["cache"], "miss");
    assert_eq!(unblocked, brighter);
    assert_eq!(work.bytes(), document, "previews never change the document");

    let (ok, _, error) = work.run(&["photo", "cache", "clear", "missing.pen"]);
    assert!(!ok && error.contains("[missing-resource]"), "{error}");
}

/// Develop, adjust locally, compare and export variants of one immutable
/// source, merge brackets, then package the catalog and reproduce every
/// export elsewhere from the package alone.
#[test]
fn a_packaged_shoot_reproduces_its_exports_offline() {
    let work = Workspace::new("package");
    let source = work.read("b2.dng");
    work.ok(&["photo", "rate", "catalog.pen", "b2", "--rating", "5"]);
    work.ok(&["photo", "rate", "catalog.pen", "b1,b3", "--rating", "2"]);
    let picked = work.ok(&["photo", "search", "catalog.pen", "rating>=4"]);
    assert_eq!(picked["matches"], 1);

    std::fs::write(work.0.join("camera.dcp"), dcp()).unwrap();
    let added = work.ok(&[
        "photo",
        "profile",
        "add",
        "catalog.pen",
        "--file",
        "camera.dcp",
    ]);
    let profile = added["result"]["profile"].as_str().unwrap().to_owned();
    work.ok(&[
        "raw",
        "develop",
        "catalog.pen",
        "b2",
        "--camera-profile",
        &profile,
        "--as-shot",
        "--exposure",
        "0.3",
        "--set",
        r#"local=[{"id":"sky","mask":{"components":[{"kind":"linear","mode":"add","start":[0.5,0],"end":[0.5,0.5]}]},"params":{"exposure":-0.5}}]"#,
    ]);
    work.ok(&["photo", "variant", "add", "catalog.pen", "b2", "mono"]);
    work.ok(&[
        "raw",
        "develop",
        "catalog.pen",
        "b2",
        "--variant",
        "mono",
        "--set",
        r#"monochrome={"enabled":true}"#,
    ]);
    work.ok(&[
        "photo",
        "settings",
        "sync",
        "catalog.pen",
        "b2",
        "--to",
        "b1,b3",
        "--groups",
        "tone",
    ]);
    work.ok(&[
        "photo",
        "merge-hdr",
        "catalog.pen",
        "b1",
        "b2",
        "b3",
        "--id",
        "hdr",
    ]);
    work.ok(&[
        "photo",
        "compare",
        "catalog.pen",
        "b2/master",
        "b2/mono",
        "--out",
        "compare.png",
    ]);
    work.ok(&[
        "photo",
        "recipe",
        "set",
        "catalog.pen",
        "proof",
        r#"{"format":"png","color_space":"display-p3","bit_depth":16,"naming":"{photo}-{variant}-proof"}"#,
    ]);

    let raw = work.doc();
    let digest = raw["photography"]["photos"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "b2")
        .unwrap()["source"]
        .clone();
    assert_eq!(digest, json!(format!("sha256:{}", sha256(&source))));
    assert_eq!(work.read("b2.dng"), source, "the source is never rewritten");

    let exports = [
        vec!["--recipe", "web-gallery"],
        vec!["--recipe", "archive-master", "--all-variants"],
        vec!["--recipe", "photo-lab", "--print", "6x4in", "--fit", "crop"],
        vec!["--recipe", "proof", "--all-variants"],
    ];
    let export = |work: &Workspace, document: &str, out: &str| -> Vec<(String, String)> {
        let mut digests = Vec::new();
        for (index, extra) in exports.iter().enumerate() {
            let out = format!("{out}/{index}");
            let mut args = vec![
                "photo",
                "export",
                document,
                "--selection",
                "b1,b2,b3,hdr",
                "--out",
                &out,
            ];
            args.extend_from_slice(extra);
            let report = work.ok(&args);
            for output in report["outputs"].as_array().unwrap() {
                let file = work.0.join(output["path"].as_str().unwrap());
                let bytes = std::fs::read(&file).unwrap();
                assert_eq!(output["sha256"], sha256(&bytes));
                digests.push((
                    format!("{index}/{}", file.file_name().unwrap().to_string_lossy()),
                    sha256(&bytes),
                ));
            }
        }
        digests
    };
    let original = export(&work, "catalog.pen", "delivery");
    assert_eq!(original.len(), 4 + 5 + 4 + 5);

    let kit = work.0.join("kit");
    work.ok(&["package", "init", kit.to_str().unwrap(), "--name", "shoot"]);
    let mut asset = raw.clone();
    asset["asset"] = json!({"schema": 1, "id": "shoot", "name": "Shoot", "asset_version": "0.1.0", "kind": "component"});
    let packaged = kit.join("assets/shoot.pen");
    std::fs::write(&packaged, serde_json::to_vec_pretty(&asset).unwrap()).unwrap();
    // A preview cache beside the packaged document stays out of the package.
    work.ok(&[
        "photo",
        "preview",
        packaged.to_str().unwrap(),
        "b2",
        "--out",
        "kit-preview.png",
    ]);
    assert!(kit.join("assets/.pentool/cache/photo").is_dir());
    work.ok(&[
        "package",
        "pack",
        kit.to_str().unwrap(),
        "--output",
        "shoot.penpkg",
    ]);
    let archive = std::fs::File::open(work.0.join("shoot.penpkg")).unwrap();
    let archive = zip::ZipArchive::new(archive).unwrap();
    let names: Vec<&str> = archive.file_names().collect();
    assert!(names.contains(&"assets/shoot.pen"), "{names:?}");
    assert!(
        names
            .iter()
            .all(|n| !n.contains(".pentool") && !n.ends_with(".png")),
        "{names:?}"
    );

    // Reproduce from the package alone: no sources, profiles or cache.
    let elsewhere = Workspace::empty("installed");
    let package = work.0.join("shoot.penpkg");
    elsewhere.ok(&["package", "install", package.to_str().unwrap()]);
    let installed = elsewhere
        .tree()
        .into_keys()
        .find(|n| n.replace('\\', "/").ends_with("assets/shoot.pen"))
        .expect("the installed catalog");
    let reproduced = export(&elsewhere, &installed, "delivery");
    assert_eq!(reproduced, original, "the package reproduces every export");
}

#[test]
fn photo_benchmark_reports_a_bounded_reproducible_run() {
    let work = Workspace::empty("bench");
    let report = work.ok(&[
        "benchmark",
        "--photo",
        "--photos",
        "4",
        "--megapixels",
        "0.05",
        "--repetitions",
        "2",
    ]);
    assert_eq!(report["benchmark"], "photo");
    assert_eq!(report["fixture"]["photos"], 4);
    let timings = &report["timings_us"];
    assert_eq!(timings["develop"].as_array().unwrap().len(), 2);
    let previews = timings["preview"].as_array().unwrap();
    assert_eq!(previews[0]["cache"], "miss");
    assert_eq!(previews[1]["cache"], "hit");
    assert_eq!(report["export"]["files"], 4);
    assert!(report["develop_sha256"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));
    for flags in [
        vec!["--photos", "0"],
        vec!["--megapixels", "80"],
        vec!["--scale", "2"],
    ] {
        let mut args = vec!["benchmark", "--photo"];
        args.extend_from_slice(&flags);
        let (ok, _, error) = work.run(&args);
        assert!(!ok, "{args:?} should fail: {error}");
    }
}
