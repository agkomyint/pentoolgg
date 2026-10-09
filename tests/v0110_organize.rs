//! v0.11.0 item 12: ratings, keywords, stacks, collections, search, contact
//! sheets and compare.
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
            "pentool-organize-{tag}-{}-{}",
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
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn ids(out: &Value) -> Vec<String> {
    out["photos"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_str().unwrap().to_string())
        .collect()
}

impl Workspace {
    fn search(&self, query: &str) -> Value {
        self.ok(&["photo", "search", "catalog.pen", query])
    }
}

#[test]
fn ratings_picks_labels_and_keywords_are_set_per_selection() {
    let work = Workspace::new("rate");
    let out = work.ok(&[
        "photo",
        "rate",
        "catalog.pen",
        "a,b",
        "--rating",
        "4",
        "--label",
        "red",
    ]);
    assert_eq!(out["result"]["changed"], 2);
    work.ok(&["photo", "rate", "catalog.pen", "c", "--pick", "reject"]);
    assert_eq!(ids(&work.search("rating>=4")), ["a", "b"]);
    assert_eq!(ids(&work.search("pick:reject")), ["c"]);
    assert_eq!(ids(&work.search("label:red rating<=4")), ["a", "b"]);
    // A query selects too; default values are stored as absence.
    work.ok(&[
        "photo",
        "rate",
        "catalog.pen",
        "pick:reject",
        "--pick",
        "none",
    ]);
    assert!(work.photo("c").get("pick").is_none());
    work.ok(&["photo", "rate", "catalog.pen", "b", "--rating", "0"]);
    assert!(work.photo("b").get("rating").is_none());

    work.ok(&[
        "photo",
        "keyword",
        "catalog.pen",
        "a,b",
        "--add",
        "Blue Hour,harbor",
    ]);
    // Keywords compare ignoring case and keep the stored spelling.
    let out = work.ok(&["photo", "keyword", "catalog.pen", "b", "--add", "blue hour"]);
    assert_eq!(out["result"]["changed"], 0);
    assert_eq!(work.photo("b")["keywords"], json!(["Blue Hour", "harbor"]));
    assert_eq!(ids(&work.search("keyword:\"blue hour\"")), ["a", "b"]);
    work.ok(&["photo", "keyword", "catalog.pen", "a", "--remove", "HARBOR"]);
    assert_eq!(work.photo("a")["keywords"], json!(["Blue Hour"]));
    work.ok(&["photo", "keyword", "catalog.pen", "a", "--clear"]);
    assert!(work.photo("a").get("keywords").is_none());

    // Refusals leave the document unchanged.
    assert!(work
        .refuse(&["photo", "rate", "catalog.pen", "a", "--rating", "6"])
        .contains("[invalid-input]"));
    assert!(work
        .refuse(&["photo", "rate", "catalog.pen", "a"])
        .contains("--rating, --pick or --label"));
    assert!(work
        .refuse(&["photo", "rate", "catalog.pen", "a,zz", "--rating", "1"])
        .contains("[missing-resource]"));
    assert!(work
        .refuse(&["photo", "rate", "catalog.pen", "rating>=5", "--rating", "1"])
        .contains("matches no photos"));
    assert!(work
        .refuse(&["photo", "keyword", "catalog.pen", "a", "--add", "bad\tword"])
        .contains("[invalid-input]"));

    // A document with keywords that differ only in case is malformed.
    let mut doc = work.doc();
    doc["photography"]["photos"][0]["keywords"] = json!(["Sky", "sky"]);
    std::fs::write(work.0.join("bad.pen"), serde_json::to_vec(&doc).unwrap()).unwrap();
    let (ok, _, error) = work.run(&["photo", "search", "bad.pen"]);
    assert!(!ok && error.contains("unique ignoring case"), "{error}");
}

