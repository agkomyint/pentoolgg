//! Read-only, deterministic composite analysis and before/after comparison.
use anyhow::{bail, Context, Result};
use image::RgbaImage;
use serde_json::{json, Value};
use std::path::Path;

fn scoped(raw: &Value, document: &Path, page: Option<&str>, scope: &str) -> Result<RgbaImage> {
    if scope == "page" {
        return crate::composite::render(raw, document, page, 1.0);
    }
    if let Some(id) = scope.strip_prefix("group:") {
        let (node, _) = crate::composite::node_ref(raw, page, id)?;
        if node["kind"] != "group" {
            bail!("scope {id} is not a group")
        }
        return crate::composite::node_pixels(raw, document, page, id, 1.0);
    }
    if let Some(id) = scope.strip_prefix("node:") {
        return crate::composite::node_pixels(raw, document, page, id, 1.0);
    }
    if scope.starts_with("mask:") {
        return crate::composite::render(raw, document, page, 1.0);
    }
    bail!("analysis scope must be page, group:ID, node:ID, or mask:NAME")
}
pub fn analyze(
    raw: &Value,
    document: &Path,
    page: Option<&str>,
    scope: &str,
    query: Option<&Value>,
    samples: &[[u32; 2]],
) -> Result<Value> {
    if samples.len() > 1024 {
        bail!("[limit-exceeded] at most 1024 samples")
    }
    let pixels = scoped(raw, document, page, scope)?;
    let mut mask = if let Some(query) = query {
        Some(crate::selection::coverage(
            raw, document, page, query, &pixels,
        )?)
    } else if let Some(name) = scope.strip_prefix("mask:") {
        Some(crate::composite::mask_coverage(raw, document, page, name)?)
    } else {
        None
    };
    if query.is_some() {
        if let Some(name) = scope.strip_prefix("mask:") {
            let scope_mask = crate::composite::mask_coverage(raw, document, page, name)?;
            for (value, scope) in mask.as_mut().unwrap().iter_mut().zip(scope_mask) {
                *value = (*value).min(scope);
            }
        }
    }
    let mut hist = [[0u64; 256]; 5];
    let mut sum = [0u64; 4];
    let mut squares = [0u64; 4];
    let mut min = [255u8; 4];
    let mut max = [0u8; 4];
    let (mut weight, mut selected, mut black, mut white) = (0u64, 0usize, 0usize, 0usize);
    let mut channel_clipping = [[0usize; 2]; 3];
    let mut alpha = vec![0u8; (pixels.width() * pixels.height()) as usize];
    let mut palette = vec![0u64; 32768];
    for (index, pixel) in pixels.pixels().enumerate() {
        let coverage = mask.as_ref().map_or(255, |m| m[index]);
        let w = (u64::from(coverage) * u64::from(pixel[3]) + 127) / 255;
        alpha[index] = w as u8;
        if w == 0 {
            continue;
        }
        selected += 1;
        weight += w;
        for i in 0..4 {
            let c = u64::from(pixel[i]);
            hist[i][pixel[i] as usize] += w;
            sum[i] += c * w;
            squares[i] += c * c * w;
            min[i] = min[i].min(pixel[i]);
            max[i] = max[i].max(pixel[i]);
        }
        let l = (u32::from(pixel[0]) * 2126
            + u32::from(pixel[1]) * 7152
            + u32::from(pixel[2]) * 722
            + 5000)
            / 10000;
        hist[4][l as usize] += w;
        if pixel[0] == 0 && pixel[1] == 0 && pixel[2] == 0 {
            black += 1;
        }
        if pixel[0] == 255 && pixel[1] == 255 && pixel[2] == 255 {
            white += 1;
        }
        for i in 0..3 {
            if pixel[i] == 0 {
                channel_clipping[i][0] += 1;
            }
            if pixel[i] == 255 {
                channel_clipping[i][1] += 1;
            }
        }
        let bin = ((usize::from(pixel[0]) >> 3) << 10)
            | ((usize::from(pixel[1]) >> 3) << 5)
            | (usize::from(pixel[2]) >> 3);
        palette[bin] += w;
    }
    let channels=(0..4).map(|i|{
        let mean=if weight==0 {0.0} else {sum[i] as f64/weight as f64};
        let variance=if weight==0 {0.0} else {(squares[i] as f64/weight as f64-mean*mean).max(0.0)};
        json!({"min":if weight==0 {Value::Null} else {json!(min[i])},"max":if weight==0 {Value::Null} else {json!(max[i])},"mean":mean,"variance":variance})
    }).collect::<Vec<_>>();
    let sampled=samples.iter().map(|[x,y]| {
        if *x>=pixels.width()||*y>=pixels.height() {bail!("sample coordinate {x},{y} lies outside the page")}
        let p=pixels.get_pixel(*x,*y);let coverage=alpha[(y*pixels.width()+x) as usize];
        Ok(json!({"x":x,"y":y,"rgba":p.0,"coverage":coverage,"hex":format!("#{:02x}{:02x}{:02x}{:02x}",p[0],p[1],p[2],p[3])}))
    }).collect::<Result<Vec<_>>>()?;
    let mut bins = palette
        .iter()
        .enumerate()
        .filter(|(_, w)| **w > 0)
        .collect::<Vec<_>>();
    bins.sort_by(|(a, wa), (b, wb)| wb.cmp(wa).then(a.cmp(b)));
    let palette = bins
        .into_iter()
        .take(5)
        .map(|(bin, w)| {
            let r = ((bin >> 10) & 31) * 8 + 4;
            let g = ((bin >> 5) & 31) * 8 + 4;
            let b = (bin & 31) * 8 + 4;
            json!({"color":format!("#{r:02x}{g:02x}{b:02x}"),"weight":w})
        })
        .collect::<Vec<_>>();
    Ok(
        json!({"analysis_version":1,"scope":scope,"width":pixels.width(),"height":pixels.height(),"selected_pixels":selected,"alpha_weight":weight,
        "channels":channels,"histogram":{"red":hist[0].to_vec(),"green":hist[1].to_vec(),"blue":hist[2].to_vec(),"alpha":hist[3].to_vec(),"luminosity":hist[4].to_vec()},
        "clipping":{"black":black,"white":white,"channels":channel_clipping},"transparency_bounds":crate::selection::bounds(&alpha,pixels.width(),pixels.height()),"samples":sampled,"palette":palette}),
    )
}
pub fn compare(
    raw: &Value,
    document: &Path,
    page: Option<&str>,
    scope: &str,
    query: Option<&Value>,
) -> Result<Value> {
    let after = scoped(raw, document, page, scope)?;
    let mut before_raw = raw.clone();
    fn disable(value: &mut Value) {
        match value {
            Value::Object(object) => {
                if object.get("enabled").is_some() {
                    object.insert("enabled".into(), json!(false));
                }
                object.remove("mask");
                for (key, child) in object {
                    if key != "mask_resources" {
                        disable(child);
                    }
                }
            }
            Value::Array(a) => {
                for child in a {
                    disable(child);
                }
            }
            _ => {}
        }
    }
    disable(&mut before_raw);
    let before = scoped(&before_raw, document, page, scope)?;
    let mut mask = query
        .map(|q| crate::selection::coverage(raw, document, page, q, &after))
        .transpose()?;
    if let Some(name) = scope.strip_prefix("mask:") {
        let scope_mask = crate::composite::mask_coverage(raw, document, page, name)?;
        if let Some(mask) = mask.as_mut() {
            for (value, scope) in mask.iter_mut().zip(scope_mask) {
                *value = (*value).min(scope);
            }
        } else {
            mask = Some(scope_mask);
        }
    }
    let (mut changed, mut maximum, mut sum) = (0usize, 0u8, 0u64);
    for (index, (a, b)) in after.pixels().zip(before.pixels()).enumerate() {
        if mask.as_ref().is_some_and(|m| m[index] == 0) {
            continue;
        }
        if a != b {
            changed += 1;
        }
        for i in 0..4 {
            let d = a[i].abs_diff(b[i]);
            maximum = maximum.max(d);
            sum += u64::from(d);
        }
    }
    Ok(
        json!({"comparison_version":1,"scope":scope,"changed_pixels":changed,"maximum_channel_difference":maximum,"absolute_channel_difference":sum}),
    )
}
pub fn palette_tokens(raw: &mut Value, analysis: &Value, prefix: &str) -> Result<Value> {
    if prefix.is_empty() || prefix.len() > 100 {
        bail!("palette prefix must contain 1..100 bytes")
    }
    let palette = analysis["palette"]
        .as_array()
        .context("analysis palette missing")?;
    if palette.is_empty() {
        bail!("analysis has no visible palette colors")
    }
    let mut candidate = raw.clone();
    let mut names = Vec::new();
    for (index, color) in palette.iter().enumerate() {
        let name = format!("{prefix}-{}", index + 1);
        if candidate["styles"].get(&name).is_some() {
            bail!("palette token {name} already exists")
        }
        crate::style::apply(
            &mut candidate,
            crate::style::Operation::Set,
            Some(&name),
            Some("color"),
            color["color"].as_str(),
            None,
        )?;
        names.push(name);
    }
    crate::scene::validate(&candidate)?;
    *raw = candidate;
    Ok(json!({"tokens":names}))
}
