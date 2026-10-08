//! v0.11.0 item 5: lens profiles (`photo profile add --lens`, `photo profile
//! import-lcp`), manual lens and geometry corrections, crop, upright, and
//! `photo render` / `photo info`.
use serde_json::{json, Value};
use std::path::Path;

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

/// Append an EXIF IFD holding FNumber 2 and FocalLength 35 and point the
/// ExifIFD tag of IFD 0 at it.
fn with_exif(mut file: Vec<u8>) -> Vec<u8> {
    if file.len() % 2 == 1 {
        file.push(0);
    }
    let exif = file.len();
    let ifd = u32::from_le_bytes(file[4..8].try_into().unwrap()) as usize;
    let count = u16::from_le_bytes(file[ifd..ifd + 2].try_into().unwrap()) as usize;
    let entry = (0..count)
        .map(|i| ifd + 2 + 12 * i)
        .find(|at| u16::from_le_bytes(file[*at..*at + 2].try_into().unwrap()) == 34665)
        .expect("ExifIFD placeholder");
    file[entry + 8..entry + 12].copy_from_slice(&(exif as u32).to_le_bytes());
    let data = exif + 2 + 2 * 12 + 4;
    file.extend(2u16.to_le_bytes());
    for (index, tag) in [33437u16, 37386].into_iter().enumerate() {
        file.extend(tag.to_le_bytes());
        file.extend(5u16.to_le_bytes());
        file.extend(1u32.to_le_bytes());
        file.extend(((data + 8 * index) as u32).to_le_bytes());
    }
    file.extend(0u32.to_le_bytes());
    for (numerator, denominator) in [(2u32, 1u32), (35, 1)] {
        file.extend(numerator.to_le_bytes());
        file.extend(denominator.to_le_bytes());
    }
    file
}

const MODEL: &str = "Pentool Lens Test Body";
const WIDTH: u32 = 96;
const HEIGHT: u32 = 64;

/// The x of the tilted bright/dark edge at row `y`: about 4 degrees off vertical.
fn edge(y: f64) -> f64 {
    48.0 + (y - 32.0) * 0.07
}

/// A 96x64 LinearRaw DNG, bright left of a slightly tilted edge and dark to
/// its right; `exif` adds a focal length and aperture.
fn dng(exif: bool) -> Vec<u8> {
    let mut pixels = Vec::new();
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let bright = (x as f64 + 0.5) < edge(y as f64 + 0.5);
            let pixel: [u16; 3] = if bright {
                [20_000, 40_000, 30_000]
            } else {
                [5_000, 10_000, 7_500]
            };
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
        (50708, Field::Ascii(MODEL)),
        (
            50721,
            Field::Srational(vec![
                0.6461, -0.0907, -0.0882, -0.4300, 1.2184, 0.2378, -0.0819, 0.1944, 0.5931,
            ]),
        ),
        (50778, Field::Short(vec![21])),
        (50728, Field::Srational(vec![0.5, 1.0, 0.75])),
    ];
    if exif {
        entries.push((34665, Field::Long(vec![0])));
    }
    let file = tiff(entries, &pixels);
    if exif {
        with_exif(file)
    } else {
        file
    }
}

