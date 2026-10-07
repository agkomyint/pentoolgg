//! v0.10.0 item 14: deterministic fuzzing and seam/alpha/pressure/replay goldens.
//!
//! The fuzzers use a fixed splitmix64 seed, so a failure reproduces exactly. They
//! assert only that malformed input is rejected or accepted without a panic and that a
//! rejected edit leaves the document unchanged; they never compare pixels.
use pentool::raster;
use serde_json::{json, Value};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn number(&mut self) -> Value {
        match self.below(9) {
            0 => json!(-1),
            1 => json!(0),
            2 => json!(1e308),
            3 => json!(-1e308),
            4 => json!(1e-308),
            5 => json!(self.below(1 << 20)),
            6 => json!(self.unit() * 1000.0 - 500.0),
            7 => json!(f64::MAX),
            _ => json!(self.unit()),
        }
    }
    fn scalar(&mut self) -> Value {
        match self.below(8) {
            0 => Value::Null,
            1 => json!(true),
            2 => json!("text"),
            3 => json!([]),
            4 => json!({}),
            _ => self.number(),
        }
    }
}

fn layer(width: u32, height: u32) -> Value {
    let mut raw = pentool::scene::new_document(width, height);
    raster::add(&mut raw, None, None, "p", 0.0, 0.0, width, height).unwrap();
    raw
}

fn stroke(raw: &mut Value, points: &[(f64, f64, f64)], size: f64, color: &str) -> Value {
    let samples: Vec<Value> = points
        .iter()
        .map(|&(x, y, pressure)| json!({"x": x, "y": y, "pressure": pressure}))
        .collect();
    raster::apply(
        raw,
        None,
        "p",
        "stroke",
        &json!({"samples": samples, "brush": {"size": size, "hardness": 1.0, "pressure_size": true}, "color": color}),
    )
    .unwrap()
}

#[test]
fn fuzzed_sample_streams_never_panic() {
    let mut rng = Rng(0x5eed_0001);
    for _ in 0..3000 {
        let count = rng.below(6);
        let events: Vec<Value> = (0..count)
            .map(|_| {
                let mut event = serde_json::Map::new();
                for key in [
                    "x", "y", "pressure", "tilt", "azimuth", "twist", "t", "velocity",
                ] {
                    if rng.below(3) != 0 {
                        event.insert(key.into(), rng.scalar());
                    }
                }
                if rng.below(20) == 0 {
                    event.insert("unknown".into(), rng.scalar());
                }
                match rng.below(12) {
                    0 => rng.scalar(),
                    _ => Value::Object(event),
                }
            })
            .collect();
        let input = if rng.below(15) == 0 {
            rng.scalar()
        } else {
            Value::Array(events)
        };
        let _ = raster::normalize_input(&input);
        let _ = raster::parse_samples(&input);
    }
}

#[test]
fn fuzzed_brush_json_never_panics_and_never_corrupts_a_document() {
    let mut rng = Rng(0x5eed_0002);
    let keys = [
        "kind",
        "size",
        "hardness",
        "spacing",
        "opacity",
        "flow",
        "angle",
        "roundness",
        "scatter",
        "pressure_size",
        "pressure_flow",
        "min_size",
        "smoothing",
        "buildup",
        "tip",
        "dynamics",
        "strength",
        "tolerance",
        "range",
        "mode",
        "texture",
        "tone",
        "preset",
        "bogus",
    ];
    let mut raw = layer(64, 64);
    for _ in 0..2000 {
        let mut brush = serde_json::Map::new();
        for _ in 0..rng.below(6) {
            let key = keys[rng.below(keys.len() as u64) as usize];
            let value = if key == "kind" && rng.below(2) == 0 {
                json!(["hard-round", "soft-round", "pixel", "textured", "x"][rng.below(5) as usize])
            } else {
                rng.scalar()
            };
            brush.insert(key.into(), value);
        }
        let _ = raster::Brush::parse(&Value::Object(brush.clone()));
        let before = raw.clone();
        let result = raster::apply(
            &mut raw,
            None,
            "p",
            "stroke",
            &json!({"samples":[{"x":4,"y":4},{"x":40,"y":30}],"brush":Value::Object(brush)}),
        );
        if result.is_err() {
            // `apply` may have touched its argument; the batch/endpoint callers clone.
            raw = before;
        }
        assert!(pentool::transaction::validate_value(&raw).is_ok());
    }
}

