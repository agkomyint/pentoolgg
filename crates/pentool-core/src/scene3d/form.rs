//! Deterministic meshes generated from bounded v11 product-form parameters.

use super::math::IDENTITY;
use super::model::{Instance, Material, Model, Primitive};
use anyhow::{bail, ensure, Context, Result};
use serde_json::Value;

const MAX_DIMENSION_MM: f64 = 100_000.0;
const MIN_SEGMENTS: usize = 8;
const MAX_SEGMENTS: usize = 256;
const MAX_PROFILE_POINTS: usize = 256;
const MAX_PROFILE_TANGENT: f64 = 20.0;

/// Offline, editable starting points for common mockup products. These are
/// geometry recipes, not licensed third-party model assets.
pub fn starter_presets() -> Vec<Value> {
    let mut presets = vec![
        serde_json::json!({
            "id":"perfume-bottle", "name":"Perfume bottle", "kind":"lathe-bottle",
            "description":"Faceted glass-style bottle with a separate cap and label band.",
            "generator":1, "material_slots":["glass-body","label-band","cap"],
            "parameters":{"height_mm":180,"radial_segments":64,"profile_mm":[[24,0],[27,10],[27,20],[27,105],[24,120],[12,132],[12,164],[8,180]],"label_band_mm":[20,105],"cap_mm":{"radius_mm":12,"height_mm":24,"offset_mm":0}}
        }),
        serde_json::json!({
            "id":"cosmetic-jar", "name":"Cosmetic jar", "kind":"lathe-bottle",
            "description":"Wide-body jar with a distinct cap and printable label band.",
            "generator":1, "material_slots":["jar-body","label-band","cap"],
            "parameters":{"height_mm":75,"radial_segments":48,"profile_mm":[[30,0],[34,4],[36,10],[36,50],[33,58],[30,75]],"label_band_mm":[10,50],"cap_mm":{"radius_mm":32,"height_mm":15,"offset_mm":0}}
        }),
        serde_json::json!({
            "id":"cosmetic-tube", "name":"Cosmetic tube", "kind":"tube",
            "description":"Open cylindrical tube shell with independent inner and rim regions.",
            "generator":1, "material_slots":["body","inside","top","bottom"],
            "parameters":{"height_mm":140,"outer_diameter_mm":44,"inner_diameter_mm":2,"radial_segments":48}
        }),
        serde_json::json!({
            "id":"cylinder-container", "name":"Cylinder container", "kind":"cylinder",
            "description":"Closed cylindrical container with independently mapped side, top, and bottom surfaces.",
            "generator":1, "material_slots":["body","top","bottom"],
            "parameters":{"height_mm":120,"outer_diameter_mm":64,"radial_segments":48,"capped":true}
        }),
        serde_json::json!({
            "id":"shipping-carton", "name":"Shipping carton", "kind":"box",
            "description":"Editable rectangular carton mockup; this is not a structural dieline.",
            "generator":1, "material_slots":["front","back","sides","top","bottom"],
            "parameters":{"length_mm":300,"width_mm":220,"height_mm":120,"bevel_mm":0.5}
        }),
        serde_json::json!({
            "id":"product-card", "name":"Product card", "kind":"box",
            "description":"Thin two-sided card mockup for tags, inserts, and small print pieces.",
            "generator":1, "material_slots":["front","back","sides","top","bottom"],
            "parameters":{"length_mm":90,"width_mm":55,"height_mm":1.2,"bevel_mm":0.2}
        }),
        serde_json::json!({
            "id":"device-frame", "name":"Simple device frame", "kind":"box",
            "description":"A simple rectangular device-body mockup; not an electronics or CAD model.",
            "generator":1, "material_slots":["front","back","sides","top","bottom"],
            "parameters":{"length_mm":155,"width_mm":75,"height_mm":9,"bevel_mm":2}
        }),
        serde_json::json!({
            "id":"stand-up-pouch", "name":"Stand-up pouch", "kind":"pouch",
            "description":"Simplified gusseted pouch mockup with editable front, back, gusset, and seal regions.",
            "generator":1, "material_slots":["front","back","gusset","seals"],
            "parameters":{"width_mm":140,"height_mm":220,"gusset_mm":65}
        }),
    ];
    for preset in &mut presets {
        let form = serde_json::json!({
            "kind":preset["kind"],
            "generator":preset["generator"],
            "material_slots":preset["material_slots"],
            "parameters":preset["parameters"]
        });
        let canonical_recipe = form.to_string();
        let digest = crate::resource::sha256(canonical_recipe.as_bytes());
        let mut uv_regions: serde_json::Map<String, Value> = preset["material_slots"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|slot| (slot.to_owned(), serde_json::json!([0.0, 0.0, 1.0, 1.0])))
            .collect();
        if let (Some(band), Some(height)) = (
            preset["parameters"]["label_band_mm"].as_array(),
            preset["parameters"]["height_mm"].as_f64(),
        ) {
            let low = band[0].as_f64().unwrap_or(0.0) / height;
            let high = band[1].as_f64().unwrap_or(height) / height;
            uv_regions.insert(
                "label-band".into(),
                serde_json::json!([0.0, low, 1.0, high - low]),
            );
        }
        preset["mockup"] = serde_json::json!({
            "source":{"kind":"procedural-recipe","geometry_digest":digest,
                "license":"MIT OR Apache-2.0",
                "attribution":"Pentool project contributors; no third-party mesh asset"},
            "uv":{"orientation":"per-material region; u increases right, v increases up",
                "regions":uv_regions},
            "allowed_source_binding":{"kinds":["page","node"],"modes":["decal","fill"]},
            "camera":{"projection":"perspective","azimuth":30,"elevation":15,"fov":35,
                "framing":"automatic model bounds"},
            "default_lighting":"two directional lights from scene3d::render::default_lights",
            "ground":{"enabled_by_default":false,"optional":true},
            "shadows":{"enabled_by_default":true,"optional":true},
            "minimum_useful_resolution_px":[640,640]
        });
    }
    presets
}

