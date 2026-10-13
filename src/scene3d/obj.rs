//! Bounded Wavefront OBJ + MTL importer.

use super::math;
use super::model::*;
use anyhow::{bail, Context, Result};
use std::collections::HashMap;

fn parse_f32(token: &str, line: usize) -> Result<f32> {
    let v: f32 = token.parse().with_context(|| {
        format!("[malformed-resource] OBJ line {line}: `{token}` is not a number")
    })?;
    if !v.is_finite() {
        bail!("[malformed-resource] OBJ line {line}: non-finite number");
    }
    Ok(v)
}

/// Resolve an OBJ face vertex index (1-based, negative = relative) to 0-based.
fn resolve(token: &str, len: usize, line: usize) -> Result<usize> {
    let n: i64 = token
        .parse()
        .with_context(|| format!("[malformed-resource] OBJ line {line}: bad index `{token}`"))?;
    let i = if n > 0 { n - 1 } else { len as i64 + n };
    if n == 0 || i < 0 || i as usize >= len {
        bail!("[malformed-resource] OBJ line {line}: index {n} is out of range");
    }
    Ok(i as usize)
}

#[derive(Default)]
struct MtlEntry {
    material: Material,
    map_kd: Option<String>,
}

fn parse_mtl(text: &str) -> Result<Vec<(String, MtlEntry)>> {
    let mut out: Vec<(String, MtlEntry)> = Vec::new();
    for (n, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        let mut parts = line.split_whitespace();
        let Some(key) = parts.next() else { continue };
        let args: Vec<&str> = parts.collect();
        if key == "newmtl" {
            if out.len() >= LIMITS.max_materials {
                bail!(
                    "[limit-exceeded] MTL has more than {} materials",
                    LIMITS.max_materials
                );
            }
            let name = args.join(" ");
            let mut entry = MtlEntry::default();
            entry.material.name = name.clone();
            // OBJ materials are non-metal, fairly rough by default.
            entry.material.metallic = 0.0;
            entry.material.roughness = 0.7;
            out.push((name, entry));
            continue;
        }
        let Some((_, entry)) = out.last_mut() else {
            continue;
        };
        let m = &mut entry.material;
        match key {
            "Kd" if args.len() >= 3 => {
                for (c, arg) in args.iter().take(3).enumerate() {
                    m.base_color[c] = parse_f32(arg, n + 1)?.clamp(0.0, 1.0);
                }
            }
            "Ke" if args.len() >= 3 => {
                for (c, arg) in args.iter().take(3).enumerate() {
                    m.emissive[c] = parse_f32(arg, n + 1)?.max(0.0);
                }
            }
            "d" if !args.is_empty() => {
                m.base_color[3] = parse_f32(args[0], n + 1)?.clamp(0.0, 1.0);
                if m.base_color[3] < 1.0 {
                    m.alpha_mode = AlphaMode::Blend;
                }
            }
            "Ns" if !args.is_empty() => {
                // Phong exponent -> roughness (deterministic closed form).
                let ns = parse_f32(args[0], n + 1)?.clamp(0.0, 1000.0);
                m.roughness = (1.0 - (ns / 1000.0).sqrt()).clamp(0.05, 1.0);
            }
            "map_Kd" if !args.is_empty() => {
                entry.map_kd = args.last().map(|s| s.to_string());
            }
            _ => {}
        }
    }
    Ok(out)
}