#[test]
fn search_is_paginated_in_catalog_order_and_rejects_unknown_terms() {
    let work = Workspace::new("search");
    let all = work.search("");
    assert_eq!(ids(&all), ["a", "b", "c"]);
    assert_eq!(all["matches"], 3);
    let row = &all["photos"][0];
    assert_eq!(row["kind"], "raw");
    assert_eq!(
        (row["width"].clone(), row["height"].clone()),
        (json!(64), json!(16))
    );
    assert_eq!(row["rating"], 0);
    assert_eq!(row["variants"], 1);

    let page = work.ok(&[
        "photo",
        "search",
        "catalog.pen",
        "",
        "--limit",
        "2",
        "--offset",
        "1",
    ]);
    assert_eq!(ids(&page), ["b", "c"]);
    assert_eq!(
        (&page["matches"], &page["returned"], &page["has_more"]),
        (&json!(3), &json!(2), &json!(false))
    );
    let page = work.ok(&["photo", "search", "catalog.pen", "", "--limit", "2"]);
    assert_eq!(page["has_more"], true);
    let past = work.ok(&["photo", "search", "catalog.pen", "", "--offset", "9"]);
    assert_eq!(
        (&past["returned"], &past["has_more"]),
        (&json!(0), &json!(false))
    );

    assert_eq!(
        ids(&work.search("camera:\"variant test\"")),
        ["a", "b", "c"]
    );
    assert_eq!(ids(&work.search("ID:?")).len(), 3);
    assert_eq!(ids(&work.search("id:b*")), ["b"]);
    assert!(ids(&work.search("camera:canon")).is_empty());
    work.ok(&["photo", "variant", "add", "catalog.pen", "c", "warm"]);
    assert_eq!(ids(&work.search("has:variants")), ["c"]);

    for (query, message) in [
        ("color:red", "unknown query term"),
        ("rating>=9", "rating is 0–5"),
        ("keyword>=x", "takes only ':'"),
        ("captured>=2024", "YYYY-MM-DD"),
        ("keyword:\"open", "unterminated quote"),
        ("collection:none", "[missing-resource]"),
    ] {
        let (ok, _, error) = work.run(&["photo", "search", "catalog.pen", query]);
        assert!(!ok && error.contains(message), "{query}: {error}");
    }
    let (ok, _, _) = work.run(&["photo", "search", "catalog.pen", "", "--limit", "0"]);
    assert!(!ok);
}

#[test]
fn stacks_and_collections_organize_without_moving_photos() {
    let work = Workspace::new("stacks");
    let out = work.ok(&["photo", "stack", "add", "catalog.pen", "burst", "a,b"]);
    assert_eq!(out["result"]["top"], "a");
    assert!(work
        .refuse(&["photo", "stack", "add", "catalog.pen", "other", "b,c"])
        .contains("already in stack burst"));
    assert!(work
        .refuse(&["photo", "stack", "add", "catalog.pen", "solo", "c"])
        .contains("at least two"));
    work.ok(&["photo", "stack", "top", "catalog.pen", "burst", "b"]);
    assert_eq!(
        work.doc()["photography"]["stacks"]["burst"]["photos"],
        json!(["b", "a"])
    );
    assert_eq!(ids(&work.search("stack:top")), ["b"]);
    assert_eq!(work.search("")["photos"][0]["stack"], "burst");

    // A stack shared by two stacks is malformed.
    let mut doc = work.doc();
    doc["photography"]["stacks"]["twin"] = json!({"photos": ["a", "c"]});
    std::fs::write(work.0.join("bad.pen"), serde_json::to_vec(&doc).unwrap()).unwrap();
    let (ok, _, error) = work.run(&["photo", "search", "bad.pen"]);
    assert!(!ok && error.contains("at most one stack"), "{error}");

    work.ok(&["photo", "rate", "catalog.pen", "a,c", "--rating", "5"]);
    work.ok(&[
        "photo",
        "collection",
        "add",
        "catalog.pen",
        "picks",
        "--photos",
        "a,b",
    ]);
    let out = work.ok(&[
        "photo",
        "collection",
        "add",
        "catalog.pen",
        "best",
        "--query",
        "rating>=5",
        "--name",
        "Best",
    ]);
    assert_eq!(out["result"]["photos"], 2);
    work.ok(&[
        "photo",
        "collection",
        "add",
        "catalog.pen",
        "both",
        "--query",
        "collection:best collection:picks",
    ]);
    assert_eq!(ids(&work.search("collection:both")), ["a"]);
    // Smart collections follow the catalog.
    work.ok(&["photo", "rate", "catalog.pen", "b", "--rating", "5"]);
    assert_eq!(ids(&work.search("collection:both")), ["a", "b"]);
    work.ok(&[
        "photo",
        "collection",
        "update",
        "catalog.pen",
        "picks",
        "--add",
        "c",
        "--remove",
        "a",
    ]);
    assert_eq!(
        work.doc()["photography"]["collections"]["picks"]["photos"],
        json!(["b", "c"])
    );

    assert!(work
        .refuse(&[
            "photo",
            "collection",
            "add",
            "catalog.pen",
            "loop",
            "--query",
            "collection:loop",
        ])
        .contains("refers to itself"));
    assert!(work
        .refuse(&[
            "photo",
            "collection",
            "add",
            "catalog.pen",
            "bad",
            "--query",
            "nope:1"
        ])
        .contains("unknown query term"));
    assert!(work
        .refuse(&[
            "photo",
            "collection",
            "update",
            "catalog.pen",
            "best",
            "--add",
            "c"
        ])
        .contains("is smart"));
    assert!(work
        .refuse(&["photo", "collection", "remove", "catalog.pen", "best"])
        .contains("smart collection both refers"));
    work.ok(&["photo", "collection", "remove", "catalog.pen", "both"]);
    work.ok(&["photo", "collection", "remove", "catalog.pen", "best"]);
    work.ok(&["photo", "collection", "remove", "catalog.pen", "picks"]);
    work.ok(&["photo", "stack", "remove", "catalog.pen", "burst"]);
    let catalog = &work.doc()["photography"];
    assert!(catalog.get("collections").is_none() && catalog.get("stacks").is_none());
    assert_eq!(catalog["photos"].as_array().unwrap().len(), 3);

    // Settings sync accepts a query: each match's master.
    work.ok(&[
        "raw",
        "develop",
        "catalog.pen",
        "a",
        "--set",
        "tone.contrast=30",
    ]);
    work.ok(&["photo", "rate", "catalog.pen", "b,c", "--label", "green"]);
    let out = work.ok(&[
        "photo",
        "settings",
        "sync",
        "catalog.pen",
        "a",
        "--to",
        "label:green",
        "--groups",
        "tone",
    ]);
    let targets = out["result"]["targets"].as_array().unwrap();
    assert_eq!(targets.len(), 2);
    assert!(targets.iter().all(|t| t["variant"] == "master"));
    assert_eq!(work.develop_of("c", "master")["tone"]["contrast"], 30.0);
}