/// Resolve one built-in product starter to its ordinary editable v11 form.
pub fn starter_preset(id: &str) -> Result<Value> {
    let mut preset = starter_presets()
        .into_iter()
        .find(|preset| preset.get("id").and_then(Value::as_str) == Some(id))
        .with_context(|| format!("[missing-resource] unknown product starter `{id}`"))?;
    let object = preset
        .as_object_mut()
        .context("built-in product starter must be an object")?;
    object.remove("id");
    object.remove("name");
    object.remove("description");
    object.remove("mockup");
    Ok(preset)
}

fn number(parameters: &Value, key: &str) -> Result<f64> {
    let n = parameters
        .get(key)
        .and_then(Value::as_f64)
        .with_context(|| {
            format!("[invalid-input] product form parameter `{key}` must be a number")
        })?;
    ensure!(
        n.is_finite() && n > 0.0 && n <= MAX_DIMENSION_MM,
        "[invalid-input] product form parameter `{key}` must be within 0..={MAX_DIMENSION_MM} mm"
    );
    Ok(n)
}

fn segments(parameters: &Value) -> Result<usize> {
    let n = parameters
        .get("radial_segments")
        .and_then(Value::as_u64)
        .context("[invalid-input] radial_segments is required")? as usize;
    ensure!(
        (MIN_SEGMENTS..=MAX_SEGMENTS).contains(&n),
        "[invalid-input] radial_segments must be {MIN_SEGMENTS}..={MAX_SEGMENTS}"
    );
    Ok(n)
}

fn materials(form: &Value) -> Result<Vec<Material>> {
    let Some(slots) = form.get("material_slots") else {
        return Ok(vec![Material {
            name: "body".into(),
            ..Material::default()
        }]);
    };
    let slots = slots
        .as_array()
        .context("[invalid-input] product form material_slots must be an array")?;
    ensure!(
        !slots.is_empty() && slots.len() <= 64,
        "[limit-exceeded] product form must have 1..=64 material slots"
    );
    let mut names = std::collections::BTreeSet::new();
    let mut materials = Vec::with_capacity(slots.len());
    for slot in slots {
        let name = slot
            .as_str()
            .context("[invalid-input] product form material slot IDs must be strings")?;
        ensure!(
            !name.trim().is_empty() && names.insert(name),
            "[invalid-input] product form material slot IDs must be unique and nonempty"
        );
        materials.push(Material {
            name: name.to_owned(),
            ..Material::default()
        });
    }
    Ok(materials)
}

fn validate_parameters(kind: &str, parameters: &Value) -> Result<()> {
    let object = parameters
        .as_object()
        .context("[invalid-input] product form parameters must be an object")?;
    let allowed: &[&str] = match kind {
        "box" => &["length_mm", "width_mm", "height_mm", "bevel_mm"],
        "pouch" => &["width_mm", "height_mm", "gusset_mm"],
        "cylinder" => &["height_mm", "outer_diameter_mm", "radial_segments", "capped"],
        "tube" => &["height_mm", "outer_diameter_mm", "inner_diameter_mm", "radial_segments", "capped"],
        "lathe-bottle" => &[
            "height_mm",
            "radial_segments",
            "profile_mm",
            "profile_tangents",
            "label_band_mm",
            "cap_mm",
        ],
        _ => bail!("[unsupported-capability] product form kind `{kind}` is not supported (box, pouch, cylinder, tube, lathe-bottle)"),
    };
    for key in object.keys() {
        ensure!(
            allowed.contains(&key.as_str()),
            "[invalid-input] unsupported `{key}` parameter for product form kind `{kind}`"
        );
    }
    if let Some(tangents) = object.get("profile_tangents") {
        let points = object
            .get("profile_mm")
            .and_then(Value::as_array)
            .context("[invalid-input] profile_tangents requires profile_mm")?;
        let tangents = tangents
            .as_array()
            .context("[invalid-input] profile_tangents must be an array of radius/height slopes")?;
        ensure!(
            tangents.len() == points.len(),
            "[invalid-input] profile_tangents must contain one slope per profile point"
        );
        for (index, tangent) in tangents.iter().enumerate() {
            let slope = tangent.as_f64().with_context(|| {
                format!("[invalid-input] profile tangent {index} must be a number")
            })?;
            ensure!(
                slope.is_finite() && slope.abs() <= MAX_PROFILE_TANGENT,
                "[invalid-input] profile tangent {index} must be within -{MAX_PROFILE_TANGENT}..={MAX_PROFILE_TANGENT} mm/mm"
            );
        }
    }
    if let Some(capped) = object.get("capped") {
        ensure!(
            capped.is_boolean(),
            "[invalid-input] product form `capped` must be boolean"
        );
        ensure!(
            kind == "cylinder" || capped.as_bool() == Some(false),
            "[unsupported-capability] capped tube geometry is not implemented yet"
        );
    }
    if let Some(cap) = object.get("cap_mm") {
        let cap = cap
            .as_object()
            .context("[invalid-input] bottle cap_mm must be an object")?;
        ensure!(
            cap.keys()
                .all(|key| { matches!(key.as_str(), "radius_mm" | "height_mm" | "offset_mm") }),
            "[invalid-input] bottle cap_mm only accepts radius_mm, height_mm, and offset_mm"
        );
        let positive = |key: &str| -> Result<f64> {
            let value = cap
                .get(key)
                .and_then(Value::as_f64)
                .with_context(|| format!("[invalid-input] bottle cap_mm.{key} must be a number"))?;
            ensure!(
                value.is_finite() && value > 0.0 && value <= MAX_DIMENSION_MM,
                "[invalid-input] bottle cap_mm.{key} must be within 0..={MAX_DIMENSION_MM} mm"
            );
            Ok(value)
        };
        positive("radius_mm")?;
        let cap_height = positive("height_mm")?;
        let bottle_height = number(parameters, "height_mm")?;
        ensure!(
            cap_height <= bottle_height,
            "[invalid-input] bottle cap height cannot exceed bottle height"
        );
        let offset = match cap.get("offset_mm") {
            None => 0.0,
            Some(value) => value
                .as_f64()
                .with_context(|| "[invalid-input] bottle cap_mm.offset_mm must be a number")?,
        };
        ensure!(
            offset.is_finite()
                && bottle_height - cap_height + offset >= 0.0
                && bottle_height + offset <= MAX_DIMENSION_MM,
            "[invalid-input] bottle cap_mm.offset_mm must keep the cap within 0..={MAX_DIMENSION_MM} mm"
        );
    }
    if kind == "pouch" {
        let width = number(parameters, "width_mm")?;
        let height = number(parameters, "height_mm")?;
        let gusset = number(parameters, "gusset_mm")?;
        ensure!(
            gusset < width,
            "[invalid-input] gusset_mm must be less than pouch width_mm"
        );
        ensure!(
            gusset < height,
            "[invalid-input] gusset_mm must be less than pouch height_mm"
        );
    }
    Ok(())
}

