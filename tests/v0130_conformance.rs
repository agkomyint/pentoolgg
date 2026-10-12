//! v0.13.0 task 14: conformance goldens, hostile input, locks, rollback and
//! bounded long sessions. Everything here runs in-process on the integer pixel
//! path, so the pinned digests are identical on every platform.
use pentool::{paint_plan, raster};
use serde_json::{json, Value};

fn document(size: u32) -> Value {
    let mut raw = pentool::scene::new_document(size, size);
    raster::add(&mut raw, None, None, "p", 0.0, 0.0, size, size).unwrap();
    raw
}

fn plan(passes: usize) -> Value {
    let list: Vec<Value> = (0..passes)
        .map(|i| {
            let y = 4 + (i % 50) as u32;
            json!({"id": format!("s{i}"), "target": "p", "action": "stroke-path",
                   "args": {"path": format!("M2 {y} L60 {y}"),
                            "brush": {"size": 3, "hardness": 1, "spacing": 0.5},
                            "color": "#336699", "seed": i}})
        })
        .collect();
    json!({"schema": 1, "engine": 1, "id": "c", "passes": list})
}

fn tile_hash(raw: &Value) -> String {
    raster::layer_surface(raw, "p").unwrap().tile_map_hash()
}

/// xorshift: deterministic, dependency-free fuzz source.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

#[test]
fn plans_replay_to_identical_pixels_and_reports() {
    let (mut a, mut b) = (document(64), document(64));
    let ra = paint_plan::execute(&mut a, None, &plan(8)).unwrap();
    let rb = paint_plan::execute(&mut b, None, &plan(8)).unwrap();
    assert_eq!(ra, rb);
    assert_eq!(tile_hash(&a), tile_hash(&b));
    assert_ne!(tile_hash(&a), tile_hash(&document(64)));
    // A reordered plan is a different, equally reproducible plan.
    let mut reordered = plan(8);
    reordered["passes"].as_array_mut().unwrap().reverse();
    let mut c = document(64);
    let rc = paint_plan::execute(&mut c, None, &reordered).unwrap();
    assert_ne!(ra["plan_hash"], rc["plan_hash"]);
}

#[test]
fn pinned_golden_digest_for_the_reference_plan() {
    let mut raw = document(64);
    let report = paint_plan::execute(&mut raw, None, &plan(4)).unwrap();
    assert_eq!(
        report["plan_hash"],
        "sha256:8657d69dbd979f50ff9170c9de77a4cc3471bc7dd7802c7910e32d607d8cd5f3",
        "plan canonicalization changed"
    );
    assert_eq!(
        tile_hash(&raw),
        "sha256:ca98063d9dde894c7c171e12b695e7846088c4e056f18cdf91cf13c238ed296e",
        "pixels changed"
    );
}

#[test]
fn hostile_dimensions_and_mutated_plans_fail_cleanly_without_mutation() {
    for (w, h) in [(0, 8), (8, 0), (u32::MAX, u32::MAX), (1 << 20, 1 << 20)] {
        let mut raw = pentool::scene::new_document(8, 8);
        let before = raw.clone();
        assert!(
            raster::add(&mut raw, None, None, "p", 0.0, 0.0, w, h).is_err(),
            "{w}x{h}"
        );
        assert_eq!(raw, before);
    }
    let base = plan(3);
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let junk = [
        json!(null),
        json!(-1),
        json!(1e300),
        json!("\u{0}"),
        json!([]),
        json!({}),
        json!(u64::MAX),
        json!("M 0 0 C nan nan"),
        json!(true),
    ];
    for round in 0..300 {
        let mut candidate = base.clone();
        // Overwrite one randomly chosen value anywhere in the plan.
        let mut paths: Vec<Vec<String>> = Vec::new();
        fn walk(v: &Value, at: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
            match v {
                Value::Object(m) => m.iter().for_each(|(k, c)| {
                    at.push(k.clone());
                    out.push(at.clone());
                    walk(c, at, out);
                    at.pop();
                }),
                Value::Array(a) => a.iter().enumerate().for_each(|(i, c)| {
                    at.push(i.to_string());
                    out.push(at.clone());
                    walk(c, at, out);
                    at.pop();
                }),
                _ => {}
            }
        }
        walk(&candidate, &mut Vec::new(), &mut paths);
        let path = &paths[(rng.next() % paths.len() as u64) as usize];
        let replacement = junk[(rng.next() % junk.len() as u64) as usize].clone();
        let mut slot = &mut candidate;
        for key in path {
            slot = match slot {
                Value::Object(m) => m.get_mut(key).unwrap(),
                Value::Array(a) => &mut a[key.parse::<usize>().unwrap()],
                _ => unreachable!(),
            };
        }
        *slot = replacement;
        let mut raw = document(64);
        let before = raw.clone();
        // Must never panic; a rejected plan leaves the document untouched.
        if paint_plan::execute(&mut raw, None, &candidate).is_err() {
            assert_eq!(
                raw, before,
                "round {round}: failed plan mutated the document"
            );
        }
    }
}

