//! Bounded, hostile-input-safe GLB / glTF 2.0 importer.

use super::math::{self, Mat4};
use super::model::*;
use anyhow::{bail, Context, Result};
use base64::Engine;
use serde_json::Value;
use std::collections::HashMap;

const SUPPORTED_EXTENSIONS: &[&str] = &["KHR_materials_emissive_strength"];

fn arr<'a>(root: &'a Value, key: &str) -> Result<&'a [Value]> {
    match root.get(key) {
        None => Ok(&[]),
        Some(Value::Array(a)) => Ok(a),
        Some(_) => bail!("[malformed-resource] glTF `{key}` must be an array"),
    }
}

fn index(value: &Value, what: &str, len: usize) -> Result<usize> {
    let i = value
        .as_u64()
        .with_context(|| format!("[malformed-resource] glTF {what} index is not an integer"))?;
    if i >= len as u64 {
        bail!("[malformed-resource] glTF {what} index {i} is out of range (0..{len})");
    }
    Ok(i as usize)
}

fn number(value: &Value, default: f64) -> f64 {
    value.as_f64().filter(|v| v.is_finite()).unwrap_or(default)
}

fn floats<const N: usize>(value: Option<&Value>, default: [f64; N]) -> Result<[f64; N]> {
    let Some(value) = value else {
        return Ok(default);
    };
    let items = value
        .as_array()
        .filter(|a| a.len() == N)
        .with_context(|| format!("[malformed-resource] glTF expected an array of {N} numbers"))?;
    let mut out = [0.0; N];
    for (slot, item) in out.iter_mut().zip(items) {
        *slot = item
            .as_f64()
            .filter(|v| v.is_finite())
            .context("[malformed-resource] glTF number is not finite")?;
    }
    Ok(out)
}

fn split_glb(bytes: &[u8]) -> Result<(Value, Option<Vec<u8>>)> {
    if bytes.len() < 20 {
        bail!("[malformed-resource] GLB is truncated");
    }
    let word =
        |at: usize| u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    if word(4) != 2 {
        bail!("[unsupported-capability] only GLB version 2 is supported");
    }
    if word(8) as usize != bytes.len() {
        bail!("[malformed-resource] GLB length field does not match the file size");
    }
    let mut at = 12usize;
    let mut json: Option<Value> = None;
    let mut bin: Option<Vec<u8>> = None;
    let mut chunks = 0;
    while at < bytes.len() {
        chunks += 1;
        if chunks > 8 {
            bail!("[malformed-resource] GLB has too many chunks");
        }
        let header_end = at
            .checked_add(8)
            .filter(|&e| e <= bytes.len())
            .context("[malformed-resource] GLB chunk header is truncated")?;
        let length = word(at) as usize;
        let kind = word(at + 4);
        let end = header_end
            .checked_add(length)
            .filter(|&e| e <= bytes.len())
            .context("[malformed-resource] GLB chunk exceeds the file")?;
        let body = &bytes[header_end..end];
        match kind {
            0x4E4F_534A if json.is_none() => {
                json = Some(
                    serde_json::from_slice(body)
                        .context("[malformed-resource] GLB JSON chunk is invalid")?,
                );
            }
            0x004E_4942 if bin.is_none() => bin = Some(body.to_vec()),
            _ => {}
        }
        at = end;
    }
    Ok((
        json.context("[malformed-resource] GLB has no JSON chunk")?,
        bin,
    ))
}

fn data_uri(uri: &str) -> Result<Option<Vec<u8>>> {
    let Some(rest) = uri.strip_prefix("data:") else {
        return Ok(None);
    };
    let (meta, payload) = rest
        .split_once(',')
        .context("[malformed-resource] malformed data URI")?;
    if !meta.ends_with(";base64") {
        bail!("[unsupported-capability] only base64 data URIs are supported");
    }
    if payload.len() / 4 * 3 > LIMITS.max_bytes {
        bail!(
            "[limit-exceeded] data URI exceeds the {} byte limit",
            LIMITS.max_bytes
        );
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload.as_bytes())
        .context("[malformed-resource] data URI is not valid base64")?;
    Ok(Some(bytes))
}

