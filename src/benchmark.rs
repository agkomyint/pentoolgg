//! Deterministic document and renderer benchmarks for release qualification.
use crate::{
    agent::{self, ObjectAction},
    document::{Document, Layer, Path, StrokeCap, StrokeJoin, Text},
    render,
};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{fs, hint::black_box, time::Instant};

#[derive(Debug, Clone, Copy)]
pub struct RenderBenchmark {
    pub layers: usize,
    pub objects: usize,
    pub paths_only: bool,
    pub scale: f32,
    pub warmups: usize,
    pub repetitions: usize,
}

#[derive(Default)]
struct Sample {
    read_parse: u128,
    validation: u128,
    svg: u128,
    fonts: u128,
    usvg: u128,
    raster: u128,
    encode: u128,
    write: u128,
    total: u128,
    output_bytes: usize,
    pixmap_bytes: u64,
}

/// Retained for compatibility with the v0.6.0 smoke benchmark CLI.
pub fn run(
    layer_count: usize,
    object_count: usize,
    png: bool,
    max_ms: Option<u128>,
) -> Result<Value> {
    if png {
        let report = run_render(RenderBenchmark {
            layers: layer_count,
            objects: object_count,
            paths_only: false,
            scale: 1.0,
            warmups: 0,
            repetitions: 1,
        })?;
        if let Some(budget) = max_ms {
            let total = report["timings_us"]["total"]["median"]
                .as_u64()
                .unwrap_or(0) as u128
                / 1000;
            if total > budget {
                bail!("benchmark exceeded {budget} ms budget: {total} ms");
            }
        }
        return Ok(report);
    }
    run_smoke(layer_count, object_count, max_ms)
}

pub fn run_render(c: RenderBenchmark) -> Result<Value> {
    validate_config(c)?;
    let source = serde_json::to_vec(&generate(c.layers, c.objects, c.paths_only))?;
    let fixture_path =
        std::env::temp_dir().join(format!("pentool-render-fixture-{}.pen", std::process::id()));
    fs::write(&fixture_path, &source)?;
    for _ in 0..c.warmups {
        black_box(render_sample(&fixture_path, c.scale, false)?);
    }
    let mut samples = Vec::with_capacity(c.repetitions);
    for _ in 0..c.repetitions {
        samples.push(render_sample(&fixture_path, c.scale, true)?);
    }
    let _ = fs::remove_file(&fixture_path);
    let last = samples.last().context("benchmark produced no samples")?;
    Ok(json!({
        "schema_version":1,"benchmark":"render",
        "fixture":{"layers":c.layers,"objects":c.objects,"kind":if c.paths_only{"paths"}else{"mixed-text"},"json_bytes":source.len()},
        "render":{"scale":c.scale,"source_width":1200,"source_height":800,"output_bytes":last.output_bytes,"pixmap_bytes":last.pixmap_bytes},
        "runs":{"warmups":c.warmups,"repetitions":c.repetitions},
        "timings_us":{
            "document_read_and_json_parse":stats(&samples,|s|s.read_parse),"validation":stats(&samples,|s|s.validation),
            "svg_construction":stats(&samples,|s|s.svg),"font_preparation_and_resolution":stats(&samples,|s|s.fonts),
            "usvg_parse":stats(&samples,|s|s.usvg),"rasterization":stats(&samples,|s|s.raster),
            "png_encoding":stats(&samples,|s|s.encode),"output_write":stats(&samples,|s|s.write),"total":stats(&samples,|s|s.total)},
        "peak_resident_bytes":peak_resident_bytes(),
        "note":"total contains each stage exactly once; fixture generation and warmups are excluded"
    }))
}

fn validate_config(c: RenderBenchmark) -> Result<()> {
    if c.layers == 0 || c.layers > 1000 || c.objects > 100_000 {
        bail!("benchmark supports 1–1,000 layers and at most 100,000 objects");
    }
    if c.repetitions == 0 || c.repetitions > 100 || c.warmups > 100 {
        bail!("warmups must be 0–100 and repetitions 1–100");
    }
    if !(0.1..=8.0).contains(&c.scale) {
        bail!("scale must be between 0.1 and 8");
    }
    Ok(())
}

fn render_sample(source: &std::path::Path, scale: f32, measure_write: bool) -> Result<Sample> {
    let total_at = Instant::now();
    let at = Instant::now();
    let input = fs::read(source)?;
    let doc: Document = serde_json::from_slice(&input)?;
    let read_parse = at.elapsed().as_micros();
    let at = Instant::now();
    doc.validate().map_err(anyhow::Error::msg)?;
    let validation = at.elapsed().as_micros();
    let rendered = render::to_png_profiled_validated(&doc, scale)?;
    let write = if measure_write {
        let path = std::env::temp_dir().join(format!(
            "pentool-render-benchmark-{}.png",
            std::process::id()
        ));
        let at = Instant::now();
        fs::write(&path, &rendered.bytes)?;
        let elapsed = at.elapsed().as_micros();
        let _ = fs::remove_file(path);
        elapsed
    } else {
        0
    };
    Ok(Sample {
        read_parse,
        validation,
        svg: rendered.timings.svg_construction_us,
        fonts: rendered.timings.font_preparation_us,
        usvg: rendered.timings.usvg_parse_us,
        raster: rendered.timings.rasterization_us,
        encode: rendered.timings.png_encoding_us,
        write,
        total: total_at.elapsed().as_micros(),
        output_bytes: rendered.bytes.len(),
        pixmap_bytes: rendered.pixmap_bytes,
    })
}

