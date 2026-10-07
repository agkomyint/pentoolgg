//! v0.10.0 item 14: raster benchmarks. Ignored by default because timings are only
//! meaningful on a release build; run
//! `cargo test --release --test raster_perf -- --ignored --nocapture` and record the
//! printed lines with the machine description (see docs/performance.md).
use pentool::raster;
use serde_json::{json, Value};
use std::time::Instant;

fn sparse(width: u32, height: u32) -> Value {
    let mut raw = pentool::scene::new_document(width.min(4096), height.min(4096));
    raster::add(&mut raw, None, None, "p", 0.0, 0.0, width, height).unwrap();
    raw
}

fn stroke(raw: &mut Value, x: f64, y: f64, size: f64) -> Value {
    let samples: Vec<Value> = (0..40)
        .map(|i| json!({"x": x + i as f64 * 2.0, "y": y + (i as f64 * 0.3).sin() * 6.0, "pressure": 0.4 + i as f64 * 0.01}))
        .collect();
    raster::apply(
        raw,
        None,
        "p",
        "stroke",
        &json!({"samples": samples, "brush": {"size": size, "pressure_size": true}, "color": "#a0522d"}),
    )
    .unwrap()
}

#[test]
#[ignore]
fn bench_stroke_latency_dirty_tiles_and_checkpoint_cadence() {
    for (w, h) in [(1000, 1000), (4000, 5000), (8000, 8000)] {
        let mut raw = sparse(w, h);
        let mut times = Vec::new();
        let mut dirty = 0usize;
        for i in 0..300 {
            let start = Instant::now();
            let r = stroke(
                &mut raw,
                20.0 + (i % 40) as f64 * 15.0,
                20.0 + (i / 40) as f64 * 40.0,
                24.0,
            );
            times.push(start.elapsed().as_secs_f64() * 1000.0);
            dirty = dirty.max(r["tiles_changed"].as_u64().unwrap() as usize);
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let info = raster::info(&raw, None, "p").unwrap();
        println!(
            "stroke {w}x{h}: p50 {:.2} ms, p95 {:.2} ms, max dirty tiles/stroke {dirty}, tiles {}, journal {}",
            times[times.len() / 2],
            times[times.len() * 95 / 100],
            info["tiles"],
            info["journal_entries"]
        );
    }
}

#[test]
#[ignore]
fn bench_ten_thousand_strokes_reopen_and_replay() {
    let mut raw = sparse(1024, 1024);
    let start = Instant::now();
    for i in 0..10_000u32 {
        stroke(
            &mut raw,
            20.0 + (i % 50) as f64 * 18.0,
            20.0 + (i / 50 % 100) as f64 * 9.0,
            10.0,
        );
        if i % 500 == 499 {
            println!("  {} strokes: {:.1}s", i + 1, start.elapsed().as_secs_f64());
        }
    }
    let paint = start.elapsed();
    let node = raw["pages"][0]["layers"][0]["nodes"][0].clone();
    let journal = node["journal"].as_array().unwrap().len();
    let start = Instant::now();
    let report = raster::verify(&raw, None, None, true).unwrap();
    let verify = start.elapsed();
    let start = Instant::now();
    let replayed = raster::replay(&raw, &node).unwrap();
    let replay = start.elapsed();
    assert_eq!(report["ok"], true);
    assert!(journal <= 256);
    assert_eq!(
        replayed.tile_map_hash(),
        raster::info(&raw, None, "p").unwrap()["tile_map_sha256"]
            .as_str()
            .unwrap()
    );
    println!(
        "10,000 strokes: paint {:.1}s, journal {journal}, verify --replay {:.0} ms, replay {:.0} ms, document {:.1} MiB",
        paint.as_secs_f64(),
        verify.as_secs_f64() * 1000.0,
        replay.as_secs_f64() * 1000.0,
        serde_json::to_vec(&raw).unwrap().len() as f64 / 1048576.0
    );
}

#[test]
#[ignore]
fn bench_compositing_and_merge() {
    let mut raw = sparse(2000, 2000);
    for i in 0..60 {
        stroke(
            &mut raw,
            20.0 + (i % 10) as f64 * 150.0,
            30.0 + (i / 10) as f64 * 200.0,
            60.0,
        );
    }
    let dir = std::env::temp_dir().join("pentool-perf");
    std::fs::create_dir_all(&dir).unwrap();
    let start = Instant::now();
    let image = pentool::composite::render(&raw, &dir.join("p.pen"), None, 1.0).unwrap();
    println!(
        "composite render {}x{}: {:.0} ms",
        image.width(),
        image.height(),
        start.elapsed().as_secs_f64() * 1000.0
    );
}