type AccessorInfo = (usize, u64, bool, Option<usize>, usize, String);

struct Ctx<'a> {
    root: &'a Value,
    glb_bin: Option<Vec<u8>>,
    resolver: Resolver<'a>,
    buffers: HashMap<usize, Vec<u8>>,
    report: ImportReport,
}

impl Ctx<'_> {
    fn buffer(&mut self, i: usize) -> Result<&[u8]> {
        if !self.buffers.contains_key(&i) {
            let buffers = arr(self.root, "buffers")?;
            let entry = buffers
                .get(i)
                .context("[malformed-resource] glTF buffer index is out of range")?;
            let declared = entry.get("byteLength").and_then(Value::as_u64).unwrap_or(0) as usize;
            if declared > LIMITS.max_bytes {
                bail!("[limit-exceeded] glTF buffer {i} declares {declared} bytes");
            }
            let bytes = match entry.get("uri").and_then(Value::as_str) {
                None => {
                    if i != 0 {
                        bail!("[malformed-resource] glTF buffer {i} has no uri");
                    }
                    self.glb_bin.clone().context(
                        "[malformed-resource] glTF buffer 0 has no uri and no GLB BIN chunk",
                    )?
                }
                Some(uri) => match data_uri(uri)? {
                    Some(b) => b,
                    None => (self.resolver)(uri)?,
                },
            };
            if bytes.len() < declared {
                bail!("[malformed-resource] glTF buffer {i} is shorter than its byteLength");
            }
            self.buffers.insert(i, bytes);
        }
        Ok(&self.buffers[&i])
    }

    fn view_bytes(&mut self, view: usize) -> Result<Vec<u8>> {
        let views = arr(self.root, "bufferViews")?;
        let v = views
            .get(view)
            .context("[malformed-resource] glTF bufferView index is out of range")?;
        let buffer = index(&v["buffer"], "buffer", arr(self.root, "buffers")?.len())?;
        let offset = v["byteOffset"].as_u64().unwrap_or(0) as usize;
        let length = v["byteLength"]
            .as_u64()
            .context("[malformed-resource] glTF bufferView has no byteLength")?
            as usize;
        let data = self.buffer(buffer)?;
        let end = offset
            .checked_add(length)
            .filter(|&e| e <= data.len())
            .context("[malformed-resource] glTF bufferView exceeds its buffer")?;
        Ok(data[offset..end].to_vec())
    }

    /// Read an accessor as `components`-wide float tuples (normalized ints decoded).
    fn floats(&mut self, accessor: usize, components: usize) -> Result<Vec<f32>> {
        let (count, ctype, normalized, view, offset, kind) = self.accessor_info(accessor)?;
        let width = match kind.as_str() {
            "SCALAR" => 1,
            "VEC2" => 2,
            "VEC3" => 3,
            "VEC4" => 4,
            other => {
                bail!("[unsupported-capability] glTF accessor type {other} is unsupported here")
            }
        };
        if width != components {
            bail!(
                "[malformed-resource] glTF accessor {accessor} has {width} components, expected {components}"
            );
        }
        let csize = component_size(ctype)?;
        if count > LIMITS.max_vertices {
            bail!("[limit-exceeded] glTF accessor {accessor} has {count} elements");
        }
        let (stride, bytes) = self.accessor_data(view, offset, count, width * csize)?;
        let mut out = Vec::with_capacity(count * width);
        for n in 0..count {
            for c in 0..width {
                let at = n * stride + c * csize;
                out.push(decode_component(&bytes[at..at + csize], ctype, normalized));
            }
        }
        Ok(out)
    }

    fn indices(&mut self, accessor: usize) -> Result<Vec<u32>> {
        let (count, ctype, _, view, offset, kind) = self.accessor_info(accessor)?;
        if kind != "SCALAR" {
            bail!("[malformed-resource] glTF index accessor must be SCALAR");
        }
        let csize = match ctype {
            5121 => 1,
            5123 => 2,
            5125 => 4,
            _ => bail!("[malformed-resource] glTF index component type must be unsigned"),
        };
        if count > LIMITS.max_triangles * 3 {
            bail!("[limit-exceeded] glTF index accessor has {count} indices");
        }
        let (stride, bytes) = self.accessor_data(view, offset, count, csize)?;
        let mut out = Vec::with_capacity(count);
        for n in 0..count {
            let at = n * stride;
            out.push(match csize {
                1 => bytes[at] as u32,
                2 => u16::from_le_bytes([bytes[at], bytes[at + 1]]) as u32,
                _ => u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]),
            });
        }
        Ok(out)
    }

    fn accessor_info(&self, i: usize) -> Result<AccessorInfo> {
        let accessors = arr(self.root, "accessors")?;
        let a = accessors
            .get(i)
            .context("[malformed-resource] glTF accessor index is out of range")?;
        if a.get("sparse").is_some() {
            bail!("[unsupported-capability] glTF sparse accessors are not supported");
        }
        let count = a["count"]
            .as_u64()
            .context("[malformed-resource] glTF accessor has no count")?
            as usize;
        let ctype = a["componentType"]
            .as_u64()
            .context("[malformed-resource] glTF accessor has no componentType")?;
        let view = match a.get("bufferView") {
            Some(v) => Some(index(
                v,
                "bufferView",
                arr(self.root, "bufferViews")?.len(),
            )?),
            None => None,
        };
        let offset = a["byteOffset"].as_u64().unwrap_or(0) as usize;
        let kind = a["type"]
            .as_str()
            .context("[malformed-resource] glTF accessor has no type")?
            .to_string();
        Ok((count, ctype, a["normalized"] == true, view, offset, kind))
    }

    /// Returns `(stride, bytes)`; `bytes` starts at the first element.
    fn accessor_data(
        &mut self,
        view: Option<usize>,
        offset: usize,
        count: usize,
        element: usize,
    ) -> Result<(usize, Vec<u8>)> {
        let Some(view) = view else {
            // No bufferView means all zeros per the glTF spec.
            return Ok((element, vec![0; count * element]));
        };
        let views = arr(self.root, "bufferViews")?;
        let stride = views[view]["byteStride"]
            .as_u64()
            .map(|s| s as usize)
            .filter(|&s| s != 0)
            .unwrap_or(element);
        if stride < element || stride > 252 {
            bail!("[malformed-resource] glTF bufferView byteStride is invalid");
        }
        let data = self.view_bytes(view)?;
        if count == 0 {
            return Ok((stride, Vec::new()));
        }
        let last = (count - 1)
            .checked_mul(stride)
            .and_then(|v| v.checked_add(offset))
            .and_then(|v| v.checked_add(element))
            .context("[malformed-resource] glTF accessor size overflows")?;
        if last > data.len() {
            bail!("[malformed-resource] glTF accessor reads past its bufferView");
        }
        Ok((stride, data[offset..last].to_vec()))
    }
}