fn push_quad(
    model: &mut Model,
    name: &str,
    points: [[f32; 3]; 4],
    normal: [f32; 3],
    material: usize,
) {
    model.primitives.push(Primitive {
        name: name.to_owned(),
        positions: points.to_vec(),
        normals: vec![normal; 4],
        uvs: vec![vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]],
        indices: vec![0, 1, 2, 0, 2, 3],
        material: Some(material),
    });
    model.instances.push(Instance {
        id: name.to_owned(),
        primitive: model.primitives.len() - 1,
        transform: IDENTITY,
    });
}

fn push_poly(
    model: &mut Model,
    name: &str,
    mut points: Vec<[f32; 3]>,
    outward: [f32; 3],
    material: usize,
) {
    let cross = |a: [f32; 3], b: [f32; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    if points.len() >= 3
        && dot(
            cross(sub(points[1], points[0]), sub(points[2], points[0])),
            outward,
        ) < 0.0
    {
        points.reverse();
    }
    let length = dot(outward, outward).sqrt().max(f32::EPSILON);
    let normal = [
        outward[0] / length,
        outward[1] / length,
        outward[2] / length,
    ];
    // Project along the dominant normal axis, then normalize each planar
    // material face into its own UV rectangle. This keeps bevel faces finite
    // and within the same documented 0..1 slot convention as ordinary quads.
    let abs = [normal[0].abs(), normal[1].abs(), normal[2].abs()];
    let axes = if abs[0] >= abs[1] && abs[0] >= abs[2] {
        [2, 1]
    } else if abs[1] >= abs[2] {
        [0, 2]
    } else {
        [0, 1]
    };
    let mut min = [f32::INFINITY; 2];
    let mut max = [f32::NEG_INFINITY; 2];
    for point in &points {
        for axis in 0..2 {
            min[axis] = min[axis].min(point[axes[axis]]);
            max[axis] = max[axis].max(point[axes[axis]]);
        }
    }
    let range = [
        (max[0] - min[0]).max(f32::EPSILON),
        (max[1] - min[1]).max(f32::EPSILON),
    ];
    let uvs = points
        .iter()
        .map(|point| {
            [
                (point[axes[0]] - min[0]) / range[0],
                (point[axes[1]] - min[1]) / range[1],
            ]
        })
        .collect();
    let mut indices = Vec::with_capacity((points.len() - 2) * 3);
    for i in 1..points.len() - 1 {
        indices.extend_from_slice(&[0, i as u32, (i + 1) as u32]);
    }
    let primitive = model.primitives.len();
    model.primitives.push(Primitive {
        name: name.into(),
        normals: vec![normal; points.len()],
        positions: points,
        uvs: vec![uvs],
        indices,
        material: Some(material),
    });
    model.instances.push(Instance {
        id: name.into(),
        primitive,
        transform: IDENTITY,
    });
}

fn push_slot_poly(
    model: &mut Model,
    name: &str,
    points: Vec<[f32; 3]>,
    outward: [f32; 3],
    slot_name: &str,
) {
    let material = slot(model, slot_name);
    push_poly(model, name, points, outward, material);
}

fn box_form(model: &mut Model, parameters: &Value) -> Result<()> {
    let l = number(parameters, "length_mm")? as f32;
    let w = number(parameters, "width_mm")? as f32;
    let h = number(parameters, "height_mm")? as f32;
    let b = parameters
        .get("bevel_mm")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    ensure!(
        b.is_finite() && b >= 0.0 && b <= f64::from(l.min(w).min(h)) / 2.0,
        "[invalid-input] bevel_mm must be between 0 and half the shortest box dimension"
    );
    let (x, y, z) = (l / 2.0, h / 2.0, w / 2.0);
    if b > 0.0 {
        let (b, x, y, z) = (b as f32, x, y, z);
        let point = |x: f32, y: f32, z: f32| [x, y, z];
        for (name, sign) in [("front", 1.0), ("back", -1.0)] {
            push_slot_poly(
                model,
                name,
                vec![
                    point(-x + b, -y + b, sign * z),
                    point(x - b, -y + b, sign * z),
                    point(x - b, y - b, sign * z),
                    point(-x + b, y - b, sign * z),
                ],
                [0.0, 0.0, sign],
                name,
            );
        }
        for (name, sign) in [("right", 1.0), ("left", -1.0)] {
            push_slot_poly(
                model,
                name,
                vec![
                    point(sign * x, -y + b, -z + b),
                    point(sign * x, -y + b, z - b),
                    point(sign * x, y - b, z - b),
                    point(sign * x, y - b, -z + b),
                ],
                [sign, 0.0, 0.0],
                "sides",
            );
        }
        for (name, sign) in [("top", 1.0), ("bottom", -1.0)] {
            push_slot_poly(
                model,
                name,
                vec![
                    point(-x + b, sign * y, -z + b),
                    point(x - b, sign * y, -z + b),
                    point(x - b, sign * y, z - b),
                    point(-x + b, sign * y, z - b),
                ],
                [0.0, sign, 0.0],
                name,
            );
        }
        for sx in [-1.0, 1.0] {
            for sy in [-1.0, 1.0] {
                push_slot_poly(
                    model,
                    "bevel-z",
                    vec![
                        point(sx * (x - b), sy * y, -z + b),
                        point(sx * x, sy * (y - b), -z + b),
                        point(sx * x, sy * (y - b), z - b),
                        point(sx * (x - b), sy * y, z - b),
                    ],
                    [sx, sy, 0.0],
                    "sides",
                );
            }
        }
        for sx in [-1.0, 1.0] {
            for sz in [-1.0, 1.0] {
                push_slot_poly(
                    model,
                    "bevel-y",
                    vec![
                        point(sx * (x - b), -y + b, sz * z),
                        point(sx * x, -y + b, sz * (z - b)),
                        point(sx * x, y - b, sz * (z - b)),
                        point(sx * (x - b), y - b, sz * z),
                    ],
                    [sx, 0.0, sz],
                    "sides",
                );
            }
        }
        for sy in [-1.0, 1.0] {
            for sz in [-1.0, 1.0] {
                push_slot_poly(
                    model,
                    "bevel-x",
                    vec![
                        point(-x + b, sy * y, sz * (z - b)),
                        point(x - b, sy * y, sz * (z - b)),
                        point(x - b, sy * (y - b), sz * z),
                        point(-x + b, sy * (y - b), sz * z),
                    ],
                    [0.0, sy, sz],
                    "sides",
                );
            }
        }
        for sx in [-1.0, 1.0] {
            for sy in [-1.0, 1.0] {
                for sz in [-1.0, 1.0] {
                    push_slot_poly(
                        model,
                        "bevel-corner",
                        vec![
                            point(sx * x, sy * (y - b), sz * (z - b)),
                            point(sx * (x - b), sy * y, sz * (z - b)),
                            point(sx * (x - b), sy * (y - b), sz * z),
                        ],
                        [sx, sy, sz],
                        "sides",
                    );
                }
            }
        }
        return Ok(());
    }
    let faces = [
        (
            "front",
            "front",
            [[-x, -y, z], [x, -y, z], [x, y, z], [-x, y, z]],
            [0.0, 0.0, 1.0],
        ),
        (
            "back",
            "back",
            [[x, -y, -z], [-x, -y, -z], [-x, y, -z], [x, y, -z]],
            [0.0, 0.0, -1.0],
        ),
        (
            "right",
            "sides",
            [[x, -y, z], [x, -y, -z], [x, y, -z], [x, y, z]],
            [1.0, 0.0, 0.0],
        ),
        (
            "left",
            "sides",
            [[-x, -y, -z], [-x, -y, z], [-x, y, z], [-x, y, -z]],
            [-1.0, 0.0, 0.0],
        ),
        (
            "top",
            "top",
            [[-x, y, z], [x, y, z], [x, y, -z], [-x, y, -z]],
            [0.0, 1.0, 0.0],
        ),
        (
            "bottom",
            "bottom",
            [[-x, -y, -z], [x, -y, -z], [x, -y, z], [-x, -y, z]],
            [0.0, -1.0, 0.0],
        ),
    ];
    for (name, slot_name, points, normal) in faces {
        let material = slot(model, slot_name);
        push_quad(model, name, points, normal, material);
    }
    Ok(())
}

fn pouch_form(model: &mut Model, parameters: &Value) -> Result<()> {
    let width = number(parameters, "width_mm")? as f32;
    let height = number(parameters, "height_mm")? as f32;
    let gusset = number(parameters, "gusset_mm")? as f32;
    let (x, z) = (width / 2.0, gusset / 2.0);
    let crease = (gusset * 0.15).min(width * 0.05);

    // Front and back printable faces, with independent full-panel UVs.
    let front = slot(model, "front");
    push_quad(
        model,
        "pouch-front",
        [[-x, 0.0, z], [x, 0.0, z], [x, height, z], [-x, height, z]],
        [0.0, 0.0, 1.0],
        front,
    );
    let back = slot(model, "back");
    push_quad(
        model,
        "pouch-back",
        [
            [x, 0.0, -z],
            [-x, 0.0, -z],
            [-x, height, -z],
            [x, height, -z],
        ],
        [0.0, 0.0, -1.0],
        back,
    );

    // The two panels on each side meet at an inward crease, so the mockup
    // reads as gusseted packaging instead of a rectangular box.
    let gusset_material = slot(model, "gusset");
    for side in [1.0_f32, -1.0] {
        let sx = side * x;
        let (front_name, back_name) = if side > 0.0 {
            ("pouch-gusset-right-front", "pouch-gusset-right-back")
        } else {
            ("pouch-gusset-left-front", "pouch-gusset-left-back")
        };
        let center_x = sx - side * crease;
        let norm = |normal: [f32; 3]| {
            let length = (normal[0] * normal[0] + normal[2] * normal[2]).sqrt();
            [normal[0] / length, 0.0, normal[2] / length]
        };
        let front_normal = norm([side * z, 0.0, -crease]);
        let back_normal = norm([side * z, 0.0, side * crease]);
        push_quad(
            model,
            front_name,
            [
                [sx, 0.0, z],
                [center_x, 0.0, 0.0],
                [center_x, height, 0.0],
                [sx, height, z],
            ],
            front_normal,
            gusset_material,
        );
        if side < 0.0 {
            model.primitives.last_mut().unwrap().indices = vec![0, 2, 1, 0, 3, 2];
        }
        push_quad(
            model,
            back_name,
            [
                [center_x, 0.0, 0.0],
                [sx, 0.0, -z],
                [sx, height, -z],
                [center_x, height, 0.0],
            ],
            back_normal,
            gusset_material,
        );
        if side < 0.0 {
            model.primitives.last_mut().unwrap().indices = vec![0, 2, 1, 0, 3, 2];
        }
    }
    let seals = slot(model, "seals");
    push_quad(
        model,
        "pouch-top-seal",
        [
            [-x, height, z],
            [x, height, z],
            [x, height, -z],
            [-x, height, -z],
        ],
        [0.0, 1.0, 0.0],
        seals,
    );
    push_quad(
        model,
        "pouch-bottom-gusset",
        [[-x, 0.0, -z], [x, 0.0, -z], [x, 0.0, z], [-x, 0.0, z]],
        [0.0, -1.0, 0.0],
        seals,
    );
    Ok(())
}

fn lathe(model: &mut Model, profile: &[(f64, f64)], segments: usize) {
    let mut positions = Vec::with_capacity(profile.len() * (segments + 1));
    let mut normals = Vec::with_capacity(profile.len() * (segments + 1));
    let mut uvs = Vec::with_capacity(profile.len() * (segments + 1));
    for (ring, &(radius, height)) in profile.iter().enumerate() {
        let (dr, dh) = if ring == 0 {
            (profile[1].0 - radius, profile[1].1 - height)
        } else if ring + 1 == profile.len() {
            (radius - profile[ring - 1].0, height - profile[ring - 1].1)
        } else {
            (
                profile[ring + 1].0 - profile[ring - 1].0,
                profile[ring + 1].1 - profile[ring - 1].1,
            )
        };
        let slope = dr / dh;
        let norm = (1.0 + slope * slope).sqrt();
        for step in 0..=segments {
            let theta = std::f64::consts::TAU * step as f64 / segments as f64;
            let (sin, cos) = theta.sin_cos();
            positions.push([(radius * cos) as f32, height as f32, (radius * sin) as f32]);
            normals.push([
                (cos / norm) as f32,
                (-slope / norm) as f32,
                (sin / norm) as f32,
            ]);
            uvs.push([
                step as f32 / segments as f32,
                height as f32 / profile.last().unwrap().1 as f32,
            ]);
        }
    }
    let stride = segments + 1;
    let mut indices = Vec::with_capacity((profile.len() - 1) * segments * 6);
    for ring in 0..profile.len() - 1 {
        for step in 0..segments {
            let a = (ring * stride + step) as u32;
            let b = ((ring + 1) * stride + step) as u32;
            indices.extend_from_slice(&[a, b, b + 1, a, b + 1, a + 1]);
        }
    }
    let primitive = model.primitives.len();
    model.primitives.push(Primitive {
        name: "lathed-body".into(),
        positions,
        normals,
        uvs: vec![uvs],
        indices,
        material: Some(0),
    });
    model.instances.push(Instance {
        id: format!("lathed-body-{primitive}"),
        primitive,
        transform: IDENTITY,
    });
}

/// Interpolate authored radius/height control points into a bounded, smooth
/// bottle silhouette. Height stays linear within each control interval (so
/// rings remain strictly ordered); radius uses a clamped Catmull-Rom Hermite
/// curve to avoid negative radii or overshoot beyond adjacent controls.
fn bottle_curve(profile: &[(f64, f64)], tangents: Option<&[f64]>) -> Vec<(f64, f64)> {
    const STEPS_PER_CONTROL_INTERVAL: usize = 4;
    let mut curve = Vec::with_capacity((profile.len() - 1) * STEPS_PER_CONTROL_INTERVAL + 1);
    curve.push(profile[0]);
    for index in 0..profile.len() - 1 {
        let (r1, y1) = profile[index];
        let (r2, y2) = profile[index + 1];
        let delta_y = y2 - y1;
        let delta_r = r2 - r1;
        let m1 = if let Some(tangents) = tangents {
            tangents[index] * delta_y
        } else if index == 0 {
            delta_r
        } else {
            let (r0, y0) = profile[index - 1];
            0.5 * (r2 - r0) / (y2 - y0) * delta_y
        };
        let m2 = if let Some(tangents) = tangents {
            tangents[index + 1] * delta_y
        } else if index + 2 == profile.len() {
            delta_r
        } else {
            let (r3, y3) = profile[index + 2];
            0.5 * (r3 - r1) / (y3 - y1) * delta_y
        };
        for step in 1..=STEPS_PER_CONTROL_INTERVAL {
            let t = step as f64 / STEPS_PER_CONTROL_INTERVAL as f64;
            let t2 = t * t;
            let t3 = t2 * t;
            let radius = (2.0 * t3 - 3.0 * t2 + 1.0) * r1
                + (t3 - 2.0 * t2 + t) * m1
                + (-2.0 * t3 + 3.0 * t2) * r2
                + (t3 - t2) * m2;
            curve.push((radius.clamp(r1.min(r2), r1.max(r2)), y1 + delta_y * t));
        }
    }
    curve
}

/// Split a lathed body at profile-ring boundaries so a label band is an actual
/// named material region, not only catalog metadata. The original generated
/// positions, normals, and UV coordinates are retained for every face.
fn split_lathe_material_band(
    model: &mut Model,
    primitive_index: usize,
    slot_name: &str,
    low: f64,
    high: f64,
) -> Result<()> {
    ensure!(
        low.is_finite() && high.is_finite() && low >= 0.0 && high > low,
        "[invalid-input] label_band_mm must be [low, high] with 0 <= low < high"
    );
    let source = model
        .primitives
        .get(primitive_index)
        .context("[missing-resource] lathe body primitive is missing")?;
    let instance = model
        .instances
        .iter()
        .find(|instance| instance.primitive == primitive_index)
        .cloned()
        .context("[invalid-input] lathe body has no instance")?;
    let body_material = source.material.unwrap_or(0);
    let band_material = slot(model, slot_name);
    ensure!(
        band_material != body_material,
        "[invalid-input] label band needs a distinct `{slot_name}` material slot"
    );
    let source = source.clone();
    let mut groups: [Primitive; 2] = std::array::from_fn(|index| Primitive {
        name: if index == 0 {
            source.name.clone()
        } else {
            format!("{}-{slot_name}", source.name)
        },
        positions: Vec::new(),
        normals: Vec::new(),
        uvs: source.uvs.iter().map(|_| Vec::new()).collect(),
        indices: Vec::new(),
        material: Some(if index == 0 {
            body_material
        } else {
            band_material
        }),
    });
    for triangle in source.indices.chunks_exact(3) {
        let ys = [triangle[0], triangle[1], triangle[2]]
            .map(|index| source.positions[index as usize][1] as f64);
        let min_y = ys[0].min(ys[1]).min(ys[2]);
        let max_y = ys[0].max(ys[1]).max(ys[2]);
        let label = min_y >= low && max_y <= high;
        ensure!(
            label || max_y <= low || min_y >= high,
            "[invalid-input] label band edges must align with profile rings"
        );
        let target = &mut groups[usize::from(label)];
        for &index in triangle {
            let source_index = index as usize;
            target.indices.push(target.positions.len() as u32);
            target.positions.push(source.positions[source_index]);
            target.normals.push(source.normals[source_index]);
            for (set, source_uvs) in source.uvs.iter().enumerate() {
                target.uvs[set].push(source_uvs[source_index]);
            }
        }
    }
    let label_primitive = model.primitives.len();
    let label_instance = Instance {
        id: format!("{}-{slot_name}", instance.id),
        primitive: label_primitive,
        transform: instance.transform,
    };
    let body = std::mem::replace(
        &mut groups[0],
        Primitive {
            name: String::new(),
            positions: Vec::new(),
            normals: Vec::new(),
            uvs: Vec::new(),
            indices: Vec::new(),
            material: None,
        },
    );
    let label = std::mem::replace(
        &mut groups[1],
        Primitive {
            name: String::new(),
            positions: Vec::new(),
            normals: Vec::new(),
            uvs: Vec::new(),
            indices: Vec::new(),
            material: None,
        },
    );
    model.primitives[primitive_index] = body;
    if !label.indices.is_empty() {
        model.primitives.push(label);
        model.instances.push(label_instance);
    }
    Ok(())
}

fn slot(model: &Model, name: &str) -> usize {
    model
        .materials
        .iter()
        .position(|material| material.name == name)
        .unwrap_or(0)
}

// The generator parameters describe one ring primitive directly; grouping
// them into a temporary config would obscure these fixed geometry inputs.
#[allow(clippy::too_many_arguments)]
fn annulus(
    model: &mut Model,
    name: &str,
    outer: f64,
    inner: f64,
    y: f64,
    segments: usize,
    top: bool,
    material: usize,
) {
    let mut positions = Vec::with_capacity((segments + 1) * 2);
    let mut normals = Vec::with_capacity((segments + 1) * 2);
    let mut uvs = Vec::with_capacity((segments + 1) * 2);
    for i in 0..=segments {
        let angle = std::f64::consts::TAU * i as f64 / segments as f64;
        let (sin, cos) = angle.sin_cos();
        positions.push([(outer * cos) as f32, y as f32, (outer * sin) as f32]);
        positions.push([(inner * cos) as f32, y as f32, (inner * sin) as f32]);
        let normal = if top {
            [0.0, 1.0, 0.0]
        } else {
            [0.0, -1.0, 0.0]
        };
        normals.extend([normal, normal]);
        uvs.extend([
            [i as f32 / segments as f32, 0.0],
            [i as f32 / segments as f32, 1.0],
        ]);
    }
    let mut indices = Vec::with_capacity(segments * 6);
    for i in 0..segments {
        let outer_a = (i * 2) as u32;
        let inner_a = outer_a + 1;
        let outer_b = outer_a + 2;
        let inner_b = outer_a + 3;
        if top {
            indices.extend_from_slice(&[outer_a, inner_a, inner_b, outer_a, inner_b, outer_b]);
        } else {
            indices.extend_from_slice(&[outer_a, inner_b, inner_a, outer_a, outer_b, inner_b]);
        }
    }
    let primitive = model.primitives.len();
    model.primitives.push(Primitive {
        name: name.into(),
        positions,
        normals,
        uvs: vec![uvs],
        indices,
        material: Some(material),
    });
    model.instances.push(Instance {
        id: name.into(),
        primitive,
        transform: IDENTITY,
    });
}

fn invert_surface(model: &mut Model, primitive: usize) {
    let surface = &mut model.primitives[primitive];
    for normal in &mut surface.normals {
        normal[0] = -normal[0];
        normal[2] = -normal[2];
    }
    for triangle in surface.indices.chunks_exact_mut(3) {
        triangle.swap(1, 2);
    }
}

/// Generate a stable mesh from one validated form record.
pub fn generate(form: &Value) -> Result<Model> {
    let kind = form
        .get("kind")
        .and_then(Value::as_str)
        .context("[invalid-input] product form kind is missing")?;
    ensure!(
        form.get("generator").and_then(Value::as_u64) == Some(1),
        "[unsupported-capability] product form generator must be version 1"
    );
    let parameters = form
        .get("parameters")
        .context("[invalid-input] product form parameters are missing")?;
    validate_parameters(kind, parameters)?;
    let mut model = Model {
        materials: materials(form)?,
        ..Model::default()
    };
    match kind {
        "box" => box_form(&mut model, parameters)?,
        "pouch" => pouch_form(&mut model, parameters)?,
        "cylinder" | "tube" => {
            let height = number(parameters, "height_mm")?;
            let outer = number(parameters, "outer_diameter_mm")? / 2.0;
            let n = segments(parameters)?;
            let inner = parameters.get("inner_diameter_mm").and_then(Value::as_f64).unwrap_or(0.0) / 2.0;
            ensure!(inner >= 0.0 && inner < outer, "[invalid-input] inner_diameter_mm must be less than outer_diameter_mm");
            ensure!(kind != "tube" || inner > 0.0, "[invalid-input] tube form requires inner_diameter_mm");
            ensure!(kind != "cylinder" || inner == 0.0, "[invalid-input] cylinder form cannot have an inner_diameter_mm");
            // The outer and inner walls have independent normals/UVs. Annular
            // ends keep tubes open while a cylinder gets planar caps.
            lathe(&mut model, &[(outer, 0.0), (outer, height)], n);
            let body_material = slot(&model, "body");
            model.primitives[0].material = Some(body_material);
            if inner > 0.0 {
                let primitive = model.primitives.len();
                lathe(&mut model, &[(inner, 0.0), (inner, height)], n);
                invert_surface(&mut model, primitive);
                model.primitives[primitive].material = Some(slot(&model, "inside"));
                let top = slot(&model, "top");
                let bottom = slot(&model, "bottom");
                annulus(&mut model, "bottom-rim", outer, inner, 0.0, n, false, bottom);
                annulus(&mut model, "top-rim", outer, inner, height, n, true, top);
            } else {
                let capped = parameters.get("capped").and_then(Value::as_bool) != Some(false);
                for (ring_height, normal, reverse, slot_name) in [(0.0, [0.0,-1.0,0.0], true, "bottom"), (height, [0.0,1.0,0.0], false, "top")].into_iter().filter(|_| capped) {
                    let center = model.primitives.len();
                    let mut positions = vec![[0.0, ring_height as f32, 0.0]];
                    let normals = vec![normal; n + 2];
                    let mut uvs = vec![[0.5,0.5]];
                    for i in 0..=n {
                        let t = std::f64::consts::TAU * i as f64 / n as f64;
                        positions.push([(outer*t.cos()) as f32, ring_height as f32, (outer*t.sin()) as f32]);
                        uvs.push([(0.5+0.5*t.cos()) as f32,(0.5+0.5*t.sin()) as f32]);
                    }
                    let mut indices = Vec::with_capacity(n*3);
                    for i in 0..n { let a=i as u32+1; let b=a+1; if reverse { indices.extend_from_slice(&[0,b,a]); } else { indices.extend_from_slice(&[0,a,b]); } }
                    let material = slot(&model, slot_name);
                    model.primitives.push(Primitive { name: if ring_height==0.0 {"bottom-cap"} else {"top-cap"}.into(), positions, normals, uvs: vec![uvs], indices, material: Some(material) });
                    model.instances.push(Instance { id: format!("cap-{center}"), primitive: center, transform: IDENTITY });
                }
            }
        }
        "lathe-bottle" => {
            let height = number(parameters, "height_mm")?;
            let segment_count = segments(parameters)?;
            let points = parameters.get("profile_mm").and_then(Value::as_array).context("[invalid-input] lathe profile_mm must be an array")?;
            ensure!((2..=MAX_PROFILE_POINTS).contains(&points.len()), "[limit-exceeded] lathe profile requires 2..={MAX_PROFILE_POINTS} points");
            let mut profile = Vec::with_capacity(points.len());
            let mut prior_height = -1.0;
            for point in points {
                let pair = point.as_array().filter(|p| p.len() == 2).context("[invalid-input] each lathe profile point must be [radius_mm, height_mm]")?;
                let radius = pair[0].as_f64().context("[invalid-input] profile radius must be a number")?;
                let y = pair[1].as_f64().context("[invalid-input] profile height must be a number")?;
                ensure!(radius.is_finite() && radius > 0.0 && radius <= MAX_DIMENSION_MM && y.is_finite() && (0.0..=height).contains(&y) && y > prior_height,
                    "[invalid-input] lathe profile radii must be positive and bounded; heights must increase within 0..=height_mm");
                prior_height = y;
                profile.push((radius, y));
            }
            ensure!(profile[0].1 == 0.0 && profile.last().unwrap().1 == height,
                "[invalid-input] lathe profile must start at 0 and end at height_mm");
            let tangents = parameters
                .get("profile_tangents")
                .map(|value| {
                    let values = value.as_array().context("[invalid-input] profile_tangents must be an array")?;
                    ensure!(values.len() == profile.len(), "[invalid-input] profile_tangents must contain one slope per profile point");
                    values.iter().enumerate().map(|(index, value)| {
                        let slope = value.as_f64().with_context(|| format!("[invalid-input] profile tangent {index} must be a number"))?;
                        ensure!(slope.is_finite() && slope.abs() <= MAX_PROFILE_TANGENT, "[invalid-input] profile tangent {index} must be within -{MAX_PROFILE_TANGENT}..={MAX_PROFILE_TANGENT} mm/mm");
                        Ok(slope)
                    }).collect::<Result<Vec<_>>>()
                })
                .transpose()?;
            let curve = bottle_curve(&profile, tangents.as_deref());
            lathe(&mut model, &curve, segment_count);
            if let Some(band) = parameters.get("label_band_mm") {
                let values = band
                    .as_array()
                    .filter(|values| values.len() == 2)
                    .context("[invalid-input] label_band_mm must be [low, high]")?;
                let low = values[0]
                    .as_f64()
                    .context("[invalid-input] label band low edge must be a number")?;
                let high = values[1]
                    .as_f64()
                    .context("[invalid-input] label band high edge must be a number")?;
                ensure!(
                    low >= profile[0].1 && high <= profile.last().unwrap().1,
                    "[invalid-input] label_band_mm must stay within the bottle profile"
                );
                split_lathe_material_band(&mut model, 0, "label-band", low, high)?;
            }
            if let Some(cap) = parameters.get("cap_mm") {
                let cap_radius = number(cap, "radius_mm")?;
                let cap_height = number(cap, "height_mm")?;
                let cap_offset = cap
                    .get("offset_mm")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0);
                ensure!(
                    cap_height <= height,
                    "[invalid-input] bottle cap height cannot exceed bottle height"
                );
                ensure!(
                    cap_radius >= profile.last().unwrap().0,
                    "[invalid-input] bottle cap radius must cover the final profile radius"
                );
                let cap_slot = model
                    .materials
                    .iter()
                    .position(|material| material.name == "cap")
                    .context("[invalid-input] a bottle with cap_mm requires the `cap` material slot")?;
                // The skirt overlaps the bottle neck by its own height. Its
                // wall and crown are one named primitive/material region.
                let skirt_bottom = (height - cap_height + cap_offset).max(0.0);
                lathe(
                    &mut model,
                    &[(cap_radius, 0.0), (cap_radius, cap_height)],
                    segment_count,
                );
                let primitive_index = model.primitives.len() - 1;
                let skirt_instance_index = model.instances.len() - 1;
                for position in &mut model.primitives[primitive_index].positions {
                    position[1] += skirt_bottom as f32;
                }
                model.primitives[primitive_index].name = "bottle-cap-skirt".into();
                model.primitives[primitive_index].material = Some(cap_slot);
                let ring_count = segment_count + 1;
                let mut positions = Vec::with_capacity(ring_count + 1);
                let mut normals = Vec::with_capacity(ring_count + 1);
                let mut uvs = Vec::with_capacity(ring_count + 1);
                positions.push([0.0, (height + cap_offset) as f32, 0.0]);
                normals.push([0.0, 1.0, 0.0]);
                uvs.push([0.5, 0.5]);
                for step in 0..=segment_count {
                    let theta = std::f64::consts::TAU * step as f64 / segment_count as f64;
                    positions.push([
                        (cap_radius * theta.cos()) as f32,
                        (height + cap_offset) as f32,
                        (cap_radius * theta.sin()) as f32,
                    ]);
                    normals.push([0.0, 1.0, 0.0]);
                    uvs.push([
                        (0.5 + 0.5 * theta.cos()) as f32,
                        (0.5 + 0.5 * theta.sin()) as f32,
                    ]);
                }
                let mut indices = Vec::with_capacity(segment_count * 3);
                for step in 0..segment_count {
                    let a = step as u32 + 1;
                    indices.extend_from_slice(&[0, a + 1, a]);
                }
                let crown = model.primitives.len();
                model.primitives.push(Primitive {
                    name: "bottle-cap-crown".into(),
                    positions,
                    normals,
                    uvs: vec![uvs],
                    indices,
                    material: Some(cap_slot),
                });
                model.instances.push(Instance {
                    id: "bottle-cap-crown".into(),
                    primitive: crown,
                    transform: IDENTITY,
                });
                // `lathe` added the skirt instance immediately before the
                // crown; give it a stable selectable name too.
                model.instances[skirt_instance_index].id = "bottle-cap-skirt".into();
            }
        }
        other => bail!("[unsupported-capability] product form kind `{other}` is not supported (box, pouch, cylinder, tube, lathe-bottle)"),
    }
    model.report.vertices = model.primitives.iter().map(|p| p.positions.len()).sum();
    model.report.triangles = model.primitives.iter().map(Primitive::triangle_count).sum();
    ensure!(
        model.report.vertices <= super::model::LIMITS.max_vertices
            && model.report.triangles <= super::model::LIMITS.max_triangles,
        "[limit-exceeded] generated product form exceeds mesh limits"
    );
    Ok(model)
}
