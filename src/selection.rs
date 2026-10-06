//! Temporary selection queries. Only explicit save/crop operations alter documents.
use anyhow::{bail, Context, Result};
use image::RgbaImage;
use serde_json::{json, Value};
use std::path::Path;
pub const MAX_QUERIES: usize = 128;
pub const MAX_QUERY_DEPTH: usize = 16;
fn number(q: &Value, key: &str, default: f64) -> Result<f64> {
    q.get(key).map_or(Ok(default), |v| {
        v.as_f64()
            .filter(|n| n.is_finite() && n.abs() <= 1e6)
            .context("selection parameter must be finite and bounded")
    })
}
fn validate(q: &Value, depth: usize, count: &mut usize) -> Result<()> {
    *count += 1;
    if depth > MAX_QUERY_DEPTH || *count > MAX_QUERIES {
        bail!("[limit-exceeded] selection query depth/count limit exceeded")
    }
    let kind = q["kind"].as_str().context("selection kind missing")?;
    let keys: &[&str] = match kind {
        "rectangle" | "ellipse" => &["kind", "x", "y", "width", "height"],
        "polygon" | "lasso" => &["kind", "points"],
        "color-range" => &["kind", "color", "tolerance"],
        "luminosity-range" => &["kind", "min", "max"],
        "node-alpha" => &["kind", "node"],
        "mask-alpha" => &["kind", "mask"],
        "add" | "subtract" | "intersect" => &["kind", "a", "b"],
        _ => bail!("unknown selection kind {kind}"),
    };
    for key in q.as_object().context("selection must be an object")?.keys() {
        if !keys.contains(&key.as_str()) {
            bail!("unknown selection parameter {key}")
        }
    }
    match kind {
        "rectangle" | "ellipse" => {
            for key in ["x", "y", "width", "height"] {
                number(q, key, 0.0)?;
            }
            if number(q, "width", 0.0)? <= 0.0 || number(q, "height", 0.0)? <= 0.0 {
                bail!("selection dimensions must be positive")
            }
        }
        "polygon" | "lasso" => {
            let points = q["points"]
                .as_array()
                .filter(|a| (3..=1024).contains(&a.len()))
                .context("polygon requires 3..1024 points")?;
            for point in points {
                let pair = point
                    .as_array()
                    .filter(|a| a.len() == 2)
                    .context("polygon point must be [x,y]")?;
                for v in pair {
                    v.as_f64()
                        .filter(|n| n.is_finite() && n.abs() <= 1e6)
                        .context("polygon point must be finite and bounded")?;
                }
            }
        }
        "color-range" => {
            crate::fill::color(&q["color"], &Value::Null)?;
            if !(0.0..=442.0).contains(&number(q, "tolerance", 0.0)?) {
                bail!("color tolerance must be in 0..442")
            }
        }
        "luminosity-range" => {
            let (min, max) = (number(q, "min", 0.0)?, number(q, "max", 255.0)?);
            if min < 0.0 || max > 255.0 || min > max {
                bail!("luminosity range must be ordered in 0..255")
            }
        }
        "node-alpha" => {
            q["node"].as_str().context("node-alpha node missing")?;
        }
        "mask-alpha" => {
            q["mask"].as_str().context("mask-alpha mask missing")?;
        }
        _ => {
            validate(&q["a"], depth + 1, count)?;
            validate(&q["b"], depth + 1, count)?;
        }
    }
    Ok(())
}
pub fn coverage(
    raw: &Value,
    document: &Path,
    page: Option<&str>,
    query: &Value,
    pixels: &RgbaImage,
) -> Result<Vec<u8>> {
    let mut count = 0;
    validate(query, 0, &mut count)?;
    let size = u64::from(pixels.width()) * u64::from(pixels.height());
    if size * count as u64 > crate::composite::MAX_PIXEL_WORK
        || size * (MAX_QUERY_DEPTH as u64 + 8) > crate::composite::MAX_TEMP_BYTES
    {
        bail!("[limit-exceeded] selection work/surface budget exceeded")
    }
    fn evaluate(
        raw: &Value,
        document: &Path,
        page: Option<&str>,
        q: &Value,
        pixels: &RgbaImage,
    ) -> Result<Vec<u8>> {
        let kind = q["kind"].as_str().unwrap();
        if matches!(kind, "add" | "subtract" | "intersect") {
            let mut a = evaluate(raw, document, page, &q["a"], pixels)?;
            let b = evaluate(raw, document, page, &q["b"], pixels)?;
            for (a, b) in a.iter_mut().zip(b) {
                *a = match kind {
                    "add" => a.saturating_add(b),
                    "subtract" => a.saturating_sub(b),
                    _ => (*a).min(b),
                };
            }
            return Ok(a);
        }
        if kind == "node-alpha" {
            return Ok(crate::composite::node_pixels(
                raw,
                document,
                page,
                q["node"].as_str().unwrap(),
                1.0,
            )?
            .pixels()
            .map(|p| p[3])
            .collect());
        }
        if kind == "mask-alpha" {
            return crate::composite::mask_coverage(
                raw,
                document,
                page,
                q["mask"].as_str().unwrap(),
            );
        }
        let (x, y, w, h) = (
            number(q, "x", 0.0)?,
            number(q, "y", 0.0)?,
            number(q, "width", 1.0)?,
            number(q, "height", 1.0)?,
        );
        let color = if kind == "color-range" {
            Some(crate::fill::color(&q["color"], &Value::Null)?)
        } else {
            None
        };
        let tolerance = number(q, "tolerance", 0.0)?;
        let (min, max) = (number(q, "min", 0.0)?, number(q, "max", 255.0)?);
        let points = q.get("points").and_then(Value::as_array);
        let mut out = Vec::with_capacity((pixels.width() * pixels.height()) as usize);
        for (px, py, pixel) in pixels.enumerate_pixels() {
            let (xp, yp) = (f64::from(px) + 0.5, f64::from(py) + 0.5);
            let selected = match kind {
                "rectangle" => xp >= x && yp >= y && xp < x + w && yp < y + h,
                "ellipse" => {
                    let (dx, dy) = (
                        (xp - x - w / 2.0) / (w / 2.0),
                        (yp - y - h / 2.0) / (h / 2.0),
                    );
                    dx * dx + dy * dy <= 1.0
                }
                "polygon" | "lasso" => {
                    let points = points.unwrap();
                    let mut inside = false;
                    let mut j = points.len() - 1;
                    for i in 0..points.len() {
                        let (a, b) = (&points[i], &points[j]);
                        let (ax, ay, bx, by) = (
                            a[0].as_f64().unwrap(),
                            a[1].as_f64().unwrap(),
                            b[0].as_f64().unwrap(),
                            b[1].as_f64().unwrap(),
                        );
                        if (ay > yp) != (by > yp) && xp < (bx - ax) * (yp - ay) / (by - ay) + ax {
                            inside = !inside;
                        }
                        j = i;
                    }
                    inside
                }
                "color-range" => {
                    let c = color.unwrap();
                    let d = (0..3)
                        .map(|i| {
                            let v = f64::from(pixel[i]) - c[i] * 255.0;
                            v * v
                        })
                        .sum::<f64>();
                    d <= tolerance * tolerance
                }
                _ => {
                    let l = f64::from(pixel[0]) * 0.2126
                        + f64::from(pixel[1]) * 0.7152
                        + f64::from(pixel[2]) * 0.0722;
                    l >= min && l <= max
                }
            };
            out.push(if selected { 255 } else { 0 });
        }
        Ok(out)
    }
    evaluate(raw, document, page, query, pixels)
}
pub fn bounds(coverage: &[u8], width: u32, height: u32) -> Option<[u32; 4]> {
    let (mut x0, mut y0, mut x1, mut y1) = (width, height, 0, 0);
    let mut any = false;
    for (i, a) in coverage.iter().enumerate() {
        if *a > 0 {
            let (x, y) = (i as u32 % width, i as u32 / width);
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x + 1);
            y1 = y1.max(y + 1);
            any = true;
        }
    }
    any.then_some([x0, y0, x1 - x0, y1 - y0])
}
pub fn save(
    raw: &mut Value,
    document: &Path,
    page: Option<&str>,
    name: &str,
    query: &Value,
) -> Result<Value> {
    let pixels = crate::composite::render(raw, document, page, 1.0)?;
    let coverage = coverage(raw, document, page, query, &pixels)?;
    if bounds(&coverage, pixels.width(), pixels.height()).is_none() {
        bail!("[empty-selection] selection contains no coverage")
    }
    if name.is_empty()
        || name.len() > 128
        || raw.get("masks").is_some_and(|m| m.get(name).is_some())
    {
        bail!("selection mask name must be new and nonempty")
    }
    let mut candidate = raw.clone();
    let mut mask = RgbaImage::new(pixels.width(), pixels.height());
    for (pixel, a) in mask.pixels_mut().zip(coverage) {
        *pixel = image::Rgba([a, a, a, 255]);
    }
    let asset = crate::mask::store_pixels(&mut candidate, mask)?;
    let result = crate::mask::create(
        &mut candidate,
        document,
        page,
        name,
        &format!("asset:{asset}"),
    )?;
    crate::scene::validate(&candidate)?;
    *raw = candidate;
    Ok(result)
}
pub fn crop(raw: &mut Value, document: &Path, page: Option<&str>, query: &Value) -> Result<Value> {
    let pixels = crate::composite::render(raw, document, page, 1.0)?;
    let coverage = coverage(raw, document, page, query, &pixels)?;
    let [x, y, width, height] = bounds(&coverage, pixels.width(), pixels.height())
        .context("[empty-selection] nothing to crop")?;
    let mut candidate = raw.clone();
    let name = format!(
        "crop-{}",
        &crate::resource::sha256(&serde_json::to_vec(query)?)[7..23]
    );
    if candidate
        .get("masks")
        .is_some_and(|m| m.get(&name).is_some())
    {
        bail!("saved crop mask already exists; choose a new selection or undo the previous crop")
    }
    save(&mut candidate, document, page, &name, query)?;
    let digest = candidate["masks"][&name].clone();
    let selected = crate::scene::page_mut(&mut candidate, page)?;
    let mut used = std::collections::HashSet::new();
    fn ids(value: &Value, used: &mut std::collections::HashSet<String>) {
        if let Some(id) = value.get("id").and_then(Value::as_str) {
            used.insert(id.to_owned());
        }
        if let Some(children) = value.get("children").and_then(Value::as_array) {
            for c in children {
                ids(c, used);
            }
        }
    }
    for layer in selected["layers"].as_array().unwrap() {
        for node in layer["nodes"].as_array().unwrap() {
            ids(node, &mut used);
        }
    }
    for (index, layer) in selected["layers"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .enumerate()
    {
        if layer["locked"] == true && !layer["nodes"].as_array().unwrap().is_empty() {
            bail!("crop affects locked layer {}; unlock it first", layer["id"])
        }
        let mut suffix = index;
        let id = loop {
            let id = format!("selection-crop-{suffix}");
            if used.insert(id.clone()) {
                break id;
            }
            suffix += 1;
        };
        let mut children = std::mem::take(layer["nodes"].as_array_mut().unwrap());
        fn rebase(raw: &Value, node: &mut Value, x: f64, y: f64) -> Result<()> {
            let shift = kurbo::Affine::translate((-x, -y));
            if crate::effects::stack(raw, node)?
                .iter()
                .any(|op| op["kind"] == "gradient-overlay")
            {
                node["effect_origin"] = json!([
                    node["effect_origin"][0].as_f64().unwrap_or(0.0) + x,
                    node["effect_origin"][1].as_f64().unwrap_or(0.0) + y
                ]);
            }
            if let Some(mask) = node
                .get_mut("mask")
                .filter(|m| m.get("resource").is_some() && m["linked"] == false)
            {
                mask["transform"] = json!((shift * crate::composite::transform(mask)?).as_coeffs());
            }
            if let Some(stack) = node.get_mut("transforms").and_then(Value::as_array_mut) {
                for operation in stack {
                    // Conjugate page-space homographies into the cropped page's coordinates.
                    let m = operation["matrix"]
                        .as_array()
                        .context("transform matrix missing")?;
                    let mut m = m.iter().map(|v| v.as_f64().unwrap()).collect::<Vec<_>>();
                    let original = m.clone();
                    for row in 0..3 {
                        m[row * 3 + 2] = original[row * 3] * x
                            + original[row * 3 + 1] * y
                            + original[row * 3 + 2];
                    }
                    for col in 0..3 {
                        m[col] -= x * m[6 + col];
                        m[3 + col] -= y * m[6 + col];
                    }
                    operation["matrix"] = json!(m);
                    rebase(raw, operation, x, y)?;
                }
            }
            if let Some(children) = node.get_mut("children").and_then(Value::as_array_mut) {
                for child in children {
                    rebase(raw, child, x, y)?;
                }
            }
            if let Some(fallback) = node.get_mut("fallback").filter(|f| f.get("kind").is_some()) {
                rebase(raw, fallback, x, y)?;
            }
            Ok(())
        }
        for child in &mut children {
            rebase(raw, child, f64::from(x), f64::from(y))?;
        }
        layer["nodes"] = json!([{"kind":"group","id":id,"transform":[1,0,0,1,-i64::from(x),-i64::from(y)],"children":children,
            "mask":{"resource":digest,"invert":false,"density":1,"feather":0,"linked":true,"anchor":[1,0,0,1,0,0],"transform":[1,0,0,1,0,0]}}]);
    }
    selected["canvas"]["width"] = json!(width);
    selected["canvas"]["height"] = json!(height);
    crate::scene::validate(&candidate)?;
    *raw = candidate;
    Ok(json!({"crop":[x,y,width,height],"mask":name}))
}