fn component_size(ctype: u64) -> Result<usize> {
    Ok(match ctype {
        5120 | 5121 => 1,
        5122 | 5123 => 2,
        5125 | 5126 => 4,
        other => bail!("[malformed-resource] glTF componentType {other} is invalid"),
    })
}

fn decode_component(bytes: &[u8], ctype: u64, normalized: bool) -> f32 {
    match ctype {
        5126 => f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        5121 => bytes[0] as f32 / if normalized { 255.0 } else { 1.0 },
        5120 => (bytes[0] as i8 as f32 / if normalized { 127.0 } else { 1.0 }).max(-1.0),
        5123 => {
            u16::from_le_bytes([bytes[0], bytes[1]]) as f32 / if normalized { 65535.0 } else { 1.0 }
        }
        5122 => (i16::from_le_bytes([bytes[0], bytes[1]]) as f32
            / if normalized { 32767.0 } else { 1.0 })
        .max(-1.0),
        _ => u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f32,
    }
}

/// Resolve a glTF textureInfo into a decoded, de-duplicated texture reference.
fn texture_ref(
    ctx: &mut Ctx,
    model: &mut Model,
    cache: &mut HashMap<usize, usize>,
    budget: &mut usize,
    info: Option<&Value>,
) -> Result<Option<TexRef>> {
    let Some(info) = info else {
        return Ok(None);
    };
    let textures = arr(ctx.root, "textures")?;
    let t = index(&info["index"], "texture", textures.len())?;
    let uv_set = info["texCoord"].as_u64().unwrap_or(0) as usize;
    if uv_set > 1 {
        bail!("[unsupported-capability] glTF texCoord {uv_set} is unsupported (0 and 1 only)");
    }
    if let Some(&done) = cache.get(&t) {
        return Ok(Some(TexRef {
            texture: done,
            uv_set,
            wrap: Default::default(),
            projection: Default::default(),
        }));
    }
    if model.textures.len() >= LIMITS.max_textures {
        bail!(
            "[limit-exceeded] glTF uses more than {} textures",
            LIMITS.max_textures
        );
    }
    let images = arr(ctx.root, "images")?;
    let source = index(&textures[t]["source"], "image", images.len())?;
    let image = &images[source];
    let encoded = if let Some(uri) = image.get("uri").and_then(Value::as_str) {
        match data_uri(uri)? {
            Some(b) => b,
            None => (ctx.resolver)(uri)?,
        }
    } else if let Some(view) = image.get("bufferView") {
        let view = index(view, "bufferView", arr(ctx.root, "bufferViews")?.len())?;
        ctx.view_bytes(view)?
    } else {
        bail!("[malformed-resource] glTF image has neither uri nor bufferView");
    };
    let (_, pixels) = crate::image::decode_source_pixels(&encoded)
        .context("[malformed-resource] glTF texture could not be decoded")?;
    *budget += pixels.as_raw().len();
    if *budget > LIMITS.max_texture_bytes {
        bail!(
            "[limit-exceeded] glTF textures exceed {} decoded bytes",
            LIMITS.max_texture_bytes
        );
    }
    let id = model.textures.len();
    model.textures.push(Texture {
        name: image["name"].as_str().unwrap_or("").to_string(),
        width: pixels.width(),
        height: pixels.height(),
        rgba: pixels.into_raw(),
    });
    cache.insert(t, id);
    Ok(Some(TexRef {
        texture: id,
        uv_set,
        wrap: Default::default(),
        projection: Default::default(),
    }))
}

