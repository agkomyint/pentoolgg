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
                blend: None,
                blend_space: None,
                opacity: None,
                content_opacity: None,
                isolation: None,
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
                width: None,
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

#[derive(Debug, Clone, Copy)]
pub struct CompositeBenchmark {
    pub clipped: usize,
    pub depth: usize,
    pub source_size: u32,
    pub blur: f64,
    pub repetitions: usize,
}
/// Shared masks, consecutive clipping, deep groups, reversible grade and blur.
pub fn run_composite(c: CompositeBenchmark) -> Result<Value> {
    if c.clipped == 0
        || c.clipped > 128
        || c.depth > 32
        || !(8..=1024).contains(&c.source_size)
        || !(0.0..=256.0).contains(&c.blur)
        || !(1..=20).contains(&c.repetitions)
    {
        bail!("composite benchmark: clipped 1..128, depth 0..32, source 8..1024, blur 0..256, repetitions 1..20")
    }
    use rand_core::RngCore;
    struct Scratch(std::path::PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let path = std::env::temp_dir().join(format!(
        "pentool-composite-bench-{:016x}",
        rand_core::OsRng.next_u64()
    ));
    fs::create_dir(&path)?;
    let scratch = Scratch(path);
    let document = scratch.0.join("benchmark.pen");
    let mut pixels = ::image::RgbaImage::new(c.source_size, c.source_size);
    for (x, y, p) in pixels.enumerate_pixels_mut() {
        *p = ::image::Rgba([(x % 256) as u8, (y % 256) as u8, ((x ^ y) % 256) as u8, 255]);
    }
    let mut png = std::io::Cursor::new(Vec::new());
    ::image::DynamicImage::ImageRgba8(pixels).write_to(&mut png, ::image::ImageFormat::Png)?;
    let png = png.into_inner();
    let mut raw =
        crate::composite::migrate(crate::scene::new_document(c.source_size, c.source_size))?;
    crate::image::add(
        &mut raw,
        None,
        "layer-1",
        "base",
        &png,
        crate::image::embedded_storage(&png),
        0.0,
        0.0,
        f64::from(c.source_size),
        f64::from(c.source_size),
        crate::image::Fit::Fill,
    )?;
    crate::image::op_add(
        &mut raw,
        None,
        "base",
        "brightness-contrast",
        Some("source-grade"),
        None,
        crate::image::OpParams(
            json!({"brightness":4,"contrast":8})
                .as_object()
                .unwrap()
                .clone(),
        ),
    )?;
    crate::mask::create(&mut raw, &document, None, "shared", "node-alpha:base")?;
    let base = raw["pages"][0]["layers"][0]["nodes"][0].clone();
    for i in 0..c.clipped {
        let mut node = base.clone();
        node["id"] = json!(format!("clipped-{i}"));
        node["clipping"] = json!({"base":"base"});
        node["opacity"] = json!(0.1);
        node["blend_mode"] = json!("overlay");
        raw["pages"][0]["layers"][0]["nodes"]
            .as_array_mut()
            .unwrap()
            .push(node);
    }
    for i in 0..c.clipped {
        crate::mask::attach(
            &mut raw,
            None,
            &format!("clipped-{i}"),
            "shared",
            &json!({"feather":1}),
        )?;
    }
    crate::composite::edit(
        &mut raw,
        None,
        "add",
        "grade",
        &json!({"adjustment":"vibrance","params":{"amount":12}}),
    )?;
    for i in 0..c.depth {
        let children = std::mem::take(
            raw["pages"][0]["layers"][0]["nodes"]
                .as_array_mut()
                .unwrap(),
        );
        raw["pages"][0]["layers"][0]["nodes"] =
            json!([{"kind":"group","id":format!("depth-{i}"),"children":children}]);
    }
    let target = if c.depth > 0 {
        format!("depth-{}", c.depth - 1)
    } else {
        "base".into()
    };
    crate::effects::edit(
        &mut raw,
        None,
        &target,
        "add",
        "large-blur",
        &json!({"kind":"blur","params":{"radius":c.blur}}),
    )?;
    crate::scene::validate(&raw)?;
    let mut runs = Vec::new();
    let mut expected = None;
    let cache = scratch.0.join(".pentool");
    if cache.exists() {
        fs::remove_dir_all(cache)?;
    }
    for i in 0..=c.repetitions {
        let start = Instant::now();
        let output = crate::composite::png(&raw, &document, None, 1.0)?;
        let elapsed = micros(start);
        let hash = crate::resource::sha256(&output);
        if expected.as_ref().is_some_and(|expected| *expected != hash) {
            bail!("cold/warm composite output differs")
        }
        expected = Some(hash.clone());
        runs.push(json!({"cache":if i==0 {"cold"} else {"warm"},"total_us":elapsed,"output_bytes":output.len(),"png_sha256":hash}));
    }
    Ok(
        json!({"schema_version":1,"benchmark":"composite","fixture":{"clipped":c.clipped,"depth":c.depth,"source_size":c.source_size,"blur":c.blur,"shared_masks":1,"json_bytes":serde_json::to_vec(&raw)?.len()},"budgets":{"temporary_bytes":crate::composite::MAX_TEMP_BYTES,"pixel_work":crate::composite::MAX_PIXEL_WORK},"runs":runs,"peak_resident_bytes":peak_resident_bytes()}),
    )
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

#[derive(Debug, Clone, Copy)]
pub struct PhotoBenchmark {
    /// Photos in the synthetic shoot.
    pub photos: usize,
    /// Megapixels of each 3:2 Bayer source.
    pub megapixels: f64,
    pub repetitions: usize,
}

/// A 16-bit uncompressed RGGB DNG, 12-bit values (black 256, white 4095), of
/// a smooth scene with deterministic texture that varies with `seed`.
pub(crate) fn bayer_dng(width: u32, height: u32, seed: u64) -> Vec<u8> {
    let rows_per_strip = 256.min(height);
    let mut pixels = Vec::with_capacity(width as usize * height as usize * 2);
    for y in 0..height {
        for x in 0..width {
            let mut h = ((u64::from(x / 4) << 32) | u64::from(y / 4))
                ^ seed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
            h ^= h >> 29;
            h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
            h ^= h >> 32;
            let (u, v) = (
                f64::from(x) / f64::from(width),
                f64::from(y) / f64::from(height),
            );
            let channel = [1.0, 0.8, 0.8, 0.55][((y & 1) * 2 + (x & 1)) as usize];
            let scene = 0.08 + 0.55 * u * (1.0 - 0.4 * v) + 0.12 * ((h % 1000) as f64 / 1000.0);
            let value = 256.0 + (scene * channel).min(1.0) * 3839.0;
            pixels.extend((value.round() as u16).to_le_bytes());
        }
    }
    let row_bytes = width * 2;
    let strips = height.div_ceil(rows_per_strip);
    let offsets: Vec<u32> = (0..strips)
        .map(|s| 8 + s * rows_per_strip * row_bytes)
        .collect();
    let counts: Vec<u32> = (0..strips)
        .map(|s| (height - s * rows_per_strip).min(rows_per_strip) * row_bytes)
        .collect();
    let srational = |v: &[f64]| -> Vec<u8> {
        v.iter()
            .flat_map(|x| {
                [
                    ((x * 10_000.0).round() as i32).to_le_bytes(),
                    10_000i32.to_le_bytes(),
                ]
                .concat()
            })
            .collect()
    };
    let shorts = |v: &[u16]| -> Vec<u8> { v.iter().flat_map(|x| x.to_le_bytes()).collect() };
    let longs = |v: &[u32]| -> Vec<u8> { v.iter().flat_map(|x| x.to_le_bytes()).collect() };
    let model = b"Pentool Benchmark Body\0".to_vec();
    // (tag, TIFF type, count, little-endian value bytes), in tag order.
    let entries: Vec<(u16, u16, u32, Vec<u8>)> = vec![
        (254, 4, 1, longs(&[0])),
        (256, 4, 1, longs(&[width])),
        (257, 4, 1, longs(&[height])),
        (258, 3, 1, shorts(&[16])),
        (259, 3, 1, shorts(&[1])),
        (262, 3, 1, shorts(&[32803])),
        (273, 4, strips, longs(&offsets)),
        (277, 3, 1, shorts(&[1])),
        (278, 4, 1, longs(&[rows_per_strip])),
        (279, 4, strips, longs(&counts)),
        (33421, 3, 2, shorts(&[2, 2])),
        (33422, 1, 4, vec![0, 1, 1, 2]),
        (50706, 1, 4, vec![1, 4, 0, 0]),
        (50708, 2, model.len() as u32, model),
        (50714, 4, 1, longs(&[256])),
        (50717, 4, 1, longs(&[4095])),
        (
            50721,
            10,
            9,
            srational(&[
                0.6461, -0.0907, -0.0882, -0.4300, 1.2184, 0.2378, -0.0819, 0.1944, 0.5931,
            ]),
        ),
        (50728, 10, 3, srational(&[0.5, 1.0, 0.75])),
        (50778, 3, 1, shorts(&[21])),
    ];
    let mut out = b"II*\0".to_vec();
    let ifd = 8 + pixels.len();
    out.extend((ifd as u32).to_le_bytes());
    out.extend(pixels);
    let mut data_at = ifd + 2 + 12 * entries.len() + 4;
    let mut data = Vec::new();
    out.extend((entries.len() as u16).to_le_bytes());
    for (tag, kind, count, bytes) in &entries {
        out.extend(tag.to_le_bytes());
        out.extend(kind.to_le_bytes());
        out.extend(count.to_le_bytes());
        if bytes.len() <= 4 {
            let mut inline = bytes.clone();
            inline.resize(4, 0);
            out.extend(inline);
        } else {
            out.extend((data_at as u32).to_le_bytes());
            data.extend(bytes);
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

/// Photography qualification: ingest and cull a synthetic Bayer shoot, develop
/// one photo at full size with tone, presence and a radial local adjustment,
/// render its editor preview cold and then from the cache, and batch-export
/// the shoot with the `web-gallery` recipe. Repeated develops and the cached
/// preview must be identical to the first.
pub fn run_photo(c: PhotoBenchmark) -> Result<Value> {
    use crate::photo::{cache, catalog, export, organize, studio};
    use rand_core::RngCore;
    if !(1..=500).contains(&c.photos)
        || !(0.01..=50.0).contains(&c.megapixels)
        || !(1..=20).contains(&c.repetitions)
    {
        bail!("photo benchmark: photos 1–500, megapixels 0.01–50, repetitions 1–20")
    }
    let width = (((c.megapixels * 1e6 * 1.5).sqrt() / 2.0).round() as u32 * 2).max(16);
    let height = ((f64::from(width) / 3.0).round() as u32 * 2).max(16);
    struct Scratch(std::path::PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let path = std::env::temp_dir().join(format!(
        "pentool-photo-bench-{:016x}",
        rand_core::OsRng.next_u64()
    ));
    fs::create_dir_all(path.join("shoot"))?;
    let scratch = Scratch(path);
    let document = scratch.0.join("shoot.pen");

    let start = Instant::now();
    let mut source_bytes = 0u64;
    for i in 0..c.photos {
        let bytes = bayer_dng(width, height, i as u64);
        source_bytes += bytes.len() as u64;
        fs::write(scratch.0.join("shoot").join(format!("p{i:03}.dng")), bytes)?;
    }
    let generate_us = micros(start);

    let mut raw = crate::scene::new_document(1200, 800);
    let ids: Vec<String> = (0..c.photos).map(|i| format!("p{i:03}")).collect();
    let start = Instant::now();
    for id in &ids {
        let relative = std::path::PathBuf::from(format!("shoot/{id}.dng"));
        let bytes = fs::read(scratch.0.join(&relative))?;
        catalog::add_raw(
            &mut raw,
            id,
            id,
            &bytes,
            crate::image::external_storage(&relative)?,
            catalog::camera_profile("auto")?,
        )?;
    }
    let ingest_us = micros(start);

    let rating = |n| organize::Rating {
        rating: Some(n),
        pick: None,
        label: None,
    };
    let start = Instant::now();
    let alternate: Vec<String> = ids.iter().step_by(2).cloned().collect();
    organize::rate(&mut raw, &organize::Selection::Ids(ids.clone()), &rating(2))?;
    organize::rate(&mut raw, &organize::Selection::Ids(alternate), &rating(4))?;
    let picked = organize::search(&raw, "rating>=4", 100, 0)?;
    let cull_us = micros(start);
    if picked["matches"] != json!(c.photos.div_ceil(2)) {
        bail!(
            "photo benchmark: culling selected {} photos",
            picked["matches"]
        )
    }

    let develop = &mut raw["photography"]["photos"][0]["variants"][0]["develop"];
    develop["tone"] = json!({"exposure": 0.35, "contrast": 12, "highlights": -20, "shadows": 15});
    develop["presence"] = json!({"clarity": 10, "vibrance": 15});
    develop["local"] = json!([{"id": "center", "mask": {"components": [
        {"kind": "radial", "mode": "add", "center": [0.5, 0.5], "radius": [0.3, 0.3]}]},
        "params": {"exposure": 0.4}}]);
    crate::scene::validate(&raw)?;
    let json_bytes = serde_json::to_vec(&raw)?;
    fs::write(&document, &json_bytes)?;

    let mut develops = Vec::new();
    let mut develop_hash = None;
    for _ in 0..c.repetitions {
        let start = Instant::now();
        let developed = catalog::render_validated(&raw, &document, "p000", "master")?;
        develops.push(micros(start));
        let bits: Vec<u8> = developed
            .image
            .rgb
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let hash = crate::resource::sha256(&bits);
        if develop_hash.as_ref().is_some_and(|e| *e != hash) {
            bail!("repeated photo develops differ")
        }
        develop_hash = Some(hash);
    }

    let preview_cache = cache::Cache::with_limit(&document, cache::DEFAULT_LIMIT);
    let request = studio::PreviewRequest {
        photo: "p000",
        variant: "master",
        edge: studio::DEFAULT_EDGE,
        space: "display-p3",
        overlay: studio::Overlay::Clipping,
        uncropped: false,
    };
    let mut previews = Vec::new();
    let mut preview_hash = None;
    for _ in 0..2 {
        let start = Instant::now();
        let preview = studio::preview_cached(&raw, &document, &request, Some(&preview_cache))?;
        let elapsed = micros(start);
        let hash = crate::resource::sha256(&preview.png);
        if preview_hash.as_ref().is_some_and(|e| *e != hash) {
            bail!("cached photo preview differs from the rendered one")
        }
        preview_hash = Some(hash);
        previews.push(json!({"cache": preview.report["cache"], "total_us": elapsed, "png_bytes": preview.png.len()}));
    }

    let recipe = export::Recipe::named(&raw, "web-gallery")?;
    let out = scratch.0.join("delivery");
    let start = Instant::now();
    let planned = export::plan(
        &raw,
        &document,
        &recipe,
        &ids,
        &export::Variants::Master,
        &out,
        false,
    )?;
    export::run(&raw, &document, &recipe, &planned, &out)?;
    let export_us = micros(start);
    let output_bytes: u64 = fs::read_dir(&out)?
        .filter_map(|e| e.ok()?.metadata().ok())
        .filter(|m| m.is_file())
        .map(|m| m.len())
        .sum();

    Ok(json!({
        "schema_version": 1,
        "benchmark": "photo",
        "fixture": {
            "photos": c.photos,
            "width": width,
            "height": height,
            "megapixels": f64::from(width) * f64::from(height) / 1e6,
            "source_bytes": source_bytes,
            "catalog_json_bytes": json_bytes.len(),
            "develop": "p000: tone, presence and one radial local adjustment",
        },
        "timings_us": {
            "generate": generate_us,
            "ingest": ingest_us,
            "cull": cull_us,
            "develop": develops,
            "preview": previews,
            "export": export_us,
        },
        "export": {"recipe": "web-gallery", "files": planned.len(), "bytes": output_bytes},
        "develop_sha256": develop_hash,
        "preview_sha256": preview_hash,
        "peak_resident_bytes": peak_resident_bytes(),
    }))
}