fn stats(samples: &[Sample], get: impl Fn(&Sample) -> u128) -> Value {
    let mut v: Vec<u128> = samples.iter().map(get).collect();
    v.sort_unstable();
    let median = if v.len() & 1 == 0 {
        (v[v.len() / 2 - 1] + v[v.len() / 2]) / 2
    } else {
        v[v.len() / 2]
    };
    let p95 = v[((v.len() * 95).div_ceil(100)).saturating_sub(1)];
    json!({"median":median,"p95":p95,"samples":v})
}

#[cfg(target_os = "linux")]
fn peak_resident_bytes() -> Option<u64> {
    fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|line| {
            line.strip_prefix("VmHWM:")
                .and_then(|v| v.split_whitespace().next()?.parse::<u64>().ok())
        })
        .map(|kb| kb * 1024)
}
#[cfg(target_os = "windows")]
fn peak_resident_bytes() -> Option<u64> {
    #[repr(C)]
    struct Counters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
        private_usage: usize,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> *mut std::ffi::c_void;
        fn K32GetProcessMemoryInfo(
            process: *mut std::ffi::c_void,
            counters: *mut Counters,
            size: u32,
        ) -> i32;
    }
    let mut counters = Counters {
        cb: std::mem::size_of::<Counters>() as u32,
        page_fault_count: 0,
        peak_working_set_size: 0,
        working_set_size: 0,
        quota_peak_paged_pool_usage: 0,
        quota_paged_pool_usage: 0,
        quota_peak_non_paged_pool_usage: 0,
        quota_non_paged_pool_usage: 0,
        pagefile_usage: 0,
        peak_pagefile_usage: 0,
        private_usage: 0,
    };
    // SAFETY: Windows fills the correctly sized process-memory structure for the current process.
    let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) };
    (ok != 0).then_some(counters.peak_working_set_size as u64)
}
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn peak_resident_bytes() -> Option<u64> {
    None
}

fn run_smoke(layers: usize, objects: usize, max_ms: Option<u128>) -> Result<Value> {
    validate_config(RenderBenchmark {
        layers,
        objects,
        paths_only: false,
        scale: 1.0,
        warmups: 0,
        repetitions: 1,
    })?;
    let started = Instant::now();
    let mut doc = generate(layers, objects, false);
    let bytes = serde_json::to_vec(&doc)?;
    let parsed: Document = serde_json::from_slice(&bytes)?;
    agent::DocumentIndex::build(&parsed)?;
    agent::inspect_paginated(&parsed, Some("object"), None, None, 0, 100)?;
    if objects > 0 {
        agent::apply(
            &mut doc,
            &ObjectAction::Set {
                id: "object-0".into(),
                layer: "layer-0".into(),
                d: None,
                stroke: None,
                width: None,
                fill: Some("#ff00ff".into()),
                cap: None,
                join: None,
                miter_limit: None,
                content: None,
                x: None,
                y: None,
                font: None,
                size: None,
                weight: None,
                italic: None,
                align: None,
                letter_spacing: None,
                line_height: None,
            },
        )?;
    }
    let elapsed = started.elapsed().as_millis();
    if let Some(limit) = max_ms {
        if elapsed > limit {
            bail!("benchmark exceeded {limit} ms budget: {elapsed} ms");
        }
    }
    Ok(
        json!({"benchmark":"document-smoke","layers":layers,"objects":objects,"json_bytes":bytes.len(),"milliseconds":{"total":elapsed}}),
    )
}

pub fn generate(layer_count: usize, object_count: usize, paths_only: bool) -> Document {
    let mut doc = Document::new(1200, 800);
    doc.layers.clear();
    for index in 0..layer_count {
        doc.layers.push(Layer {
            id: format!("layer-{index}"),
            name: format!("Layer {index}"),
            visible: true,
            locked: false,
            paths: vec![],
            texts: vec![],
        });
    }
    for index in 0..object_count {
        let layer = &mut doc.layers[index % layer_count];
        if !paths_only && index % 10 == 9 {
            layer.texts.push(Text {
                id: format!("object-{index}"),
                content: format!("Label {index}"),
                x: (index % 100) as f64 * 10.0,
                y: (index % 70) as f64 * 10.0 + 20.0,
                font_family: crate::fonts::DEFAULT_FAMILY.into(),
                font_size: 12.0,
                font_weight: 400,
                italic: false,
                fill: "#111827".into(),
                align: Default::default(),
                letter_spacing: 0.0,
                line_height: 1.2,
                transform: crate::document::identity(),
            });
        } else {
            let x = (index % 100) * 10;
            let y = (index % 70) * 10;
            layer.paths.push(Path {
                id: format!("object-{index}"),
                d: format!("M {x} {y} L {} {}", x + 6, y + 6),
                stroke: "#111827".into(),
                stroke_width: 1.0,
                stroke_linecap: StrokeCap::Butt,
                stroke_linejoin: StrokeJoin::Miter,
                stroke_miterlimit: 4.0,
                fill: "none".into(),
                closed: false,
            });
        }
    }
    doc
}