#[test]
fn fuzzed_selection_edges_are_rejected_or_valid() {
    let mut rng = Rng(0x5eed_0003);
    let mut raw = layer(40, 30);
    for _ in 0..800 {
        let mut next = raw.clone();
        let action = ["select-marquee", "select-lasso", "select-wand"][rng.below(3) as usize];
        let shape = ["rect", "ellipse", "x"][rng.below(3) as usize];
        let mode = ["replace", "add", "subtract", "intersect"][rng.below(4) as usize];
        let args = match action {
            "select-marquee" => {
                let (x, y, w, h) = (rng.number(), rng.number(), rng.number(), rng.number());
                json!({"x": x, "y": y, "width": w, "height": h, "shape": shape,
                       "feather": rng.below(40), "mode": mode})
            }
            "select-lasso" => {
                let points: Vec<Value> = (0..rng.below(9))
                    .map(|_| json!([rng.number(), rng.number()]))
                    .collect();
                json!({"points": points, "mode": "replace", "feather": rng.below(10)})
            }
            _ => {
                let (x, y, t, g) = (
                    rng.below(100),
                    rng.below(100),
                    rng.below(300),
                    rng.below(12),
                );
                json!({"x": x, "y": y, "tolerance": t, "gap": g})
            }
        };
        if raster::apply(&mut next, None, "p", action, &args).is_ok() {
            assert!(
                pentool::transaction::validate_value(&next).is_ok(),
                "{action} {args}"
            );
            raw = next;
        }
    }
}

#[test]
fn corrupted_journals_and_tiles_are_reported_not_panicked_on() {
    let mut rng = Rng(0x5eed_0004);
    let mut base = layer(300, 100);
    for i in 0..4 {
        stroke(
            &mut base,
            &[(10.0 + i as f64 * 20.0, 10.0, 0.5), (280.0, 60.0, 1.0)],
            9.0,
            "#336699",
        );
    }
    assert!(raster::verify(&base, None, None, true).unwrap()["ok"] == true);
    for _ in 0..300 {
        let mut raw = base.clone();
        let node = &mut raw["pages"][0]["layers"][0]["nodes"][0];
        match rng.below(6) {
            0 => {
                let journal = node["journal"].as_array_mut().unwrap();
                let at = rng.below(journal.len() as u64) as usize;
                journal.remove(at);
            }
            1 => {
                let journal = node["journal"].as_array_mut().unwrap();
                let at = rng.below(journal.len() as u64) as usize;
                let key =
                    ["samples", "brush", "color", "bounds", "seed", "blend"][rng.below(6) as usize];
                journal[at][key] = rng.scalar();
            }
            2 => {
                let journal = node["journal"].as_array_mut().unwrap();
                let (a, b) = (
                    rng.below(journal.len() as u64) as usize,
                    rng.below(journal.len() as u64) as usize,
                );
                journal.swap(a, b);
            }
            3 => node["checkpoint"] = rng.scalar(),
            4 => node["tiles"] = rng.scalar(),
            _ => node["journal"] = rng.scalar(),
        }
        let _ = raster::verify(&raw, None, None, true);
        let _ = raster::replay(&raw, &raw["pages"][0]["layers"][0]["nodes"][0].clone());
    }
}

#[test]
fn fuzzed_brush_assets_and_presets_never_panic() {
    let mut rng = Rng(0x5eed_0005);
    let mut raw = layer(16, 16);
    for _ in 0..1500 {
        let length = rng.below(300) as usize;
        let mut bytes: Vec<u8> = (0..length).map(|_| rng.next() as u8).collect();
        if rng.below(2) == 0 && length > 24 {
            // A plausible GIMP header with hostile numbers.
            bytes[..4].copy_from_slice(&(28u32 + rng.below(8) as u32).to_be_bytes());
            bytes[4..8].copy_from_slice(&(rng.below(3) as u32).to_be_bytes());
        }
        let _ = raster::preset_import_gbr(&mut raw.clone(), &bytes, "fuzz");
        let myb = json!({"version": rng.scalar(), "settings": rng.scalar()}).to_string();
        let _ = raster::preset_import_mypaint(&mut raw.clone(), &myb, "fuzz");
        let _ = raster::preset_import_mypaint(
            &mut raw.clone(),
            &String::from_utf8_lossy(&bytes),
            "fuzz",
        );
        let _ = raster::preset_import(&mut raw.clone(), &rng.scalar(), None, false);
        let _ = raster::preset_add(&mut raw.clone(), "p1", &rng.scalar(), None, false);
    }
    // A hostile import must never leave a half-written document.
    let before = raw.clone();
    assert!(raster::preset_import_gbr(&mut raw, &[0xff; 40], "x").is_err());
    assert_eq!(raw, before);
}