/// Import a `.glb` file or a `.gltf` JSON document.
pub fn import(bytes: &[u8], resolver: Resolver) -> Result<Model> {
    if bytes.len() > LIMITS.max_bytes {
        bail!(
            "[limit-exceeded] model exceeds the {} byte limit",
            LIMITS.max_bytes
        );
    }
    let is_glb = bytes.starts_with(b"glTF");
    let (root, glb_bin) = if is_glb {
        split_glb(bytes)?
    } else {
        (
            serde_json::from_slice::<Value>(bytes)
                .context("[malformed-resource] glTF JSON is invalid")?,
            None,
        )
    };
    let version = root["asset"]["version"].as_str().unwrap_or("");
    if !version.starts_with("2.") {
        bail!("[unsupported-capability] only glTF 2.x is supported (found `{version}`)");
    }
    for required in arr(&root, "extensionsRequired")? {
        let name = required.as_str().unwrap_or("");
        if !SUPPORTED_EXTENSIONS.contains(&name) {
            bail!("[unsupported-capability] glTF requires the unsupported extension {name}");
        }
    }
    let mut ctx = Ctx {
        root: &root,
        glb_bin,
        resolver,
        buffers: HashMap::new(),
        report: ImportReport {
            format: if is_glb { "glb" } else { "gltf" }.into(),
            ..Default::default()
        },
    };
    for used in arr(&root, "extensionsUsed")? {
        let name = used.as_str().unwrap_or("");
        if !SUPPORTED_EXTENSIONS.contains(&name) {
            ctx.report.unsupported.push(format!("extension {name}"));
        }
    }
    for key in ["animations", "skins", "cameras"] {
        if !arr(&root, key)?.is_empty() {
            ctx.report.unsupported.push(format!("{key} (ignored)"));
        }
    }

    // Materials and textures first so primitives can reference them.
    let mut model = Model::default();
    let material_defs = arr(&root, "materials")?;
    if material_defs.len() > LIMITS.max_materials {
        bail!(
            "[limit-exceeded] glTF has {} materials",
            material_defs.len()
        );
    }
    let mut cache: HashMap<usize, usize> = HashMap::new();
    let mut budget = 0usize;
    for def in material_defs {
        let pbr = &def["pbrMetallicRoughness"];
        let base = floats(pbr.get("baseColorFactor"), [1.0, 1.0, 1.0, 1.0])?;
        let emissive = floats(def.get("emissiveFactor"), [0.0; 3])?;
        let strength = number(
            &def["extensions"]["KHR_materials_emissive_strength"]["emissiveStrength"],
            1.0,
        );
        let alpha_mode = match def["alphaMode"].as_str().unwrap_or("OPAQUE") {
            "OPAQUE" => AlphaMode::Opaque,
            "MASK" => AlphaMode::Mask(number(&def["alphaCutoff"], 0.5) as f32),
            "BLEND" => AlphaMode::Blend,
            other => bail!("[malformed-resource] glTF alphaMode {other} is invalid"),
        };
        let mut tex =
            |info: Option<&Value>| texture_ref(&mut ctx, &mut model, &mut cache, &mut budget, info);
        let base_color_texture = tex(pbr.get("baseColorTexture"))?;
        let metallic_roughness_texture = tex(pbr.get("metallicRoughnessTexture"))?;
        let normal_texture = tex(def.get("normalTexture"))?;
        let occlusion_texture = tex(def.get("occlusionTexture"))?;
        let emissive_texture = tex(def.get("emissiveTexture"))?;
        model.materials.push(Material {
            name: def["name"].as_str().unwrap_or("").to_string(),
            base_color: [
                base[0] as f32,
                base[1] as f32,
                base[2] as f32,
                base[3] as f32,
            ],
            base_color_texture,
            metallic: number(&pbr["metallicFactor"], 1.0).clamp(0.0, 1.0) as f32,
            roughness: number(&pbr["roughnessFactor"], 1.0).clamp(0.0, 1.0) as f32,
            metallic_roughness_texture,
            normal_texture,
            normal_scale: number(&def["normalTexture"]["scale"], 1.0) as f32,
            occlusion_texture,
            occlusion_strength: number(&def["occlusionTexture"]["strength"], 1.0).clamp(0.0, 1.0)
                as f32,
            emissive: [
                (emissive[0] * strength) as f32,
                (emissive[1] * strength) as f32,
                (emissive[2] * strength) as f32,
            ],
            emissive_texture,
            alpha_mode,
            double_sided: def["doubleSided"] == true,
        });
    }

    // Meshes -> primitives. `mesh_prims[m]` lists primitive indices of mesh m.
    let meshes = arr(&root, "meshes")?;
    let accessor_count = arr(&root, "accessors")?.len();
    let mut mesh_prims: Vec<Vec<usize>> = Vec::with_capacity(meshes.len());
    let mut total_vertices = 0usize;
    for (mi, mesh) in meshes.iter().enumerate() {
        let mut list = Vec::new();
        for (pi, prim) in arr(mesh, "primitives")?.iter().enumerate() {
            if model.primitives.len() >= LIMITS.max_primitives {
                bail!(
                    "[limit-exceeded] glTF has more than {} primitives",
                    LIMITS.max_primitives
                );
            }
            let mode = prim["mode"].as_u64().unwrap_or(4);
            if mode > 6 {
                bail!("[malformed-resource] glTF primitive mode {mode} is invalid");
            }
            if mode < 4 {
                ctx.report.unsupported.push(format!(
                    "mesh {mi} primitive {pi}: points/lines are not rendered"
                ));
                continue;
            }
            let attrs = prim
                .get("attributes")
                .and_then(Value::as_object)
                .context("[malformed-resource] glTF primitive has no attributes")?;
            let position = index(
                attrs
                    .get("POSITION")
                    .context("[malformed-resource] glTF primitive has no POSITION")?,
                "accessor",
                accessor_count,
            )?;
            let flat = ctx.floats(position, 3)?;
            let vertex_count = flat.len() / 3;
            total_vertices += vertex_count;
            if total_vertices > LIMITS.max_vertices {
                bail!(
                    "[limit-exceeded] glTF has more than {} vertices",
                    LIMITS.max_vertices
                );
            }
            let positions: Vec<[f32; 3]> =
                flat.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
            if positions.iter().flatten().any(|v| !v.is_finite()) {
                bail!("[malformed-resource] glTF positions contain non-finite values");
            }
            let normals = match attrs.get("NORMAL") {
                Some(n) => {
                    let n = index(n, "accessor", accessor_count)?;
                    let flat = ctx.floats(n, 3)?;
                    if flat.len() / 3 != vertex_count {
                        bail!("[malformed-resource] glTF NORMAL count differs from POSITION");
                    }
                    flat.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect()
                }
                None => Vec::new(),
            };
            let mut uvs = Vec::new();
            for set in 0..2 {
                if let Some(a) = attrs.get(&format!("TEXCOORD_{set}")) {
                    let a = index(a, "accessor", accessor_count)?;
                    let flat = ctx.floats(a, 2)?;
                    if flat.len() / 2 != vertex_count {
                        bail!(
                            "[malformed-resource] glTF TEXCOORD_{set} count differs from POSITION"
                        );
                    }
                    uvs.push(flat.chunks_exact(2).map(|c| [c[0], c[1]]).collect());
                }
            }
            let raw = match prim.get("indices") {
                Some(i) => ctx.indices(index(i, "accessor", accessor_count)?)?,
                None => (0..vertex_count as u32).collect(),
            };
            let tris = triangulate(&raw, mode)?;
            if tris.iter().any(|&i| i as usize >= vertex_count) {
                bail!("[malformed-resource] glTF index refers past the vertex list");
            }
            let material = match prim.get("material") {
                Some(m) => Some(index(m, "material", model.materials.len())?),
                None => None,
            };
            model.primitives.push(Primitive {
                name: mesh["name"].as_str().unwrap_or("").to_string(),
                positions,
                normals,
                uvs,
                indices: tris,
                material,
            });
            list.push(model.primitives.len() - 1);
        }
        mesh_prims.push(list);
    }

    // Node tree -> instances with world transforms (iterative, depth bounded).
    let nodes = arr(&root, "nodes")?;
    if nodes.len() > LIMITS.max_nodes {
        bail!("[limit-exceeded] glTF has {} nodes", nodes.len());
    }
    let mut is_child = vec![false; nodes.len()];
    for node in nodes {
        for c in arr(node, "children")? {
            is_child[index(c, "node", nodes.len())?] = true;
        }
    }
    let scenes = arr(&root, "scenes")?;
    let roots: Vec<usize> = if scenes.is_empty() {
        (0..nodes.len()).filter(|&n| !is_child[n]).collect()
    } else {
        let s = match root.get("scene") {
            Some(s) => index(s, "scene", scenes.len())?,
            None => 0,
        };
        arr(&scenes[s], "nodes")?
            .iter()
            .map(|n| index(n, "node", nodes.len()))
            .collect::<Result<_>>()?
    };
    let mut visited = vec![false; nodes.len()];
    let mut stack: Vec<(usize, Mat4, usize)> = roots
        .iter()
        .rev()
        .map(|&n| (n, math::IDENTITY, 0))
        .collect();
    while let Some((n, parent, depth)) = stack.pop() {
        if depth >= LIMITS.max_depth {
            bail!(
                "[limit-exceeded] glTF node tree is deeper than {}",
                LIMITS.max_depth
            );
        }
        if std::mem::replace(&mut visited[n], true) {
            bail!("[malformed-resource] glTF node {n} is referenced more than once (cycle or shared node)");
        }
        let node = &nodes[n];
        let local = if node.get("matrix").is_some() {
            floats::<16>(node.get("matrix"), math::IDENTITY)?
        } else {
            let t = floats(node.get("translation"), [0.0; 3])?;
            let r = floats(node.get("rotation"), [0.0, 0.0, 0.0, 1.0])?;
            let s = floats(node.get("scale"), [1.0; 3])?;
            math::from_trs(t, r, s)
        };
        let world = math::mul(&parent, &local);
        if let Some(m) = node.get("mesh") {
            let m = index(m, "mesh", mesh_prims.len())?;
            for (k, &primitive) in mesh_prims[m].iter().enumerate() {
                if model.instances.len() >= LIMITS.max_instances {
                    bail!(
                        "[limit-exceeded] glTF has more than {} instances",
                        LIMITS.max_instances
                    );
                }
                let base = node["name"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("node-{n}"));
                let id = if mesh_prims[m].len() > 1 {
                    format!("{base}.{k}")
                } else {
                    base
                };
                model.instances.push(Instance {
                    id,
                    primitive,
                    transform: world,
                });
            }
        }
        for c in arr(node, "children")?.iter().rev() {
            stack.push((index(c, "node", nodes.len())?, world, depth + 1));
        }
    }
    model.report = ctx.report;
    model.finish()
}