#[derive(Debug, Clone, Copy)]
pub struct ImageBenchmark {
    pub images: usize,
    pub source_size: u32,
    pub operations: usize,
    pub scale: f32,
    pub repetitions: usize,
}

fn micros(since: Instant) -> u64 {
    since.elapsed().as_micros() as u64
}

/// Image-aware benchmark: one reused source placed `images` times with an
/// operation stack, measured cold (empty processed cache) and warm.
pub fn run_image(c: ImageBenchmark) -> Result<Value> {
    if c.images == 0 || c.images > 1000 || !(1..=4096).contains(&c.source_size) {
        bail!("image benchmark supports 1–1,000 images and sources of 1–4,096 pixels");
    }
    if c.operations > 8 || c.repetitions == 0 || c.repetitions > 20 {
        bail!("operations must be 0–8 and repetitions 1–20");
    }
    if !(0.1..=4.0).contains(&c.scale) {
        bail!("scale must be between 0.1 and 4");
    }
    let dir = std::env::temp_dir().join(format!("pentool-image-bench-{}", std::process::id()));
    fs::create_dir_all(&dir)?;
    let result = image_bench_in(&dir, c);
    let _ = fs::remove_dir_all(&dir);
    result
}

fn image_bench_in(dir: &std::path::Path, c: ImageBenchmark) -> Result<Value> {
    let mut source = ::image::RgbaImage::new(c.source_size, c.source_size);
    for (x, y, px) in source.enumerate_pixels_mut() {
        *px = ::image::Rgba([(x % 256) as u8, (y % 256) as u8, ((x ^ y) % 256) as u8, 255]);
    }
    let mut png = std::io::Cursor::new(Vec::new());
    ::image::DynamicImage::ImageRgba8(source).write_to(&mut png, ::image::ImageFormat::Png)?;
    let png = png.into_inner();
    let mut raw = json!({"format":"pentool","version":4,"name":"bench",
        "pages":[{"id":"page-1","name":"Page 1","canvas":{"width":1200,"height":800,"background":"#ffffff"},
        "layers":[{"id":"layer-1","name":"Layer 1","visible":true,"locked":false,"nodes":[]}]}]});
    let kinds = [
        (
            "brightness-contrast",
            json!({"brightness":10,"contrast":10}),
        ),
        ("blur", json!({"radius":2})),
        ("hue-saturation", json!({"hue":15,"saturation":10})),
        ("sharpen", json!({"radius":1,"amount":50})),
    ];
    let columns = (c.images as f64).sqrt().ceil() as usize;
    let cell = 1200.0 / columns as f64;
    for i in 0..c.images {
        crate::image::add(
            &mut raw,
            None,
            "layer-1",
            &format!("img-{i}"),
            &png,
            crate::image::embedded_storage(&png),
            (i % columns) as f64 * cell,
            (i / columns) as f64 * cell.min(800.0 / columns as f64),
            cell,
            cell.min(800.0 / columns as f64),
            crate::image::Fit::Cover,
        )?;
        for k in 0..c.operations {
            let (kind, params) = &kinds[k % kinds.len()];
            let map = params.as_object().cloned().unwrap_or_default();
            crate::image::op_add(
                &mut raw,
                None,
                &format!("img-{i}"),
                kind,
                Some(&format!("op-{i}-{k}")),
                None,
                crate::image::OpParams(map),
            )?;
        }
    }
    let path = dir.join("bench.pen");
    fs::write(&path, serde_json::to_vec(&raw)?)?;
    let mut runs = Vec::new();
    for run in 0..c.repetitions + 1 {
        let cold = run == 0;
        if cold {
            let _ = fs::remove_dir_all(dir.join(".pentool"));
        }
        let total = Instant::now();
        let at = Instant::now();
        let scene = crate::image::to_svg(&raw, &path, None)?;
        let compose = micros(at);
        let at = Instant::now();
        let png = render::scene_to_png(&scene, c.scale)?;
        let raster_encode = micros(at);
        let at = Instant::now();
        let out = dir.join("out.png");
        fs::write(&out, &png)?;
        runs.push(json!({"cache":if cold {"cold"} else {"warm"},
            "decode_process_compose_us":compose,"rasterize_encode_us":raster_encode,
            "write_us":micros(at),"total_us":micros(total),"output_bytes":png.len()}));
    }
    Ok(json!({"schema_version":1,"benchmark":"image","codec":"png",
        "fixture":{"images":c.images,"reuse_count":c.images,"source_width":c.source_size,
            "source_height":c.source_size,"operations_per_image":c.operations,"document_bytes":fs::metadata(&path)?.len()},
        "render":{"scale":c.scale},"runs":runs,"peak_resident_bytes":peak_resident_bytes()}))
}