fn lens_profile() -> Value {
    json!({
        "pentool_lens_profile": 1,
        "make": "Pentool",
        "model": "Test 35mm",
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

const LCP: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:photoshop="http://ns.adobe.com/photoshop/1.0/" xmlns:stCamera="http://ns.adobe.com/photoshop/1.0/camera-profile">
   <photoshop:CameraProfiles>
    <rdf:Seq>
     <rdf:li rdf:parseType="Resource">
      <stCamera:Make>Pentool</stCamera:Make>
      <stCamera:Lens>Imported 35mm</stCamera:Lens>
      <stCamera:FocalLength>35</stCamera:FocalLength>
      <stCamera:ApertureValue>2</stCamera:ApertureValue>
      <stCamera:PerspectiveModel rdf:parseType="Resource">
       <stCamera:Version>2</stCamera:Version>
       <stCamera:RadialDistortParam1>-0.05</stCamera:RadialDistortParam1>
      </stCamera:PerspectiveModel>
      <stCamera:FisheyeModel rdf:parseType="Resource"><stCamera:FisheyeModelParam1>1</stCamera:FisheyeModelParam1></stCamera:FisheyeModel>
     </rdf:li>
    </rdf:Seq>
   </photoshop:CameraProfiles>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;

struct Workspace(std::path::PathBuf);

impl Workspace {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "pentool-lens-{tag}-{}-{}",
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

    fn info(&self) -> Value {
        let (ok, out, error) = self.run(&["photo", "info", "catalog.pen", "hero"]);
        assert!(ok, "{error}");
        out
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

fn png_size(path: &Path) -> (u32, u32) {
    let bytes = std::fs::read(path).unwrap();
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    (
        u32::from_be_bytes(bytes[16..20].try_into().unwrap()),
        u32::from_be_bytes(bytes[20..24].try_into().unwrap()),
    )
}

#[test]
fn identity_development_renders_the_full_frame() {
    let work = Workspace::new("identity");
    work.catalog(&dng(false));
    let info = work.info();
    assert_eq!(info["decoded"], json!([96, 64]));
    assert_eq!(info["oriented"], json!([96, 64]));
    assert_eq!(info["output"], json!([96, 64]));
    assert_eq!(info["invalid_pixels"], 0);
    assert_eq!(info["crop"]["rect"], json!([0, 0, 96, 64]));

    let (ok, rendered, error) = work.run(&[
        "photo",
        "render",
        "catalog.pen",
        "hero",
        "--out",
        "hero.png",
    ]);
    assert!(ok, "{error}");
    assert_eq!(rendered["output"], json!([96, 64]));
    assert_eq!(rendered["invalid_pixels"], 0);
    assert_eq!(png_size(&work.0.join("hero.png")), (96, 64));
    let first = std::fs::read(work.0.join("hero.png")).unwrap();
    // Rendering is deterministic, and 16-bit output keeps the size.
    let (ok, _, error) = work.run(&[
        "photo",
        "render",
        "catalog.pen",
        "hero",
        "--out",
        "hero.png",
    ]);
    assert!(ok, "{error}");
    assert_eq!(std::fs::read(work.0.join("hero.png")).unwrap(), first);
    let (ok, deep, error) = work.run(&[
        "photo",
        "render",
        "catalog.pen",
        "hero",
        "--out",
        "deep.png",
        "--depth",
        "16",
        "--space",
        "display-p3",
    ]);
    assert!(ok, "{error}");
    assert_eq!(deep["depth"], 16);
    assert_eq!(png_size(&work.0.join("deep.png")), (96, 64));
    for (args, code) in [
        (vec!["--depth", "12"], "[invalid-input]"),
        (vec!["--space", "cmyk"], "[invalid-input]"),
        (vec!["--variant", "nope"], "[missing-resource]"),
    ] {
        let mut command = vec!["photo", "render", "catalog.pen", "hero", "--out", "x.png"];
        command.extend(args.iter().copied());
        let (ok, _, error) = work.run(&command);
        assert!(!ok && error.contains(code), "{command:?}: {error}");
    }
}

#[test]
fn manual_corrections_geometry_and_crop() {
    let work = Workspace::new("manual");
    let document = work.catalog(&dng(false));

    let developed = work.develop(&[
        "--lens-profile",
        "none",
        "--set",
        "lens.distortion=20",
        "--set",
        r#"lens.vignetting={"amount":30,"midpoint":50}"#,
        "--set",
        "geometry.rotate=5",
    ]);
    let stored = master(&document);
    assert_eq!(stored["lens"]["profile"], "none");
    assert_eq!(stored["lens"]["distortion"], 20);
    assert_eq!(stored["geometry"]["rotate"], 5);
    assert_eq!(developed["lens_profile"], "none");
    let applied = developed["frame"]["lens"].as_array().unwrap();
    assert!(!applied.is_empty(), "{developed}");
    // A rotation without constrain leaves uncovered corners.
    let info = work.info();
    assert!(info["invalid_pixels"].as_u64().unwrap() > 0, "{info}");
    assert_eq!(info["output"], json!([96, 64]));

    // Constrain crops inward until every pixel has a source.
    work.develop(&["--set", "crop.constrain=true"]);
    let info = work.info();
    assert_eq!(info["invalid_pixels"], 0, "{info}");
    assert_eq!(info["crop"]["constrained"], true);
    let [w, h] = [
        info["output"][0].as_u64().unwrap(),
        info["output"][1].as_u64().unwrap(),
    ];
    assert!(w < 96 && h < 64 && w > 48 && h > 32, "{info}");
    let (ok, rendered, error) = work.run(&[
        "photo",
        "render",
        "catalog.pen",
        "hero",
        "--out",
        "rotated.png",
    ]);
    assert!(ok, "{error}");
    assert_eq!(rendered["invalid_pixels"], 0);
    assert_eq!(png_size(&work.0.join("rotated.png")), (w as u32, h as u32));

    // An explicit rectangle and aspect snap to whole pixels.
    work.develop(&[
        "--unset",
        "geometry",
        "--unset",
        "crop",
        "--set",
        "crop.rect=[0.25,0.25,0.5,0.5]",
    ]);
    let info = work.info();
    assert_eq!(info["output"], json!([48, 32]));
    assert_eq!(info["crop"]["rect"], json!([24, 16, 48, 32]));
    // The aspect shapes the constrained crop; the stored rect alone is exact.
    work.develop(&["--set", r#"crop.aspect="1:1""#]);
    assert_eq!(work.info()["output"], json!([48, 32]));
    work.develop(&["--set", "crop.constrain=true"]);
    let info = work.info();
    assert_eq!(info["output"], json!([32, 32]), "{info}");
    assert!(master(&document).get("geometry").is_none());
}

#[test]
fn raw_develop_refuses_bad_lens_and_geometry_without_mutation() {
    let work = Workspace::new("refuse");
    let document = work.catalog(&dng(false));
    let unknown = format!("sha256:{}", "0".repeat(64));
    let before = std::fs::read(&document).unwrap();
    for (args, code) in [
        (vec!["--set", "process=2"], "[invalid-input]"),
        (vec!["--set", "lens.distortion"], "[invalid-input]"),
        (vec!["--set", "lens.distortion=x"], "[invalid-input]"),
        (vec!["--set", "Lens.distortion=1"], "[invalid-input]"),
        (vec!["--set", "lens.distortion=500"], "[invalid-develop]"),
        (vec!["--set", "lens.focus=1"], "[invalid-develop]"),
        (vec!["--set", "geometry.rotate=60"], "[invalid-develop]"),
        (
            vec!["--set", "crop.rect=[0.5,0.5,0.6,0.1]"],
            "[invalid-develop]",
        ),
        (vec!["--set", r#"crop.aspect="0:1""#], "[invalid-develop]"),
        (vec!["--unset", "lens.distortion"], "[invalid-input]"),
        (vec!["--lens-profile", "adobe"], "[invalid-develop]"),
        (vec!["--lens-profile", &unknown], "[missing-resource]"),
        (vec!["--upright", "sideways"], "[invalid-input]"),
        (vec!["--guide", "0,0,1,1"], "[invalid-input]"),
        (vec!["--upright", "guided"], "[invalid-input]"),
        (
            vec!["--upright", "guided", "--guide", "0,0,2,1"],
            "[invalid-input]",
        ),
        (
            vec!["--set", "geometry.rotate=5", "--if-revision", "stale"],
            "",
        ),
    ] {
        let mut command = vec!["raw", "develop", "catalog.pen", "hero"];
        command.extend(args.iter().copied());
        let (ok, _, error) = work.run(&command);
        assert!(!ok, "{command:?} succeeded");
        assert!(error.contains(code), "{command:?}: {error}");
        assert_eq!(std::fs::read(&document).unwrap(), before, "{command:?}");
    }
    // A dry run reports without writing.
    let dry = work.develop(&["--set", "geometry.rotate=3", "--dry-run"]);
    assert_eq!(dry["frame"]["geometry"]["rotate"], 3);
    assert_eq!(std::fs::read(&document).unwrap(), before);
}

#[test]
fn lens_profiles_are_verified_stored_and_interpolated() {
    let work = Workspace::new("profile");
    let document = work.catalog(&dng(true));
    std::fs::write(
        work.0.join("lens.json"),
        serde_json::to_vec_pretty(&lens_profile()).unwrap(),
    )
    .unwrap();
    let before = std::fs::read(&document).unwrap();
    let (ok, dry, error) = work.run(&[
        "photo",
        "profile",
        "add",
        "catalog.pen",
        "--lens",
        "lens.json",
        "--dry-run",
    ]);
    assert!(ok, "{error}");
    assert_eq!(std::fs::read(&document).unwrap(), before);
    let (ok, added, error) = work.run(&[
        "photo",
        "profile",
        "add",
        "catalog.pen",
        "--lens",
        "lens.json",
    ]);
    assert!(ok, "{error}");
    assert_eq!(added["result"], dry["result"]);
    let result = &added["result"];
    assert_eq!(result["kind"], "lens");
    assert_eq!(result["name"], "Pentool Test 35mm");
    assert_eq!(result["imported_from"], "pentool-lens");
    assert_eq!(result["samples"], 2);
    let digest = result["profile"].as_str().unwrap().to_string();
    let record = &read(&document)["photography"]["profiles"][&digest];
    assert_eq!(record["kind"], "lens");
    assert_eq!(record["storage"]["kind"], "embedded");

    // The source's EXIF focal length and aperture select the correction.
    let developed = work.develop(&["--lens-profile", &digest]);
    assert_eq!(developed["lens_profile"], json!({"profile": digest}));
    assert!(!developed["frame"]["lens"].as_array().unwrap().is_empty());
    let (ok, _, error) = work.run(&[
        "photo",
        "render",
        "catalog.pen",
        "hero",
        "--out",
        "lens.png",
    ]);
    assert!(ok, "{error}");

    // Malformed lens profiles and kind mismatches are refused.
    let mut bad = lens_profile();
    bad["samples"][1]["vignette"] = json!({"falloff": [0.1]});
    std::fs::write(work.0.join("mixed.json"), serde_json::to_vec(&bad).unwrap()).unwrap();
    std::fs::write(work.0.join("broken.json"), b"{").unwrap();
    let before = std::fs::read(&document).unwrap();
    for file in ["mixed.json", "broken.json"] {
        let (ok, _, error) = work.run(&["photo", "profile", "add", "catalog.pen", "--lens", file]);
        assert!(
            !ok && error.contains("[malformed-resource]"),
            "{file}: {error}"
        );
        assert_eq!(std::fs::read(&document).unwrap(), before);
    }
    let (ok, _, error) = work.run(&[
        "raw",
        "develop",
        "catalog.pen",
        "hero",
        "--camera-profile",
        &digest,
    ]);
    assert!(!ok, "a lens profile used as a camera profile: {error}");
    assert_eq!(std::fs::read(&document).unwrap(), before);
}

#[test]
fn lens_profile_needs_capture_facts() {
    let work = Workspace::new("nofacts");
    let document = work.catalog(&dng(false));
    std::fs::write(
        work.0.join("lens.json"),
        serde_json::to_vec(&lens_profile()).unwrap(),
    )
    .unwrap();
    let (ok, added, error) = work.run(&[
        "photo",
        "profile",
        "add",
        "catalog.pen",
        "--lens",
        "lens.json",
    ]);
    assert!(ok, "{error}");
    let digest = added["result"]["profile"].as_str().unwrap().to_string();
    let before = std::fs::read(&document).unwrap();
    let (ok, _, error) = work.run(&[
        "raw",
        "develop",
        "catalog.pen",
        "hero",
        "--lens-profile",
        &digest,
    ]);
    assert!(
        !ok && error.contains("[invalid-develop]") && error.contains("focal length"),
        "{error}"
    );
    assert_eq!(std::fs::read(&document).unwrap(), before);
}

#[test]
fn lcp_import_stores_canonical_profile_and_lists_unsupported_models() {
    let work = Workspace::new("lcp");
    let document = work.catalog(&dng(true));
    std::fs::write(work.0.join("lens.lcp"), LCP).unwrap();
    std::fs::write(
        work.0.join("entity.lcp"),
        "<!DOCTYPE x [<!ENTITY a 'b'>]><x/>",
    )
    .unwrap();
    let (ok, imported, error) =
        work.run(&["photo", "profile", "import-lcp", "catalog.pen", "lens.lcp"]);
    assert!(ok, "{error}");
    let result = &imported["result"];
    assert_eq!(result["imported_from"], "lcp");
    assert_eq!(result["name"], "Pentool Imported 35mm");
    let unsupported = result["unsupported"].as_array().unwrap();
    assert!(
        unsupported
            .iter()
            .any(|u| u.as_str().unwrap().contains("FisheyeModel")),
        "{result}"
    );
    let digest = result["profile"].as_str().unwrap().to_string();
    let record = &read(&document)["photography"]["profiles"][&digest];
    assert_eq!(record["unsupported"], result["unsupported"]);
    // Importing again deduplicates on the canonical bytes.
    let (ok, again, error) =
        work.run(&["photo", "profile", "import-lcp", "catalog.pen", "lens.lcp"]);
    assert!(ok, "{error}");
    assert_eq!(again["result"]["deduplicated"], true);
    assert_eq!(again["result"]["profile"], digest.as_str());
    let developed = work.develop(&["--lens-profile", &digest]);
    assert_eq!(developed["lens_profile"], json!({"profile": digest}));

    let before = std::fs::read(&document).unwrap();
    let (ok, _, error) = work.run(&[
        "photo",
        "profile",
        "import-lcp",
        "catalog.pen",
        "entity.lcp",
    ]);
    assert!(!ok && error.contains("[unsupported-capability]"), "{error}");
    assert_eq!(std::fs::read(&document).unwrap(), before);
}

#[test]
fn upright_modes_store_solutions_and_provenance() {
    let work = Workspace::new("upright");
    let document = work.catalog(&dng(false));

    // A guide along the tilted edge straightens it.
    let top = edge(0.0) / 96.0;
    let bottom = edge(64.0) / 96.0;
    let guide = format!("{top:.4},0,{bottom:.4},1");
    let guided = work.develop(&["--upright", "guided", "--guide", &guide]);
    let geometry = master(&document)["geometry"].clone();
    assert_eq!(geometry["upright"], "guided");
    assert_eq!(
        geometry["auto"],
        json!({"algorithm": "upright-guided", "version": 1})
    );
    assert_eq!(geometry["guides"].as_array().unwrap().len(), 1);
    assert_eq!(guided["upright"]["segments"], 1);
    let correction = ["rotate", "vertical", "horizontal"]
        .iter()
        .filter_map(|k| geometry.get(*k).and_then(Value::as_f64))
        .fold(0.0f64, |a, v| a.max(v.abs()));
    assert!(correction > 1.0, "{geometry}");

    // Automatic analysis finds the edge itself.
    let automatic = work.develop(&["--upright", "vertical"]);
    let geometry = master(&document)["geometry"].clone();
    assert_eq!(geometry["upright"], "vertical");
    assert_eq!(
        geometry["auto"],
        json!({"algorithm": "upright-hough", "version": 1})
    );
    assert!(geometry.get("guides").is_none(), "{geometry}");
    assert!(automatic["upright"]["segments"].as_u64().unwrap() >= 1);
    // Deterministic: the same analysis again changes nothing.
    let before = std::fs::read(&document).unwrap();
    work.develop(&["--upright", "vertical"]);
    assert_eq!(std::fs::read(&document).unwrap(), before);

    work.develop(&["--upright", "off"]);
    assert!(master(&document).get("geometry").is_none());
}