/// Convert triangles, strips and fans to a plain triangle list.
fn triangulate(raw: &[u32], mode: u64) -> Result<Vec<u32>> {
    let mut out = Vec::new();
    match mode {
        4 => {
            if raw.len() % 3 != 0 {
                bail!("[malformed-resource] glTF triangle index count is not a multiple of 3");
            }
            out.extend_from_slice(raw);
        }
        5 => {
            for i in 2..raw.len() {
                if i % 2 == 0 {
                    out.extend_from_slice(&[raw[i - 2], raw[i - 1], raw[i]]);
                } else {
                    out.extend_from_slice(&[raw[i - 1], raw[i - 2], raw[i]]);
                }
            }
        }
        _ => {
            for i in 2..raw.len() {
                out.extend_from_slice(&[raw[0], raw[i - 1], raw[i]]);
            }
        }
    }
    if out.len() / 3 > LIMITS.max_triangles {
        bail!(
            "[limit-exceeded] glTF primitive has more than {} triangles",
            LIMITS.max_triangles
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: &str) -> Result<Vec<u8>> {
        bail!("[path-escape] no external resources in tests")
    }

    /// One triangle: 3 positions then 3 u16 indices, as a data-URI buffer.
    fn triangle_gltf(extra: &str) -> String {
        let mut buf = Vec::new();
        for p in [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
            for c in p {
                buf.extend_from_slice(&c.to_le_bytes());
            }
        }
        for i in [0u16, 1, 2] {
            buf.extend_from_slice(&i.to_le_bytes());
        }
        buf.extend_from_slice(&[0, 0]);
        let b64 = base64::engine::general_purpose::STANDARD.encode(&buf);
        format!(
            r#"{{"asset":{{"version":"2.0"}},{extra}
"scene":0,"scenes":[{{"nodes":[0]}}],
"nodes":[{{"name":"tri","mesh":0}}],
"meshes":[{{"primitives":[{{"attributes":{{"POSITION":0}},"indices":1}}]}}],
"accessors":[{{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"}},
 {{"bufferView":1,"componentType":5123,"count":3,"type":"SCALAR"}}],
"bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":36}},{{"buffer":0,"byteOffset":36,"byteLength":6}}],
"buffers":[{{"byteLength":44,"uri":"data:application/octet-stream;base64,{b64}"}}]}}"#
        )
    }

    #[test]
    fn imports_a_triangle() {
        let m = import(triangle_gltf("").as_bytes(), &none).unwrap();
        assert_eq!(m.report.triangles, 1);
        assert_eq!(m.instances[0].id, "tri");
        assert_eq!(m.primitives[0].normals.len(), 3);
        // Smooth normal of a CCW XY triangle points at +Z.
        assert!((m.primitives[0].normals[0][2] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn glb_container_round_trips() {
        let json = triangle_gltf("");
        let mut padded = json.into_bytes();
        while padded.len() % 4 != 0 {
            padded.push(b' ');
        }
        let total = 12 + 8 + padded.len();
        let mut glb = Vec::new();
        glb.extend_from_slice(b"glTF");
        glb.extend_from_slice(&2u32.to_le_bytes());
        glb.extend_from_slice(&(total as u32).to_le_bytes());
        glb.extend_from_slice(&(padded.len() as u32).to_le_bytes());
        glb.extend_from_slice(&0x4E4F_534Au32.to_le_bytes());
        glb.extend_from_slice(&padded);
        assert_eq!(import(&glb, &none).unwrap().report.format, "glb");
        // Truncation and a lying length field must fail, not panic.
        assert!(import(&glb[..glb.len() - 5], &none).is_err());
        let mut lying = glb.clone();
        lying[8] = lying[8].wrapping_add(4);
        assert!(import(&lying, &none).is_err());
        let mut huge = glb;
        huge[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(import(&huge, &none).is_err());
    }

    #[test]
    fn rejects_hostile_documents() {
        for (case, doc) in [
            ("version", triangle_gltf("").replace("\"2.0\"", "\"1.0\"")),
            (
                "required extension",
                triangle_gltf(r#""extensionsRequired":["KHR_draco_mesh_compression"],"#),
            ),
            (
                "index range",
                triangle_gltf("").replace(
                    "\"count\":3,\"type\":\"SCALAR\"",
                    "\"count\":300,\"type\":\"SCALAR\"",
                ),
            ),
            (
                "accessor view",
                triangle_gltf("").replace("\"bufferView\":1,", "\"bufferView\":9,"),
            ),
            (
                "cycle",
                triangle_gltf("").replace(
                    "\"nodes\":[{\"name\":\"tri\",\"mesh\":0}]",
                    "\"nodes\":[{\"children\":[0],\"mesh\":0}]",
                ),
            ),
            (
                "huge buffer",
                triangle_gltf("").replace("\"byteLength\":44", "\"byteLength\":99999999999"),
            ),
        ] {
            assert!(
                import(doc.as_bytes(), &none).is_err(),
                "{case} must be rejected"
            );
        }
    }

    #[test]
    fn external_buffers_go_through_the_resolver() {
        let doc = triangle_gltf("");
        let marker = "data:application/octet-stream;base64,";
        let start = doc.find(marker).unwrap() + marker.len();
        let end = start + doc[start..].find('"').unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&doc[start..end])
            .unwrap();
        let external = format!("{}x.bin{}", &doc[..start - marker.len()], &doc[end..]);
        let resolver = move |uri: &str| -> Result<Vec<u8>> {
            assert_eq!(uri, "x.bin");
            Ok(bytes.clone())
        };
        assert_eq!(
            import(external.as_bytes(), &resolver)
                .unwrap()
                .report
                .triangles,
            1
        );
        assert!(import(external.as_bytes(), &none).is_err());
    }
}