#[test]
fn locked_layers_and_missing_targets_reject_atomically() {
    let mut raw = document(64);
    raw["pages"][0]["layers"][0]["locked"] = json!(true);
    let before = raw.clone();
    let err = paint_plan::execute(&mut raw, None, &plan(2)).unwrap_err();
    assert!(format!("{err:#}").contains("lock"), "{err:#}");
    assert_eq!(raw, before);
    let mut missing = plan(2);
    missing["passes"][1]["target"] = json!("ghost");
    let mut raw = document(64);
    let before = raw.clone();
    assert!(paint_plan::execute(&mut raw, None, &missing).is_err());
    assert_eq!(raw, before);
}

#[test]
fn plan_limits_are_hard_ceilings() {
    let mut raw = document(64);
    let err = paint_plan::execute(&mut raw, None, &plan(65)).unwrap_err();
    assert!(err.to_string().contains("needs 1-"), "{err}");
    let mut raised = plan(2);
    raised["limits"] = json!({"max_changed_pixels": 1u64 << 40});
    assert!(paint_plan::execute(&mut raw, None, &raised).is_err());
}

/// One session of `rounds` 64-pass plans on a single layer.
fn long_session(rounds: usize) {
    let mut raw = document(64);
    for round in 0..rounds {
        let mut p = plan(64);
        p["id"] = json!(format!("round-{round}"));
        let report = paint_plan::execute(&mut raw, None, &p).unwrap();
        assert_eq!(report["totals"]["passes"], 64);
    }
    let surface = raster::layer_surface(&raw, "p").unwrap();
    assert_eq!(surface.tiles.len(), 1, "a 64px layer owns one tile");
    // No per-stroke history is embedded in the document.
    assert!(serde_json::to_vec(&raw).unwrap().len() < 400_000);
}

#[test]
fn long_sessions_stay_bounded() {
    long_session(8);
}

/// 157 plans x 64 passes = 10,048 strokes. Slow in debug builds, so run it with
/// `cargo test --release --test v0130_conformance -- --ignored`.
#[test]
#[ignore]
fn ten_thousand_strokes_stay_bounded() {
    long_session(157);
}

#[test]
fn hostile_colors_references_and_fields_fail_cleanly() {
    use pentool::paint_review::{compare, CompareOptions};
    // Palette colors: never a panic, always Ok or a structured error.
    let mut rng = Rng(0xDEAD_BEEF_CAFE_F00D);
    for _ in 0..500 {
        let len = (rng.next() % 12) as usize;
        let text: String = (0..len)
            .map(|_| char::from_u32((rng.next() % 0x2fff) as u32 + 1).unwrap_or('?'))
            .collect();
        let _ = pentool::paint_palette::parse_color(&text);
    }
    assert!(pentool::paint_palette::parse_color("#zzzzzz").is_err());

    // Damaged references: truncated, garbage, and empty files are errors that
    // leave the document untouched.
    let dir = std::env::temp_dir().join(format!("pentool-v0130-hostile-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let raw = document(32);
    let before = raw.clone();
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(16, 16, image::Rgba([9, 9, 9, 255]))
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let good = png.into_inner();
    let cases: [(&str, Vec<u8>); 3] = [
        ("truncated.png", good[..good.len() / 2].to_vec()),
        (
            "garbage.png",
            (0..200u32).map(|i| (i * 37 % 251) as u8).collect(),
        ),
        ("empty.png", Vec::new()),
    ];
    for (name, bytes) in cases {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        let result = compare(
            &raw,
            &dir.join("doc.pen"),
            None,
            CompareOptions {
                scope: "page".into(),
                rect: None,
                selection: None,
                reference_image: Some(path.to_string_lossy().into_owned()),
                reference_document: None,
                reference_scope: None,
                background: [255, 255, 255],
                heatmap: None,
            },
        );
        assert!(result.is_err(), "{name} was accepted");
    }
    let _ = std::fs::remove_dir_all(&dir);

    // Hostile field specs and plan arguments are rejected, not run.
    for field in [
        json!({}),
        json!({"method": "nope"}),
        json!({"scale": -1}),
        json!(null),
    ] {
        assert!(
            pentool::paint_fields::preview_value(&raw, "p", &field, None, None, &json!({}), 0, 10)
                .is_err(),
            "{field}"
        );
    }
    assert_eq!(raw, before);
}