fn render(raw: &Value) -> image::RgbaImage {
    let dir = std::env::temp_dir().join("pentool-hardening");
    std::fs::create_dir_all(&dir).unwrap();
    pentool::composite::render(raw, &dir.join("render.pen"), None, 1.0).unwrap()
}

#[test]
fn strokes_across_tile_seams_have_no_gaps_or_double_application() {
    // The 256-px tile edge is at x = 256 and y = 256.
    let mut raw = layer(600, 600);
    stroke(
        &mut raw,
        &[(200.0, 100.0, 1.0), (330.0, 100.0, 1.0)],
        10.0,
        "#ff0000",
    );
    stroke(
        &mut raw,
        &[(100.0, 200.0, 1.0), (100.0, 330.0, 1.0)],
        10.0,
        "#ff0000",
    );
    let image = render(&raw);
    for x in 200..=330 {
        let p = image.get_pixel(x, 100).0;
        assert!(
            p[0] > 200 && p[1] < 60 && p[3] == 255,
            "gap at x={x}: {p:?}"
        );
    }
    for y in 200..=330 {
        let p = image.get_pixel(100, y).0;
        assert!(
            p[0] > 200 && p[1] < 60 && p[3] == 255,
            "gap at y={y}: {p:?}"
        );
    }
    // Pixels beside the seam match pixels away from it: no doubled or missing edge row.
    assert_eq!(image.get_pixel(255, 98), image.get_pixel(200, 98));
    assert_eq!(image.get_pixel(256, 98), image.get_pixel(200, 98));
    assert_eq!(image.get_pixel(257, 102), image.get_pixel(210, 102));
}

#[test]
fn soft_edges_and_pressure_produce_clean_alpha() {
    let mut raw = layer(120, 40);
    let samples: Vec<Value> = (0..=20)
        .map(|i| json!({"x": 10 + i * 5, "y": 20, "pressure": 0.1 + 0.045 * i as f64}))
        .collect();
    raster::apply(
        &mut raw,
        None,
        "p",
        "stroke",
        &json!({"samples": samples, "brush": {"kind":"soft-round","size":14,"hardness":0.2,"pressure_size":true}, "color":"#000000"}),
    )
    .unwrap();
    let image = render(&raw);
    // Straight-alpha output over an opaque white page: no pixel may be darker than the
    // brush color, and the stroke widens with pressure.
    assert!(image.pixels().all(|p| p.0[3] == 255));
    let width_at = |x: u32| {
        (0..40)
            .filter(|&y| image.get_pixel(x, y).0[0] < 128)
            .count()
    };
    assert!(
        width_at(100) > width_at(15),
        "{} vs {}",
        width_at(100),
        width_at(15)
    );
    assert!(
        image.pixels().any(|p| p.0[0] > 0 && p.0[0] < 255),
        "soft edge exists"
    );
}

#[test]
fn replay_of_many_strokes_matches_the_live_surface_and_stays_bounded() {
    let mut rng = Rng(0x5eed_0006);
    let mut raw = layer(300, 200);
    for i in 0..400 {
        let a = (
            rng.unit() * 290.0,
            rng.unit() * 190.0,
            0.2 + rng.unit() * 0.8,
        );
        let b = (
            rng.unit() * 290.0,
            rng.unit() * 190.0,
            0.2 + rng.unit() * 0.8,
        );
        let color = format!("#{:06x}", rng.below(0x100_0000));
        stroke(&mut raw, &[a, b], 3.0 + (i % 7) as f64, &color);
    }
    let node = raw["pages"][0]["layers"][0]["nodes"][0].clone();
    assert!(
        node["journal"].as_array().unwrap().len() <= 256,
        "the journal is bounded by checkpoints"
    );
    let report = raster::verify(&raw, None, None, true).unwrap();
    assert_eq!(report["ok"], true, "{report}");
    let replayed = raster::replay(&raw, &node).unwrap();
    let info = raster::info(&raw, None, "p").unwrap();
    assert_eq!(
        replayed.tile_map_hash(),
        info["tile_map_sha256"].as_str().unwrap()
    );
}

#[test]
fn extreme_selection_geometry_does_not_overflow() {
    // Regression: a marquee or lasso reaching toward f64::MAX overflowed the pixel range.
    let mut raw = layer(32, 32);
    let before = raw.clone();
    let marquee = json!({"x": -1e300, "y": -1e300, "width": 1.7e308, "height": 1.7e308});
    assert!(raster::apply(&mut raw, None, "p", "select-marquee", &marquee).is_ok());
    let mut raw = before;
    let lasso = json!({"points": [[0, -1e308], [1e308, 0], [0, 1e308], [-1e308, 0]]});
    assert!(raster::apply(&mut raw, None, "p", "select-lasso", &lasso).is_ok());
}