#[test]
fn contact_sheets_and_compare_write_png_and_pdf() {
    let work = Workspace::new("sheet");
    work.ok(&["photo", "variant", "add", "catalog.pen", "a", "warm"]);
    work.ok(&[
        "raw",
        "develop",
        "catalog.pen",
        "a",
        "--variant",
        "warm",
        "--set",
        "tone.exposure=1",
    ]);
    let before = work.bytes();
    let out = work.ok(&[
        "photo",
        "contact-sheet",
        "catalog.pen",
        "--selection",
        "id:*",
        "--columns",
        "2",
        "--cell",
        "64",
        "--caption",
        "{id} {rating}",
        "--out",
        "sheet.png",
    ]);
    assert_eq!(
        work.bytes(),
        before,
        "a contact sheet never changes the document"
    );
    assert_eq!(
        (out["columns"].clone(), out["rows"].clone()),
        (json!(2), json!(2))
    );
    let png = image::open(work.0.join("sheet.png")).unwrap();
    assert_eq!(
        (png.width(), png.height()),
        (
            out["width"].as_u64().unwrap() as u32,
            out["height"].as_u64().unwrap() as u32
        )
    );
    assert_eq!(out["cells"].as_array().unwrap().len(), 3);
    assert!(out["cells"][0].get("unavailable").is_none(), "{out}");
    // The bright photo's cell is brighter than the dark photo's cell.
    let rgb = png.to_rgb8();
    let luma = |x: u32, y: u32| -> u32 {
        let p = rgb.get_pixel(x, y);
        u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2])
    };
    // Sample the right half of each 64x16 thumbnail, centered vertically in its cell.
    assert!(luma(16 + 56, 16 + 32) > luma(16 + 80 + 56, 16 + 32));

    let out = work.ok(&[
        "photo",
        "compare",
        "catalog.pen",
        "a",
        "a/warm",
        "b",
        "--cell",
        "48",
        "--caption",
        "{variant}",
        "--out",
        "compare.pdf",
    ]);
    assert_eq!(
        (out["columns"].clone(), out["rows"].clone()),
        (json!(3), json!(1))
    );
    let pdf = std::fs::read(work.0.join("compare.pdf")).unwrap();
    assert!(pdf.starts_with(b"%PDF"));

    let (ok, _, error) = work.run(&[
        "photo",
        "compare",
        "catalog.pen",
        "a",
        "zz",
        "--out",
        "x.png",
    ]);
    assert!(!ok && error.contains("[missing-resource]"), "{error}");
    let (ok, _, error) = work.run(&[
        "photo",
        "compare",
        "catalog.pen",
        "a",
        "b",
        "--out",
        "x.jpg",
    ]);
    assert!(!ok && error.contains(".png or .pdf"), "{error}");
    let (ok, _, error) = work.run(&[
        "photo",
        "contact-sheet",
        "catalog.pen",
        "--selection",
        "a",
        "--columns",
        "40",
        "--out",
        "x.png",
    ]);
    assert!(!ok && error.contains("--columns"), "{error}");
    assert!(!work.0.join("x.png").exists() && !work.0.join("x.jpg").exists());
}