pub fn import(bytes: &[u8], resolver: Resolver) -> Result<Model> {
    if bytes.len() > LIMITS.max_bytes {
        bail!(
            "[limit-exceeded] model exceeds the {} byte limit",
            LIMITS.max_bytes
        );
    }
    let text = std::str::from_utf8(bytes).context("[malformed-resource] OBJ must be UTF-8 text")?;
    let mut model = Model::default();
    model.report.format = "obj".into();

    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();

    // Materials from every mtllib, keyed by name.
    let mut material_ids: HashMap<String, usize> = HashMap::new();
    let mut texture_cache: HashMap<String, usize> = HashMap::new();
    let mut texture_budget = 0usize;

    // Current group: faces are accumulated per (name, material) with deduped
    // vertex tuples so every group becomes one primitive.
    struct Group {
        name: String,
        material: Option<usize>,
        lookup: HashMap<(usize, Option<usize>, Option<usize>), u32>,
        positions: Vec<[f32; 3]>,
        normals: Vec<[f32; 3]>,
        uvs: Vec<[f32; 2]>,
        indices: Vec<u32>,
        has_normals: bool,
        has_uvs: bool,
    }
    fn fresh(name: &str, material: Option<usize>) -> Group {
        Group {
            name: name.to_string(),
            material,
            lookup: HashMap::new(),
            positions: Vec::new(),
            normals: Vec::new(),
            uvs: Vec::new(),
            indices: Vec::new(),
            has_normals: false,
            has_uvs: false,
        }
    }
    let mut groups: Vec<Group> = Vec::new();
    let mut name = String::from("object");
    let mut material: Option<usize> = None;
    let mut current = fresh(&name, material);
    let mut triangles = 0usize;

    let flush = |current: Group, groups: &mut Vec<Group>| {
        if !current.indices.is_empty() {
            groups.push(current);
        }
    };

    for (n, raw) in text.lines().enumerate() {
        let n = n + 1;
        let line = raw.split('#').next().unwrap_or("").trim();
        let mut parts = line.split_whitespace();
        let Some(key) = parts.next() else { continue };
        let args: Vec<&str> = parts.collect();
        match key {
            "v" if args.len() >= 3 => {
                if positions.len() >= LIMITS.max_vertices {
                    bail!(
                        "[limit-exceeded] OBJ has more than {} vertices",
                        LIMITS.max_vertices
                    );
                }
                positions.push([
                    parse_f32(args[0], n)?,
                    parse_f32(args[1], n)?,
                    parse_f32(args[2], n)?,
                ]);
            }
            "vt" if args.len() >= 2 => {
                if uvs.len() >= LIMITS.max_vertices {
                    bail!("[limit-exceeded] OBJ has too many texture coordinates");
                }
                uvs.push([parse_f32(args[0], n)?, parse_f32(args[1], n)?]);
            }
            "vn" if args.len() >= 3 => {
                if normals.len() >= LIMITS.max_vertices {
                    bail!("[limit-exceeded] OBJ has too many normals");
                }
                normals.push([
                    parse_f32(args[0], n)?,
                    parse_f32(args[1], n)?,
                    parse_f32(args[2], n)?,
                ]);
            }
            "o" | "g" => {
                let new = if args.is_empty() {
                    "object".to_string()
                } else {
                    args.join(" ")
                };
                let old = std::mem::replace(&mut current, fresh(&new, material));
                flush(old, &mut groups);
                name = new;
            }
            "usemtl" => {
                let wanted = args.join(" ");
                material = material_ids.get(&wanted).copied();
                if material.is_none() {
                    model
                        .report
                        .unsupported
                        .push(format!("usemtl {wanted}: material not found"));
                }
                let old = std::mem::replace(&mut current, fresh(&name, material));
                flush(old, &mut groups);
            }
            "mtllib" => {
                let file = args.join(" ");
                let bytes = match resolver(&file) {
                    Ok(b) => b,
                    Err(e) => {
                        model.report.unsupported.push(format!("mtllib {file}: {e}"));
                        continue;
                    }
                };
                let text = std::str::from_utf8(&bytes)
                    .context("[malformed-resource] MTL must be UTF-8 text")?;
                for (mat_name, entry) in parse_mtl(text)? {
                    if model.materials.len() >= LIMITS.max_materials {
                        bail!("[limit-exceeded] OBJ has too many materials");
                    }
                    let mut mat = entry.material;
                    if let Some(tex) = entry.map_kd {
                        let id = match texture_cache.get(&tex) {
                            Some(&id) => Some(id),
                            None if model.textures.len() >= LIMITS.max_textures => {
                                bail!(
                                    "[limit-exceeded] OBJ uses more than {} textures",
                                    LIMITS.max_textures
                                )
                            }
                            None => match resolver(&tex)
                                .and_then(|b| crate::image::decode_source_pixels(&b).map(|r| r.1))
                            {
                                Ok(pixels) => {
                                    texture_budget += pixels.as_raw().len();
                                    if texture_budget > LIMITS.max_texture_bytes {
                                        bail!("[limit-exceeded] OBJ textures are too large");
                                    }
                                    let id = model.textures.len();
                                    model.textures.push(Texture {
                                        name: tex.clone(),
                                        width: pixels.width(),
                                        height: pixels.height(),
                                        rgba: pixels.into_raw(),
                                    });
                                    texture_cache.insert(tex.clone(), id);
                                    Some(id)
                                }
                                Err(e) => {
                                    model.report.unsupported.push(format!("map_Kd {tex}: {e}"));
                                    None
                                }
                            },
                        };
                        mat.base_color_texture = id.map(|texture| TexRef { texture, uv_set: 0 });
                    }
                    material_ids.insert(mat_name, model.materials.len());
                    model.materials.push(mat);
                }
            }
            "f" if args.len() >= 3 => {
                let mut verts = Vec::with_capacity(args.len());
                for token in &args {
                    let mut it = token.split('/');
                    let v = resolve(it.next().unwrap_or(""), positions.len(), n)?;
                    let t = match it.next() {
                        Some("") | None => None,
                        Some(s) => Some(resolve(s, uvs.len(), n)?),
                    };
                    let nn = match it.next() {
                        Some("") | None => None,
                        Some(s) => Some(resolve(s, normals.len(), n)?),
                    };
                    verts.push((v, t, nn));
                }
                // Fan triangulation in declaration order.
                for i in 2..verts.len() {
                    triangles += 1;
                    if triangles > LIMITS.max_triangles {
                        bail!(
                            "[limit-exceeded] OBJ has more than {} triangles",
                            LIMITS.max_triangles
                        );
                    }
                    for key in [verts[0], verts[i - 1], verts[i]] {
                        let g = &mut current;
                        let next = g.positions.len() as u32;
                        let slot = *g.lookup.entry(key).or_insert(next);
                        if slot == next {
                            g.positions.push(positions[key.0]);
                            match key.1 {
                                Some(t) => {
                                    g.uvs.push(uvs[t]);
                                    g.has_uvs = true;
                                }
                                None => g.uvs.push([0.0, 0.0]),
                            }
                            match key.2 {
                                Some(nm) => {
                                    g.normals.push(normals[nm]);
                                    g.has_normals = true;
                                }
                                None => g.normals.push([0.0, 0.0, 0.0]),
                            }
                        }
                        g.indices.push(slot);
                    }
                }
            }
            "v" | "vt" | "vn" | "f" => {
                bail!("[malformed-resource] OBJ line {n}: `{key}` has too few values");
            }
            _ => {}
        }
    }
    flush(current, &mut groups);

    for (k, g) in groups.into_iter().enumerate() {
        let normals = if g.has_normals && g.normals.iter().all(|n| n != &[0.0, 0.0, 0.0]) {
            g.normals
                .iter()
                .map(|n| {
                    let v = math::normalize([n[0] as f64, n[1] as f64, n[2] as f64]);
                    [v[0] as f32, v[1] as f32, v[2] as f32]
                })
                .collect()
        } else {
            Vec::new()
        };
        let primitive = model.primitives.len();
        model.primitives.push(Primitive {
            name: g.name.clone(),
            positions: g.positions,
            normals,
            uvs: if g.has_uvs { vec![g.uvs] } else { Vec::new() },
            indices: g.indices,
            material: g.material,
        });
        model.instances.push(Instance {
            id: if g.name == "object" {
                format!("object-{k}")
            } else {
                g.name
            },
            primitive,
            transform: math::IDENTITY,
        });
    }
    model.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: &str) -> Result<Vec<u8>> {
        bail!("[path-escape] no external resources in tests")
    }

    #[test]
    fn quad_becomes_two_triangles_with_uvs() {
        let obj = "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nvt 0 0\nvt 1 0\nvt 1 1\nvt 0 1\n\
                   o plate\nf 1/1 2/2 3/3 4/4\n";
        let m = import(obj.as_bytes(), &none).unwrap();
        assert_eq!(m.report.triangles, 2);
        assert_eq!(m.instances[0].id, "plate");
        assert_eq!(m.primitives[0].uvs.len(), 1);
    }

    #[test]
    fn negative_indices_and_range_errors() {
        let ok = "v 0 0 0\nv 1 0 0\nv 0 1 0\nf -3 -2 -1\n";
        assert_eq!(import(ok.as_bytes(), &none).unwrap().report.triangles, 1);
        for bad in [
            "v 0 0 0\nf 1 2 3\n",
            "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 0 1 2\n",
            "v 0 0\n",
            "v a b c\n",
        ] {
            assert!(import(bad.as_bytes(), &none).is_err(), "{bad:?}");
        }
        assert!(import(b"", &none).is_err());
    }

    #[test]
    fn missing_mtllib_is_reported_not_fatal() {
        let obj = "mtllib gone.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nusemtl red\nf 1 2 3\n";
        let m = import(obj.as_bytes(), &none).unwrap();
        assert!(m.report.unsupported.iter().any(|u| u.contains("gone.mtl")));
    }
}
