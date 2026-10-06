//! Reusable content-addressed mask resources for compositing engine 1.
use anyhow::{bail, Context, Result};
use image::RgbaImage;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::Path;

pub const MAX_MASKS: usize = 256;

pub fn validate(raw: &Value) -> Result<()> {
    let empty = json!({});
    let resources = raw.get("mask_resources").unwrap_or(&empty);
    let resources = resources
        .as_object()
        .context("[invalid-mask] mask_resources must be an object")?;
    if resources.len() > MAX_MASKS {
        bail!("[limit-exceeded] at most {MAX_MASKS} mask resources")
    }
    let assets = raw["image_assets"]
        .as_object()
        .context("image assets missing")?;
    for (digest, resource) in resources {
        crate::resource::verify(&serde_json::to_vec(resource)?, digest)?;
        match resource["kind"].as_str() {
            Some("raster") => {
                let asset = resource["asset"]
                    .as_str()
                    .context("[invalid-mask] raster mask asset missing")?;
                if !assets.contains_key(asset) {
                    bail!("[missing-mask] raster mask {digest} source {asset} missing")
                }
            }
            Some("vector") => {
                fn check(node: &Value, depth: usize) -> Result<()> {
                    if depth > 64 {
                        bail!("[limit-exceeded] vector mask nesting exceeds 64")
                    }
                    if [
                        "mask",
                        "clip",
                        "clipping",
                        "transforms",
                        "effects",
                        "content_opacity",
                    ]
                    .iter()
                    .any(|key| node.get(*key).is_some())
                    {
                        bail!("[invalid-mask] vector mask snapshots cannot reference other masks or clips")
                    }
                    if !matches!(
                        node["kind"].as_str(),
                        Some("group" | "rect" | "ellipse" | "line" | "path" | "text")
                    ) {
                        bail!("[invalid-mask] vector masks contain only vector content")
                    }
                    if let Some(children) = node.get("children").and_then(Value::as_array) {
                        for child in children {
                            check(child, depth + 1)?;
                        }
                    }
                    Ok(())
                }
                check(&resource["node"], 0)?;
                crate::scene::validate_node(&resource["node"], 0, &mut HashSet::new(), None, true)?;
            }
            _ => bail!("[invalid-mask] mask {digest} kind must be raster or vector"),
        }
    }
    if let Some(names) = raw.get("masks") {
        let names = names
            .as_object()
            .context("[invalid-mask] masks must be an object")?;
        if names.len() > MAX_MASKS {
            bail!("[limit-exceeded] too many named masks")
        }
        for (name, digest) in names {
            if name.is_empty()
                || name.len() > 128
                || !digest.as_str().is_some_and(|id| resources.contains_key(id))
            {
                bail!("[invalid-mask] mask alias {name} references missing resource")
            }
        }
    }
    fn visit(value: &Value, resources: &serde_json::Map<String, Value>) -> Result<()> {
        match value {
            Value::Object(object) => {
                if let Some(mask) = object.get("mask").filter(|m| m.get("resource").is_some()) {
                    validate_attachment(mask)?;
                    let digest = mask["resource"].as_str().unwrap();
                    if !resources.contains_key(digest) {
                        bail!(
                            "[missing-mask] node {} mask resource {digest} missing",
                            object.get("id").unwrap_or(&Value::Null)
                        )
                    }
                }
                for (key, child) in object {
                    if key != "mask_resources" {
                        visit(child, resources)?;
                    }
                }
            }
            Value::Array(values) => {
                for child in values {
                    visit(child, resources)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    visit(raw, resources)
}

pub fn validate_attachment(mask: &Value) -> Result<()> {
    mask["resource"]
        .as_str()
        .context("[invalid-mask] resource is required")?;
    for (key, max) in [("density", 1.0), ("feather", 256.0)] {
        if !mask
            .get(key)
            .and_then(Value::as_f64)
            .is_some_and(|v| v.is_finite() && (0.0..=max).contains(&v))
        {
            bail!("[invalid-mask] {key} must be in 0..={max}")
        }
    }
    for key in ["invert", "linked"] {
        if !mask.get(key).is_some_and(Value::is_boolean) {
            bail!("[invalid-mask] {key} must be boolean")
        }
    }
    crate::composite::transform(mask)?;
    if mask.get("anchor").is_some() {
        crate::composite::transform(&json!({"transform":mask["anchor"]}))?;
    }
    Ok(())
}

pub(crate) fn store_pixels(raw: &mut Value, pixels: RgbaImage) -> Result<String> {
    let mut encoded = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(pixels).write_to(&mut encoded, image::ImageFormat::Png)?;
    let bytes = encoded.into_inner();
    let source = crate::image::decode_source(&bytes)?;
    let digest = source.digest.clone();
    raw["image_assets"].as_object_mut().context("image asset table missing")?.entry(digest.clone()).or_insert(json!({
        "media_type":source.media_type,"byte_length":source.byte_length,"pixel_width":source.pixel_width,
        "pixel_height":source.pixel_height,"color_space":source.color_space,"orientation":source.orientation,
        "storage":crate::image::embedded_storage(&bytes)
    }));
    Ok(digest)
}

pub fn create(
    raw: &mut Value,
    document: &Path,
    page: Option<&str>,
    name: &str,
    from: &str,
) -> Result<Value> {
    crate::scene::validate(raw)?;
    if !crate::composite::is_document(raw) {
        bail!("mask creation requires `migrate --target 6`")
    }
    if name.is_empty() || name.len() > 128 {
        bail!("mask name must contain 1..=128 bytes")
    }
    if raw
        .get("masks")
        .is_some_and(|names| names.get(name).is_some())
    {
        bail!("[invalid-mask] named mask {name} already exists")
    }
    let mut candidate = raw.clone();
    let resource = if let Some(id) = from.strip_prefix("vector:") {
        let (source, parent) = crate::composite::node_ref(raw, page, id)?;
        let mut node = source.clone();
        node["transform"] = json!((parent * crate::composite::transform(source)?).as_coeffs());
        json!({"kind":"vector","node":node})
    } else if let Some(id) = from.strip_prefix("node-alpha:") {
        let mut pixels = crate::composite::node_pixels(raw, document, page, id, 1.0)?;
        for pixel in pixels.pixels_mut() {
            *pixel = image::Rgba([pixel[3], pixel[3], pixel[3], 255]);
        }
        let asset = store_pixels(&mut candidate, pixels)?;
        json!({"kind":"raster","asset":asset})
    } else if let Some(asset) = from.strip_prefix("asset:") {
        if candidate["image_assets"].get(asset).is_none() {
            bail!("mask source image asset {asset} missing")
        }
        json!({"kind":"raster","asset":asset})
    } else {
        bail!("mask source must be vector:ID, node-alpha:ID, or asset:sha256:DIGEST")
    };
    let digest = crate::asset::hash_bytes(&serde_json::to_vec(&resource)?);
    let object = candidate.as_object_mut().unwrap();
    object
        .entry("mask_resources")
        .or_insert(json!({}))
        .as_object_mut()
        .unwrap()
        .insert(digest.clone(), resource);
    object
        .entry("masks")
        .or_insert(json!({}))
        .as_object_mut()
        .unwrap()
        .insert(name.into(), json!(digest));
    crate::scene::validate(&candidate)?;
    *raw = candidate;
    Ok(json!({"mask":name,"resource":digest}))
}

pub fn attach(
    raw: &mut Value,
    page: Option<&str>,
    id: &str,
    name: &str,
    settings: &Value,
) -> Result<Value> {
    crate::composite::settings(
        settings,
        &["invert", "density", "feather", "linked", "transform"],
    )?;
    crate::scene::validate(raw)?;
    let digest = raw
        .get("masks")
        .and_then(|m| m.get(name))
        .and_then(Value::as_str)
        .or_else(|| {
            raw.get("mask_resources")
                .filter(|m| m.get(name).is_some())
                .map(|_| name)
        })
        .with_context(|| format!("[missing-mask] mask {name} missing"))?
        .to_owned();
    let mut candidate = raw.clone();
    let (original, parent) = crate::composite::node_ref(raw, page, id)?;
    let anchor = (parent * crate::composite::transform(original)?)
        .inverse()
        .as_coeffs();
    if anchor.iter().any(|n| !n.is_finite()) {
        bail!("[invalid-mask] linked masks require an invertible node transform")
    }
    let selected = crate::scene::page_mut(&mut candidate, page)?;
    crate::composite::ensure_unlocked(selected, id)?;
    let node = crate::scene::find_node_mut(selected, id).context("mask target node missing")?;
    let mask = json!({"resource":digest,"invert":settings.get("invert").cloned().unwrap_or(json!(false)),
        "anchor":anchor,
        "density":settings.get("density").cloned().unwrap_or(json!(1.0)),
        "feather":settings.get("feather").cloned().unwrap_or(json!(0.0)),
        "linked":settings.get("linked").cloned().unwrap_or(json!(true)),
        "transform":settings.get("transform").cloned().unwrap_or(json!([1,0,0,1,0,0]))});
    node["mask"] = mask.clone();
    crate::scene::validate(&candidate)?;
    *raw = candidate;
    Ok(json!({"node":id,"mask":mask}))
}

pub fn detach(raw: &mut Value, page: Option<&str>, id: &str) -> Result<Value> {
    crate::scene::validate(raw)?;
    let mut candidate = raw.clone();
    let selected = crate::scene::page_mut(&mut candidate, page)?;
    crate::composite::ensure_unlocked(selected, id)?;
    let node = crate::scene::find_node_mut(selected, id).context("mask target node missing")?;
    let mask = node
        .as_object_mut()
        .unwrap()
        .remove("mask")
        .context("node has no mask to detach")?;
    crate::scene::validate(&candidate)?;
    *raw = candidate;
    Ok(json!({"node":id,"detached":mask}))
}

pub fn delete(raw: &mut Value, name: &str) -> Result<Value> {
    crate::scene::validate(raw)?;
    let mut candidate = raw.clone();
    let resource = candidate
        .get_mut("masks")
        .and_then(Value::as_object_mut)
        .and_then(|m| m.remove(name))
        .context("mask name missing")?;
    // Retain immutable resources: attached nodes may still use them, and history is recoverable.
    crate::scene::validate(&candidate)?;
    *raw = candidate;
    Ok(json!({"deleted":name,"resource_retained":resource}))
}

pub fn apply(raw: &mut Value, document: &Path, page: Option<&str>, id: &str) -> Result<Value> {
    crate::scene::validate(raw)?;
    let mut candidate = raw.clone();
    let selected = crate::scene::page_mut(&mut candidate, page)?;
    crate::composite::ensure_unlocked(selected, id)?;
    let node = crate::scene::find_node_mut(selected, id).context("mask target missing")?;
    if node.get("mask").is_none() || node["kind"] == "adjustment" {
        bail!("mask apply requires a masked content node")
    }
    let pixels = crate::composite::node_pixels(raw, document, page, id, 1.0)?;
    let (_, parent) = crate::composite::node_ref(raw, page, id)?;
    let inverse = parent.inverse().as_coeffs();
    if inverse.iter().any(|n| !n.is_finite()) {
        bail!("[invalid-mask] baking requires an invertible parent transform")
    }
    let (width, height) = pixels.dimensions();
    let asset = store_pixels(&mut candidate, pixels)?;
    let node =
        crate::scene::find_node_mut(crate::scene::page_mut(&mut candidate, page)?, id).unwrap();
    let extensions = node
        .as_object()
        .unwrap()
        .iter()
        .filter(|(key, _)| {
            ![
                "transforms",
                "effects",
                "content_opacity",
                "isolation",
                "clipping",
                "fill",
                "kind",
                "id",
                "children",
                "fallback",
                "component",
                "overrides",
                "transform",
                "mask",
                "clip",
                "style",
                "operations",
                "x",
                "y",
                "width",
                "height",
                "fit",
                "position",
                "crop",
                "asset",
                "opacity",
                "blend_mode",
            ]
            .contains(&key.as_str())
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect::<serde_json::Map<_, _>>();
    *node = json!({"kind":"image","id":id,"asset":asset,"x":0,"y":0,"width":width,"height":height,
        "transform":inverse,
        "fit":"fill","position":[0.5,0.5],"crop":[0,0,1,1],"opacity":1,"blend_mode":"normal","operations":[]});
    node.as_object_mut().unwrap().extend(extensions);
    crate::scene::validate(&candidate)?;
    *raw = candidate;
    Ok(json!({"node":id,"baked":true,"asset":asset}))
}
