//! Deterministic CPU rasterizer: depth buffer, perspective-correct
//! interpolation, metallic-roughness PBR, directional/point lights, a
//! gradient environment and supersampled anti-aliasing.
//!
//! Only IEEE-754 basic operations, `sqrt` and the crate's own `exp`/`ln` are
//! used, in a fixed order, so output is identical on every platform.

use super::math::{self, Mat4, Vec3};
use super::model::{
    AlphaMode, GraphicBlend, Instance, Material, Model, Texture, TextureProjection, WrapMode,
};
use anyhow::{bail, ensure, Context, Result};

pub const MAX_DIMENSION: u32 = 8192;
pub const MAX_SUPERSAMPLE: u32 = 4;
pub const MAX_LIGHTS: usize = 16;
/// Peak bytes reserved for the CPU framebuffer and resolved image per render.
pub const MAX_RENDER_BUFFER_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub enum Projection {
    Perspective { fov_y_deg: f64 },
    Orthographic { height: f64 },
}

#[derive(Clone, Debug)]
pub struct Camera {
    pub eye: Vec3,
    pub target: Vec3,
    pub up: Vec3,
    pub projection: Projection,
    pub near: f64,
    pub far: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Light {
    /// `direction` is the way the light travels (from the light into the scene).
    Directional {
        direction: Vec3,
        color: [f64; 3],
        intensity: f64,
    },
    Point {
        position: Vec3,
        color: [f64; 3],
        intensity: f64,
    },
    /// A point light limited to a cone. `direction` is normalized; the
    /// cosines of the inner (full) and outer (zero) cone half angles.
    Spot {
        position: Vec3,
        direction: Vec3,
        color: [f64; 3],
        intensity: f64,
        inner_cos: f64,
        outer_cos: f64,
    },
}

#[derive(Clone, Debug)]
pub struct Environment {
    pub sky: [f64; 3],
    pub ground: [f64; 3],
    pub intensity: f64,
}

impl Default for Environment {
    fn default() -> Self {
        Self {
            sky: [0.6, 0.65, 0.75],
            ground: [0.25, 0.22, 0.2],
            intensity: 0.5,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Scene {
    pub camera: Camera,
    pub lights: Vec<Light>,
    pub environment: Environment,
    /// Straight sRGB background; alpha 0 leaves the image transparent.
    pub background: [f64; 4],
    pub exposure_ev: f64,
    /// Shadow map edge in texels; 0 turns shadows off.
    pub shadow_size: u32,
}

#[derive(Clone, Debug)]
pub struct Options {
    pub width: u32,
    pub height: u32,
    /// Supersampling factor per axis, 1..=4.
    pub supersample: u32,
}

/// Straight-alpha sRGB RGBA8, row-major.
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

fn srgb_to_linear(v: f64) -> f64 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        math::pow((v + 0.055) / 1.055, 2.4)
    }
}

fn linear_to_srgb(v: f64) -> f64 {
    let v = math::clamp01(v);
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * math::pow(v, 1.0 / 2.4) - 0.055
    }
}

/// 8-bit sRGB -> linear lookup, built once per render.
fn decode_table() -> [f64; 256] {
    let mut t = [0.0; 256];
    for (i, slot) in t.iter_mut().enumerate() {
        *slot = srgb_to_linear(i as f64 / 255.0);
    }
    t
}

#[derive(Clone, Copy)]
struct Vertex {
    clip: [f64; 4],
    world: Vec3,
    normal: Vec3,
    uv: [[f64; 2]; 2],
}

fn lerp_vertex(a: &Vertex, b: &Vertex, t: f64) -> Vertex {
    let l = |x: f64, y: f64| x + (y - x) * t;
    let mut v = *a;
    for i in 0..4 {
        v.clip[i] = l(a.clip[i], b.clip[i]);
    }
    for i in 0..3 {
        v.world[i] = l(a.world[i], b.world[i]);
        v.normal[i] = l(a.normal[i], b.normal[i]);
    }
    for s in 0..2 {
        for i in 0..2 {
            v.uv[s][i] = l(a.uv[s][i], b.uv[s][i]);
        }
    }
    v
}

/// Sutherland-Hodgman against the near plane (clip.z >= -clip.w).
fn clip_near(poly: &[Vertex]) -> Vec<Vertex> {
    let mut out = Vec::with_capacity(poly.len() + 1);
    for i in 0..poly.len() {
        let a = &poly[i];
        let b = &poly[(i + 1) % poly.len()];
        let da = a.clip[2] + a.clip[3];
        let db = b.clip[2] + b.clip[3];
        if da >= 0.0 {
            out.push(*a);
        }
        if (da >= 0.0) != (db >= 0.0) {
            out.push(lerp_vertex(a, b, da / (da - db)));
        }
    }
    out
}

struct Buffers {
    w: usize,
    h: usize,
    depth: Vec<f64>,
    /// Premultiplied linear RGB + coverage alpha.
    color: Vec<[f64; 4]>,
}

struct Shared<'a> {
    model: &'a Model,
    scene: &'a Scene,
    decode: [f64; 256],
    eye: Vec3,
    exposure: f64,
    /// One entry per light, `None` when shadows are off.
    shadows: Vec<Option<super::shadow::ShadowSet>>,
    bounds: (Vec3, Vec3),
}

fn sample(
    tex: &Texture,
    uv: [f64; 2],
    decode: &[f64; 256],
    srgb: bool,
    mode: &WrapMode,
) -> [f64; 4] {
    let w = tex.width as usize;
    let h = tex.height as usize;
    // Normalize the address mode first, then use bilinear filtering with
    // texel centers at half-integers.
    let address = |v: f64| match mode {
        WrapMode::Repeat => v.rem_euclid(1.0),
        WrapMode::Clamp => v.clamp(0.0, 1.0),
        WrapMode::Mirror => {
            let p = v.rem_euclid(2.0);
            if p <= 1.0 {
                p
            } else {
                2.0 - p
            }
        }
    };
    let fx = address(uv[0]);
    let fy = address(uv[1]);
    let x = fx * w as f64 - 0.5;
    let y = fy * h as f64 - 0.5;
    let x0 = floor(x);
    let y0 = floor(y);
    let tx = x - x0;
    let ty = y - y0;
    let index = |v: f64, n: usize| match mode {
        WrapMode::Repeat => (((v as i64) % n as i64 + n as i64) % n as i64) as usize,
        WrapMode::Clamp | WrapMode::Mirror => (v as i64).clamp(0, n as i64 - 1) as usize,
    };
    let fetch = |xi: f64, yi: f64| {
        let i = (index(yi, h) * w + index(xi, w)) * 4;
        let p = &tex.rgba[i..i + 4];
        let c = |k: usize| {
            if srgb && k < 3 {
                decode[p[k] as usize]
            } else {
                p[k] as f64 / 255.0
            }
        };
        [c(0), c(1), c(2), p[3] as f64 / 255.0]
    };
    let a = fetch(x0, y0);
    let b = fetch(x0 + 1.0, y0);
    let c = fetch(x0, y0 + 1.0);
    let d = fetch(x0 + 1.0, y0 + 1.0);
    let mut out = [0.0; 4];
    for k in 0..4 {
        let top = a[k] + (b[k] - a[k]) * tx;
        let bottom = c[k] + (d[k] - c[k]) * tx;
        out[k] = top + (bottom - top) * ty;
    }
    out
}

fn sample_triplanar(
    tex: &Texture,
    position: Vec3,
    normal: Vec3,
    bounds: (Vec3, Vec3),
    decode: &[f64; 256],
    srgb: bool,
    wrap: &WrapMode,
) -> [f64; 4] {
    let (min, max) = bounds;
    let extent = |axis: usize| (max[axis] - min[axis]).max(1e-9);
    let uv = [
        [
            (position[2] - min[2]) / extent(2),
            (position[1] - min[1]) / extent(1),
        ],
        [
            (position[0] - min[0]) / extent(0),
            (position[2] - min[2]) / extent(2),
        ],
        [
            (position[0] - min[0]) / extent(0),
            (position[1] - min[1]) / extent(1),
        ],
    ];
    // Fourth-power weights keep the projection stable on broad faces while
    // blending smoothly through edges and rounded corners.
    let mut weights = [
        normal[0].abs().powi(4),
        normal[1].abs().powi(4),
        normal[2].abs().powi(4),
    ];
    let total = (weights[0] + weights[1] + weights[2]).max(1e-12);
    for weight in &mut weights {
        *weight /= total;
    }
    let mut out = [0.0; 4];
    for axis in 0..3 {
        let value = sample(tex, uv[axis], decode, srgb, wrap);
        for channel in 0..4 {
            out[channel] += value[channel] * weights[axis];
        }
    }
    out
}

fn floor(x: f64) -> f64 {
    let t = x as i64 as f64;
    if t > x {
        t - 1.0
    } else {
        t
    }
}

fn blend_channel(mode: GraphicBlend, source: f64, destination: f64) -> f64 {
    let (source, destination) = (math::clamp01(source), math::clamp01(destination));
    match mode {
        GraphicBlend::Multiply => source * destination,
        GraphicBlend::Screen => source + destination - source * destination,
        GraphicBlend::Overlay if destination <= 0.5 => 2.0 * source * destination,
        GraphicBlend::Overlay => 1.0 - 2.0 * (1.0 - source) * (1.0 - destination),
        GraphicBlend::Darken => source.min(destination),
        GraphicBlend::Lighten => source.max(destination),
    }
}

fn shade(
    shared: &Shared,
    material: &Material,
    vertex: &Vertex,
    tangent: Option<Vec3>,
    front: bool,
) -> [f64; 4] {
    let model = shared.model;
    let uv_of = |set: usize| vertex.uv[set.min(1)];
    let tex = |r: &Option<super::model::TexRef>, srgb: bool| {
        r.as_ref().map(|r| {
            let texture = &model.textures[r.texture];
            match r.projection {
                TextureProjection::Uv => {
                    sample(texture, uv_of(r.uv_set), &shared.decode, srgb, &r.wrap)
                }
                TextureProjection::Triplanar => sample_triplanar(
                    texture,
                    vertex.world,
                    math::normalize(vertex.normal),
                    shared.bounds,
                    &shared.decode,
                    srgb,
                    &r.wrap,
                ),
            }
        })
    };
    let mut base = [
        material.base_color[0] as f64,
        material.base_color[1] as f64,
        material.base_color[2] as f64,
        material.base_color[3] as f64,
    ];
    if let Some(t) = tex(&material.base_color_texture, true) {
        for k in 0..4 {
            base[k] *= t[k];
        }
    }
    let mut metallic = material.metallic as f64;
    let mut roughness = material.roughness as f64;
    if let Some(t) = tex(&material.metallic_roughness_texture, false) {
        roughness *= t[1];
        metallic *= t[2];
    }
    let roughness = roughness.clamp(0.045, 1.0);
    let metallic = math::clamp01(metallic);
    let mut occlusion = 1.0;
    if let Some(t) = tex(&material.occlusion_texture, false) {
        occlusion = 1.0 + material.occlusion_strength as f64 * (t[0] - 1.0);
    }
    let mut emissive = [
        material.emissive[0] as f64,
        material.emissive[1] as f64,
        material.emissive[2] as f64,
    ];
    if let Some(t) = tex(&material.emissive_texture, true) {
        for k in 0..3 {
            emissive[k] *= t[k];
        }
    }

    let mut n = math::normalize(vertex.normal);
    if !front {
        n = math::scale(n, -1.0);
    }
    if let (Some(t), Some(map)) = (tangent, tex(&material.normal_texture, false)) {
        // Gram-Schmidt the per-triangle tangent against the interpolated normal.
        let t = math::normalize(math::sub(t, math::scale(n, math::dot(n, t))));
        let b = math::cross(n, t);
        let s = material.normal_scale as f64;
        let x = (map[0] * 2.0 - 1.0) * s;
        let y = (map[1] * 2.0 - 1.0) * s;
        let z = map[2] * 2.0 - 1.0;
        n = math::normalize(math::add(
            math::add(math::scale(t, x), math::scale(b, y)),
            math::scale(n, z),
        ));
    }
    let v = math::normalize(math::sub(shared.eye, vertex.world));
    let n_dot_v = math::dot(n, v).max(1e-4);
    let a = roughness * roughness;
    let a2 = a * a;
    let k = (roughness + 1.0) * (roughness + 1.0) / 8.0;
    let f0 = [
        0.04 + (base[0] - 0.04) * metallic,
        0.04 + (base[1] - 0.04) * metallic,
        0.04 + (base[2] - 0.04) * metallic,
    ];
    let diffuse_color = [
        base[0] * (1.0 - metallic),
        base[1] * (1.0 - metallic),
        base[2] * (1.0 - metallic),
    ];
    let mut out = [0.0f64; 3];
    let shadow_catcher = material.name == "ground-shadow-catcher";
    let mut catcher_shadow = 0.0;
    let mut catcher_weight = 0.0;

    for (light_index, light) in shared.scene.lights.iter().enumerate() {
        let (l, radiance) = match light {
            Light::Spot {
                position,
                direction,
                color,
                intensity,
                inner_cos,
                outer_cos,
            } => {
                let d = math::sub(*position, vertex.world);
                let d2 = math::dot(d, d).max(1e-4);
                let l = math::normalize(d);
                let cd = -math::dot(l, *direction);
                let t = math::clamp01((cd - outer_cos) / (inner_cos - outer_cos).max(1e-6));
                let s = intensity / d2 * t * t * (3.0 - 2.0 * t);
                (l, [color[0] * s, color[1] * s, color[2] * s])
            }
            Light::Directional {
                direction,
                color,
                intensity,
            } => (
                math::normalize(math::scale(*direction, -1.0)),
                [
                    color[0] * intensity,
                    color[1] * intensity,
                    color[2] * intensity,
                ],
            ),
            Light::Point {
                position,
                color,
                intensity,
            } => {
                let d = math::sub(*position, vertex.world);
                let d2 = math::dot(d, d).max(1e-4);
                let s = intensity / d2;
                (
                    math::normalize(d),
                    [color[0] * s, color[1] * s, color[2] * s],
                )
            }
        };
        let n_dot_l = math::dot(n, l);
        if n_dot_l <= 0.0 {
            continue;
        }
        let visibility = shared
            .shadows
            .get(light_index)
            .and_then(Option::as_ref)
            .map_or(1.0, |set| set.visibility(vertex.world, n));
        if shadow_catcher {
            let weight = (radiance[0] + radiance[1] + radiance[2]) * n_dot_l;
            catcher_shadow += (1.0 - visibility) * weight;
            catcher_weight += weight;
        }
        let radiance = match shared.shadows.get(light_index).and_then(Option::as_ref) {
            Some(_) => {
                if visibility <= 0.0 {
                    continue;
                }
                [
                    radiance[0] * visibility,
                    radiance[1] * visibility,
                    radiance[2] * visibility,
                ]
            }
            None => radiance,
        };
        let h = math::normalize(math::add(l, v));
        let n_dot_h = math::dot(n, h).max(0.0);
        let v_dot_h = math::dot(v, h).max(0.0);
        let denom = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
        let d = a2 / (math::PI * denom * denom).max(1e-12);
        let g = (n_dot_l / (n_dot_l * (1.0 - k) + k)) * (n_dot_v / (n_dot_v * (1.0 - k) + k));
        let f_pow = math::powi(1.0 - v_dot_h, 5);
        for c in 0..3 {
            let f = f0[c] + (1.0 - f0[c]) * f_pow;
            let spec = d * g * f / (4.0 * n_dot_l * n_dot_v).max(1e-6);
            let diff = (1.0 - f) * diffuse_color[c] / math::PI;
            out[c] += (diff + spec) * radiance[c] * n_dot_l;
        }
    }

    if shadow_catcher {
        let opacity = if catcher_weight > 0.0 {
            (catcher_shadow / catcher_weight).clamp(0.0, 1.0) * base[3]
        } else {
            0.0
        };
        return [base[0], base[1], base[2], opacity];
    }

    // Gradient environment: diffuse from the normal, specular from reflection.
    let env = &shared.scene.environment;
    let gradient = |dir: Vec3| {
        let t = dir[1] * 0.5 + 0.5;
        [
            (env.ground[0] + (env.sky[0] - env.ground[0]) * t) * env.intensity,
            (env.ground[1] + (env.sky[1] - env.ground[1]) * t) * env.intensity,
            (env.ground[2] + (env.sky[2] - env.ground[2]) * t) * env.intensity,
        ]
    };
    let r = math::sub(math::scale(n, 2.0 * math::dot(n, v)), v);
    let amb_d = gradient(n);
    let amb_s = gradient(r);
    let f_env = math::powi(1.0 - n_dot_v, 5);
    for c in 0..3 {
        let f = f0[c] + (math::clamp01(1.0 - roughness).max(f0[c]) - f0[c]) * f_env;
        out[c] += (diffuse_color[c] * amb_d[c] + f * amb_s[c] * (1.0 - roughness * 0.8))
            * occlusion
            + emissive[c];
        out[c] *= shared.exposure;
    }
    [out[0], out[1], out[2], base[3]]
}

fn draw_triangle(
    shared: &Shared,
    buf: &mut Buffers,
    inst_material: &Material,
    verts: [Vertex; 3],
    blend_pass: bool,
) {
    let (w, h) = (buf.w as f64, buf.h as f64);
    // Near-plane clip, then fan the resulting polygon.
    let poly = clip_near(&verts);
    if poly.len() < 3 {
        return;
    }
    for i in 1..poly.len() - 1 {
        raster(
            shared,
            buf,
            inst_material,
            [poly[0], poly[i], poly[i + 1]],
            blend_pass,
            w,
            h,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn raster(
    shared: &Shared,
    buf: &mut Buffers,
    material: &Material,
    tri: [Vertex; 3],
    blend_pass: bool,
    w: f64,
    h: f64,
) {
    let mut sx = [0.0; 3];
    let mut sy = [0.0; 3];
    let mut sz = [0.0; 3];
    let mut inv_w = [0.0; 3];
    for i in 0..3 {
        let cw = tri[i].clip[3];
        if cw <= 1e-12 {
            return;
        }
        inv_w[i] = 1.0 / cw;
        sx[i] = (tri[i].clip[0] * inv_w[i] * 0.5 + 0.5) * w;
        sy[i] = (0.5 - tri[i].clip[1] * inv_w[i] * 0.5) * h;
        sz[i] = tri[i].clip[2] * inv_w[i];
    }
    let area = (sx[1] - sx[0]) * (sy[2] - sy[0]) - (sx[2] - sx[0]) * (sy[1] - sy[0]);
    if area == 0.0 || !area.is_finite() {
        return;
    }
    // Screen y points down, so counter-clockwise (front) has negative area.
    let front = area < 0.0;
    if !front && !material.double_sided {
        return;
    }
    let min_x = floor(sx[0].min(sx[1]).min(sx[2])).max(0.0) as usize;
    let max_x = (sx[0].max(sx[1]).max(sx[2]).ceil()).min(w - 1.0).max(-1.0);
    let min_y = floor(sy[0].min(sy[1]).min(sy[2])).max(0.0) as usize;
    let max_y = (sy[0].max(sy[1]).max(sy[2]).ceil()).min(h - 1.0).max(-1.0);
    if max_x < 0.0 || max_y < 0.0 {
        return;
    }
    let (max_x, max_y) = (max_x as usize, max_y as usize);

    // Per-triangle tangent from the UV set used by the normal map.
    let tangent = material.normal_texture.as_ref().and_then(|r| {
        let s = r.uv_set.min(1);
        let e1 = math::sub(tri[1].world, tri[0].world);
        let e2 = math::sub(tri[2].world, tri[0].world);
        let du1 = tri[1].uv[s][0] - tri[0].uv[s][0];
        let dv1 = tri[1].uv[s][1] - tri[0].uv[s][1];
        let du2 = tri[2].uv[s][0] - tri[0].uv[s][0];
        let dv2 = tri[2].uv[s][1] - tri[0].uv[s][1];
        let det = du1 * dv2 - du2 * dv1;
        if det.abs() < 1e-12 {
            return None;
        }
        let f = 1.0 / det;
        let t = [
            f * (dv2 * e1[0] - dv1 * e2[0]),
            f * (dv2 * e1[1] - dv1 * e2[1]),
            f * (dv2 * e1[2] - dv1 * e2[2]),
        ];
        Some(t)
    });

    let alpha_cutoff = match material.alpha_mode {
        AlphaMode::Mask(c) => Some(c as f64),
        _ => None,
    };
    for y in min_y..=max_y {
        let py = y as f64 + 0.5;
        for x in min_x..=max_x {
            let px = x as f64 + 0.5;
            let e0 = (sx[2] - sx[1]) * (py - sy[1]) - (sy[2] - sy[1]) * (px - sx[1]);
            let e1 = (sx[0] - sx[2]) * (py - sy[2]) - (sy[0] - sy[2]) * (px - sx[2]);
            let e2 = (sx[1] - sx[0]) * (py - sy[0]) - (sy[1] - sy[0]) * (px - sx[0]);
            let l0 = e0 / area;
            let l1 = e1 / area;
            let l2 = e2 / area;
            if l0 < 0.0 || l1 < 0.0 || l2 < 0.0 {
                continue;
            }
            let z = l0 * sz[0] + l1 * sz[1] + l2 * sz[2];
            if !(-1.0..=1.0).contains(&z) {
                continue;
            }
            let idx = y * buf.w + x;
            if z >= buf.depth[idx] {
                continue;
            }
            // Perspective-correct weights.
            let p0 = l0 * inv_w[0];
            let p1 = l1 * inv_w[1];
            let p2 = l2 * inv_w[2];
            let norm = 1.0 / (p0 + p1 + p2);
            let (b0, b1, b2) = (p0 * norm, p1 * norm, p2 * norm);
            let mix3 = |a: Vec3, b: Vec3, c: Vec3| {
                [
                    a[0] * b0 + b[0] * b1 + c[0] * b2,
                    a[1] * b0 + b[1] * b1 + c[1] * b2,
                    a[2] * b0 + b[2] * b1 + c[2] * b2,
                ]
            };
            let mut v = tri[0];
            v.world = mix3(tri[0].world, tri[1].world, tri[2].world);
            v.normal = mix3(tri[0].normal, tri[1].normal, tri[2].normal);
            for s in 0..2 {
                for k in 0..2 {
                    v.uv[s][k] = tri[0].uv[s][k] * b0 + tri[1].uv[s][k] * b1 + tri[2].uv[s][k] * b2;
                }
            }
            let color = shade(shared, material, &v, tangent, front);
            if let Some(cut) = alpha_cutoff {
                if color[3] < cut {
                    continue;
                }
            }
            if blend_pass {
                let a = math::clamp01(color[3]);
                let dst = buf.color[idx];
                let keep = 1.0 - a;
                let dst_a = math::clamp01(dst[3]);
                let blend_mode = match material.alpha_mode {
                    AlphaMode::GraphicBlend(mode) => Some(mode),
                    _ => None,
                };
                let mut output = [0.0; 4];
                for channel in 0..3 {
                    let dst_straight = if dst_a > 1e-12 {
                        dst[channel] / dst_a
                    } else {
                        0.0
                    };
                    let blended = blend_mode.map_or(color[channel], |mode| {
                        blend_channel(mode, color[channel], dst_straight)
                    });
                    output[channel] = dst[channel] * keep
                        + color[channel] * a * (1.0 - dst_a)
                        + blended * a * dst_a;
                }
                output[3] = a + dst_a * keep;
                buf.color[idx] = output;
            } else {
                buf.depth[idx] = z;
                buf.color[idx] = [color[0], color[1], color[2], 1.0];
            }
        }
    }
}

fn view_projection(camera: &Camera, aspect: f64) -> Mat4 {
    let view = math::look_at(camera.eye, camera.target, camera.up);
    let proj = match camera.projection {
        Projection::Perspective { fov_y_deg } => math::perspective(
            fov_y_deg * math::PI / 180.0,
            aspect,
            camera.near,
            camera.far,
        ),
        Projection::Orthographic { height } => {
            math::orthographic(height * 0.5 * aspect, height * 0.5, camera.near, camera.far)
        }
    };
    math::mul(&proj, &view)
}

fn project(vp: &Mat4, p: Vec3) -> [f64; 4] {
    [
        vp[0] * p[0] + vp[4] * p[1] + vp[8] * p[2] + vp[12],
        vp[1] * p[0] + vp[5] * p[1] + vp[9] * p[2] + vp[13],
        vp[2] * p[0] + vp[6] * p[1] + vp[10] * p[2] + vp[14],
        vp[3] * p[0] + vp[7] * p[1] + vp[11] * p[2] + vp[15],
    ]
}

fn normal_matrix_apply(m: &Mat4, n: Vec3) -> Vec3 {
    // Adequate for uniform scale and rotation; non-uniform scale keeps the
    // direction transform, which is what the importers' fixtures use.
    math::normalize(math::transform_dir(m, n))
}

fn check(scene: &Scene, options: &Options) -> Result<()> {
    if options.width == 0
        || options.height == 0
        || options.width > MAX_DIMENSION
        || options.height > MAX_DIMENSION
    {
        bail!("[limit-exceeded] render size must be 1..={MAX_DIMENSION} pixels per side");
    }
    if options.supersample == 0 || options.supersample > MAX_SUPERSAMPLE {
        bail!("[limit-exceeded] supersample must be 1..={MAX_SUPERSAMPLE}");
    }
    let output_pixels = u64::from(options.width) * u64::from(options.height);
    let pixels = output_pixels * u64::from(options.supersample) * u64::from(options.supersample);
    if pixels > 64 * 1024 * 1024 {
        bail!("[limit-exceeded] render needs {pixels} samples; the limit is 67108864");
    }
    let framebuffer_bytes = pixels
        .checked_mul((std::mem::size_of::<f64>() + std::mem::size_of::<[f64; 4]>()) as u64)
        .context("[limit-exceeded] render framebuffer byte count overflow")?;
    let output_bytes = output_pixels
        .checked_mul(4)
        .context("[limit-exceeded] render output byte count overflow")?;
    let reserved_bytes = framebuffer_bytes
        .checked_add(output_bytes)
        .context("[limit-exceeded] render allocation byte count overflow")?;
    ensure!(
        reserved_bytes <= MAX_RENDER_BUFFER_BYTES,
        "[limit-exceeded] render buffers need {reserved_bytes} bytes; the limit is {MAX_RENDER_BUFFER_BYTES}; lower output size or supersample"
    );
    if scene.lights.len() > MAX_LIGHTS {
        bail!("[limit-exceeded] at most {MAX_LIGHTS} lights are supported");
    }
    let c = &scene.camera;
    if !(c.near > 0.0 && c.far > c.near) {
        bail!("[invalid-argument] camera needs 0 < near < far");
    }
    if math::length(math::sub(c.eye, c.target)) < 1e-9 {
        bail!("[invalid-argument] camera eye and target coincide");
    }
    Ok(())
}

/// Thin-lens depth of field: a fixed pattern of `samples` points on the
/// aperture disc, all aimed at the same point on the focus plane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lens {
    /// Aperture radius in world units.
    pub aperture_radius: f64,
    /// Distance from the eye to the plane that stays sharp.
    pub focus_distance: f64,
    pub samples: u32,
}

/// Largest total supersampled sample count across all lens samples.
pub const MAX_LENS_WORK: u64 = 256 * 1024 * 1024;
pub const MAX_LENS_SAMPLES: u32 = 64;

/// Render with optional depth of field. Without a lens this is `render`.
pub fn render_with_lens(
    model: &Model,
    scene: &Scene,
    options: &Options,
    lens: Option<&Lens>,
) -> Result<Image> {
    let Some(lens) = lens.filter(|l| l.samples > 1 && l.aperture_radius > 0.0) else {
        return render(model, scene, options);
    };
    check(scene, options)?;
    ensure!(
        lens.samples <= MAX_LENS_SAMPLES,
        "[limit-exceeded] depth of field supports at most {MAX_LENS_SAMPLES} lens samples"
    );
    let work = options.width as u64
        * options.height as u64
        * (options.supersample as u64).pow(2)
        * lens.samples as u64;
    ensure!(
        work <= MAX_LENS_WORK,
        "[limit-exceeded] depth of field would take {work} samples; the limit is {MAX_LENS_WORK}; lower samples, supersample or size"
    );
    let output_pixels = u64::from(options.width) * u64::from(options.height);
    let accumulator_bytes = output_pixels
        .checked_mul(std::mem::size_of::<[f64; 4]>() as u64)
        .context("[limit-exceeded] depth-of-field accumulator byte count overflow")?;
    let render_bytes = output_pixels
        .checked_mul(u64::from(options.supersample).pow(2))
        .and_then(|samples| {
            samples
                .checked_mul((std::mem::size_of::<f64>() + std::mem::size_of::<[f64; 4]>()) as u64)
        })
        .and_then(|bytes| bytes.checked_add(output_pixels * 4))
        .context("[limit-exceeded] depth-of-field render byte count overflow")?;
    let peak_bytes = accumulator_bytes
        .checked_add(render_bytes)
        .context("[limit-exceeded] depth-of-field allocation byte count overflow")?;
    ensure!(
        peak_bytes <= MAX_RENDER_BUFFER_BYTES,
        "[limit-exceeded] depth of field needs up to {peak_bytes} render-buffer bytes; the limit is {MAX_RENDER_BUFFER_BYTES}; lower samples or output size"
    );
    let cam = &scene.camera;
    let forward = math::normalize(math::sub(cam.target, cam.eye));
    let right = math::normalize(math::cross(forward, cam.up));
    let up = math::cross(right, forward);
    let focus = math::add(cam.eye, math::scale(forward, lens.focus_distance));
    let golden = 2.399_963_229_728_653;
    let n = (options.width as usize) * (options.height as usize);
    let mut acc = vec![[0.0f64; 4]; n];
    for k in 0..lens.samples {
        let r = lens.aperture_radius * ((k as f64 + 0.5) / lens.samples as f64).sqrt();
        let a = k as f64 * golden;
        let offset = math::add(
            math::scale(right, r * math::cos(a)),
            math::scale(up, r * math::sin(a)),
        );
        let mut s = scene.clone();
        s.camera.eye = math::add(cam.eye, offset);
        s.camera.target = focus;
        let image = render(model, &s, options)?;
        for (px, out) in image.rgba.chunks_exact(4).zip(acc.iter_mut()) {
            let alpha = px[3] as f64 / 255.0;
            out[0] += px[0] as f64 * alpha;
            out[1] += px[1] as f64 * alpha;
            out[2] += px[2] as f64 * alpha;
            out[3] += alpha;
        }
    }
    let samples = lens.samples as f64;
    let mut rgba = Vec::with_capacity(n * 4);
    for p in &acc {
        let alpha = p[3] / samples;
        if p[3] > 0.0 {
            for c in &p[..3] {
                rgba.push((c / p[3]).round().clamp(0.0, 255.0) as u8);
            }
        } else {
            rgba.extend_from_slice(&[0, 0, 0]);
        }
        rgba.push((alpha * 255.0).round().clamp(0.0, 255.0) as u8);
    }
    Ok(Image {
        width: options.width,
        height: options.height,
        rgba,
    })
}

pub fn render(model: &Model, scene: &Scene, options: &Options) -> Result<Image> {
    check(scene, options)?;
    let ss = options.supersample as usize;
    let (w, h) = (options.width as usize * ss, options.height as usize * ss);
    let mut buf = Buffers {
        w,
        h,
        depth: vec![f64::INFINITY; w * h],
        color: vec![[0.0; 4]; w * h],
    };
    let mut shadows: Vec<Option<super::shadow::ShadowSet>> = Vec::new();
    if scene.shadow_size > 0 {
        let size = scene.shadow_size.clamp(
            super::shadow::MIN_SHADOW_SIZE,
            super::shadow::MAX_SHADOW_SIZE,
        );
        let maps: u64 = scene.lights.iter().map(super::shadow::maps_for).sum();
        let texels = maps * size as u64 * size as u64;
        ensure!(
            texels <= super::shadow::MAX_SHADOW_TEXELS,
            "[limit-exceeded] shadow maps would need {texels} texels; the limit is {}; lower shadow_map_size or the number of lights",
            super::shadow::MAX_SHADOW_TEXELS
        );
        let sphere = super::shadow::scene_sphere(model);
        for light in &scene.lights {
            shadows.push(sphere.and_then(|s| super::shadow::build(model, light, size, s)));
        }
    }
    let shared = Shared {
        model,
        scene,
        decode: decode_table(),
        eye: scene.camera.eye,
        exposure: math::pow(2.0, scene.exposure_ev),
        shadows,
        bounds: model.bounds().unwrap_or(([0.0; 3], [1.0; 3])),
    };
    let vp = view_projection(&scene.camera, options.width as f64 / options.height as f64);
    let default_material = Material::default();

    // Opaque and masked first, then blended back-to-front (stable order).
    let mut blended: Vec<(f64, usize)> = Vec::new();
    for pass_blend in [false, true] {
        let order: Vec<usize> = if pass_blend {
            blended.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
            blended.iter().map(|b| b.1).collect()
        } else {
            (0..model.instances.len()).collect()
        };
        for ii in order {
            let inst: &Instance = &model.instances[ii];
            let prim = &model.primitives[inst.primitive];
            let material = prim
                .material
                .and_then(|m| model.materials.get(m))
                .unwrap_or(&default_material);
            let is_blend = matches!(
                material.alpha_mode,
                AlphaMode::Blend | AlphaMode::GraphicBlend(_)
            );
            if !pass_blend && is_blend {
                // Depth of the instance origin decides the blend order.
                let o = math::transform_point(&inst.transform, [0.0; 3]);
                let d = math::length(math::sub(o, scene.camera.eye));
                blended.push((d, ii));
                continue;
            }
            let world: Vec<Vec3> = prim
                .positions
                .iter()
                .map(|p| {
                    math::transform_point(&inst.transform, [p[0] as f64, p[1] as f64, p[2] as f64])
                })
                .collect();
            let make = |i: u32| {
                let i = i as usize;
                let n = prim.normals[i];
                let uv = |s: usize| {
                    prim.uvs
                        .get(s)
                        .map(|u| [u[i][0] as f64, u[i][1] as f64])
                        .unwrap_or([0.0, 0.0])
                };
                Vertex {
                    clip: project(&vp, world[i]),
                    world: world[i],
                    normal: normal_matrix_apply(
                        &inst.transform,
                        [n[0] as f64, n[1] as f64, n[2] as f64],
                    ),
                    uv: [uv(0), uv(1)],
                }
            };
            for tri in prim.indices.chunks_exact(3) {
                draw_triangle(
                    &shared,
                    &mut buf,
                    material,
                    [make(tri[0]), make(tri[1]), make(tri[2])],
                    pass_blend,
                );
            }
        }
    }

    // Resolve: composite over the background in linear light, box-downsample.
    let bg_a = math::clamp01(scene.background[3]);
    let bg = [
        srgb_to_linear(scene.background[0]) * bg_a,
        srgb_to_linear(scene.background[1]) * bg_a,
        srgb_to_linear(scene.background[2]) * bg_a,
    ];
    let mut rgba = Vec::with_capacity(options.width as usize * options.height as usize * 4);
    let inv = 1.0 / (ss * ss) as f64;
    for y in 0..options.height as usize {
        for x in 0..options.width as usize {
            let mut acc = [0.0f64; 4];
            for sy in 0..ss {
                for sx in 0..ss {
                    let c = buf.color[(y * ss + sy) * w + x * ss + sx];
                    let keep = 1.0 - c[3];
                    acc[0] += c[0] + bg[0] * keep;
                    acc[1] += c[1] + bg[1] * keep;
                    acc[2] += c[2] + bg[2] * keep;
                    acc[3] += c[3] + bg_a * keep;
                }
            }
            let a = acc[3] * inv;
            for channel in acc.iter().take(3) {
                let straight = if a > 0.0 { channel * inv / a } else { 0.0 };
                rgba.push((linear_to_srgb(straight) * 255.0 + 0.5) as u8);
            }
            rgba.push((math::clamp01(a) * 255.0 + 0.5) as u8);
        }
    }
    Ok(Image {
        width: options.width,
        height: options.height,
        rgba,
    })
}

/// A perspective camera that frames the model's bounds from the given
/// azimuth/elevation (degrees; azimuth 0 looks down -Z toward the model).
pub fn frame_camera(model: &Model, azimuth_deg: f64, elevation_deg: f64, fov_y_deg: f64) -> Camera {
    let (min, max) = model.bounds().unwrap_or(([-1.0; 3], [1.0; 3]));
    let center = [
        (min[0] + max[0]) * 0.5,
        (min[1] + max[1]) * 0.5,
        (min[2] + max[2]) * 0.5,
    ];
    let radius = (math::length(math::sub(max, min)) * 0.5).max(1e-6);
    let half = fov_y_deg * 0.5 * math::PI / 180.0;
    let distance = radius / math::sin(half) * 1.05;
    let az = azimuth_deg * math::PI / 180.0;
    let el = elevation_deg * math::PI / 180.0;
    let dir = [
        math::sin(az) * math::cos(el),
        math::sin(el),
        math::cos(az) * math::cos(el),
    ];
    Camera {
        eye: math::add(center, math::scale(dir, distance)),
        target: center,
        up: [0.0, 1.0, 0.0],
        projection: Projection::Perspective { fov_y_deg },
        near: (distance - radius * 1.5).max(distance * 0.01),
        far: distance + radius * 2.0,
    }
}

/// Convenient three-point style default lighting.
pub fn default_lights() -> Vec<Light> {
    vec![
        Light::Directional {
            direction: math::normalize([-0.4, -0.7, -0.6]),
            color: [1.0, 0.98, 0.95],
            intensity: 3.0,
        },
        Light::Directional {
            direction: math::normalize([0.7, -0.2, 0.4]),
            color: [0.7, 0.8, 1.0],
            intensity: 1.0,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene3d::model::{
        Instance, Material, Model, Primitive, TexRef, Texture, TextureProjection, WrapMode,
    };

    #[test]
    fn texture_address_modes_repeat_clamp_and_mirror_out_of_range_uvs() {
        let texture = Texture {
            name: "wrap-test".into(),
            width: 2,
            height: 1,
            rgba: vec![255, 0, 0, 255, 0, 0, 255, 255],
        };
        let decode = [0.0; 256];
        let uv = [1.1, 0.5];
        let repeat = sample(&texture, uv, &decode, false, &WrapMode::Repeat);
        let clamp = sample(&texture, uv, &decode, false, &WrapMode::Clamp);
        let mirror = sample(&texture, uv, &decode, false, &WrapMode::Mirror);
        assert!(repeat[0] > 0.65 && repeat[0] < 0.75);
        assert_eq!(clamp, [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(mirror, [0.0, 0.0, 1.0, 1.0]);
        let mirrored_negative = sample(&texture, [-0.1, 0.5], &decode, false, &WrapMode::Mirror);
        assert_eq!(mirrored_negative, [1.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn graphic_blend_modes_use_deterministic_linear_channel_formulas() {
        for (mode, expected) in [
            (GraphicBlend::Multiply, 0.2),
            (GraphicBlend::Screen, 0.85),
            (GraphicBlend::Overlay, 0.7),
            (GraphicBlend::Darken, 0.25),
            (GraphicBlend::Lighten, 0.8),
        ] {
            assert!((blend_channel(mode, 0.25, 0.8) - expected).abs() < 1e-12);
        }
    }

    fn quad_model(z: f32, color: [f32; 4], blend: bool) -> Model {
        let mut m = Model::default();
        m.materials.push(Material {
            base_color: color,
            metallic: 0.0,
            roughness: 0.8,
            alpha_mode: if blend {
                AlphaMode::Blend
            } else {
                AlphaMode::Opaque
            },
            ..Default::default()
        });
        m.primitives.push(Primitive {
            name: "quad".into(),
            positions: vec![
                [-1.0, -1.0, z],
                [1.0, -1.0, z],
                [1.0, 1.0, z],
                [-1.0, 1.0, z],
            ],
            normals: vec![[0.0, 0.0, 1.0]; 4],
            uvs: vec![],
            indices: vec![0, 1, 2, 0, 2, 3],
            material: Some(0),
        });
        m.instances.push(Instance {
            id: "quad".into(),
            primitive: 0,
            transform: math::IDENTITY,
        });
        m.finish().unwrap()
    }

    fn scene(model: &Model) -> Scene {
        Scene {
            camera: frame_camera(model, 0.0, 0.0, 40.0),
            lights: default_lights(),
            environment: Environment::default(),
            background: [0.0, 0.0, 0.0, 0.0],
            exposure_ev: 0.0,
            shadow_size: 0,
        }
    }

    #[test]
    fn quad_covers_center_and_leaves_corners_transparent() {
        let model = quad_model(0.0, [1.0, 0.0, 0.0, 1.0], false);
        let opts = Options {
            width: 32,
            height: 32,
            supersample: 1,
        };
        let img = render(&model, &scene(&model), &opts).unwrap();
        let px = |x: usize, y: usize| &img.rgba[(y * 32 + x) * 4..(y * 32 + x) * 4 + 4];
        assert_eq!(px(16, 16)[3], 255);
        assert!(px(16, 16)[0] > px(16, 16)[1]);
        assert_eq!(px(0, 0)[3], 0);
    }

    #[test]
    fn textured_alpha_overlay_in_front_of_an_opaque_face_changes_pixels() {
        let mut model = quad_model(0.0, [0.35, 0.35, 0.35, 1.0], false);
        model.textures.push(Texture {
            name: "solid-red-overlay".into(),
            width: 1,
            height: 1,
            rgba: vec![255, 0, 0, 255],
        });
        model.materials.push(Material {
            name: "red-overlay".into(),
            base_color: [1.0; 4],
            base_color_texture: Some(TexRef {
                texture: 0,
                uv_set: 0,
                wrap: WrapMode::Clamp,
                projection: TextureProjection::Uv,
            }),
            alpha_mode: AlphaMode::Blend,
            ..Material::default()
        });
        let mut overlay = model.primitives[0].clone();
        overlay.name = "red-overlay".into();
        overlay
            .positions
            .iter_mut()
            .for_each(|position| position[2] = 0.01);
        overlay.uvs = vec![vec![[0.5, 0.5]; overlay.positions.len()]];
        overlay.material = Some(1);
        let primitive = model.primitives.len();
        model.primitives.push(overlay);
        model.instances.push(Instance {
            id: "red-overlay".into(),
            primitive,
            transform: math::IDENTITY,
        });
        let opts = Options {
            width: 32,
            height: 32,
            supersample: 1,
        };
        let image = render(&model, &scene(&model), &opts).unwrap();
        let center = &image.rgba[(16 * 32 + 16) * 4..(16 * 32 + 16) * 4 + 4];
        assert!(
            center[0] > center[1].saturating_add(80),
            "overlay pixel: {center:?}"
        );
    }

    #[test]
    fn rendering_is_deterministic_and_antialiasing_adds_partial_alpha() {
        let model = quad_model(0.0, [0.2, 0.6, 1.0, 1.0], false);
        let opts = Options {
            width: 48,
            height: 48,
            supersample: 3,
        };
        let a = render(&model, &scene(&model), &opts).unwrap();
        let b = render(&model, &scene(&model), &opts).unwrap();
        assert_eq!(a.rgba, b.rgba);
        assert!(a.rgba.chunks_exact(4).any(|p| p[3] > 0 && p[3] < 255));
    }

    #[test]
    fn nearer_surface_wins_and_backface_is_culled() {
        let mut model = quad_model(0.0, [1.0, 0.0, 0.0, 1.0], false);
        // Second, nearer green quad in the same model.
        let near = quad_model(0.5, [0.0, 1.0, 0.0, 1.0], false);
        model.materials.push(near.materials[0].clone());
        let mut p = near.primitives[0].clone();
        p.material = Some(1);
        model.primitives.push(p);
        model.instances.push(Instance {
            id: "near".into(),
            primitive: 1,
            transform: math::IDENTITY,
        });
        let opts = Options {
            width: 16,
            height: 16,
            supersample: 1,
        };
        let img = render(&model, &scene(&model), &opts).unwrap();
        let c = &img.rgba[(8 * 16 + 8) * 4..(8 * 16 + 8) * 4 + 4];
        assert!(c[1] > c[0], "green quad must be in front: {c:?}");
        // Looking from behind culls the single-sided quad entirely.
        let mut back = scene(&model);
        back.camera = frame_camera(&model, 180.0, 0.0, 40.0);
        let img = render(&model, &back, &opts).unwrap();
        assert!(img.rgba.chunks_exact(4).all(|p| p[3] == 0));
    }

    #[test]
    fn limits_are_enforced_before_allocation() {
        let model = quad_model(0.0, [1.0; 4], false);
        let s = scene(&model);
        for (w, h, ss) in [(0, 10, 1), (9000, 10, 1), (10, 10, 0), (8192, 8192, 4)] {
            let o = Options {
                width: w,
                height: h,
                supersample: ss,
            };
            assert!(render(&model, &s, &o).is_err(), "{w}x{h}@{ss}");
        }
        let large_supersampled = Options {
            width: 2048,
            height: 2048,
            supersample: 2,
        };
        let error = render(&model, &s, &large_supersampled)
            .err()
            .unwrap()
            .to_string();
        assert!(
            error.contains("render buffers need"),
            "byte budget must reject before allocating the framebuffer: {error}"
        );
        let large_lens = Options {
            width: 2048,
            height: 2048,
            supersample: 1,
        };
        let lens = Lens {
            aperture_radius: 0.1,
            focus_distance: 4.0,
            samples: 2,
        };
        let error = render_with_lens(&model, &s, &large_lens, Some(&lens))
            .err()
            .unwrap()
            .to_string();
        assert!(
            error.contains("depth of field needs up to"),
            "combined lens and framebuffer bytes must be rejected before allocation: {error}"
        );
    }

    #[test]
    fn blended_surface_shows_what_is_behind_it() {
        let mut model = quad_model(0.0, [1.0, 0.0, 0.0, 1.0], false);
        let glass = quad_model(0.5, [0.0, 0.0, 1.0, 0.5], true);
        model.materials.push(glass.materials[0].clone());
        let mut p = glass.primitives[0].clone();
        p.material = Some(1);
        model.primitives.push(p);
        model.instances.push(Instance {
            id: "glass".into(),
            primitive: 1,
            transform: math::IDENTITY,
        });
        let opts = Options {
            width: 16,
            height: 16,
            supersample: 1,
        };
        let img = render(&model, &scene(&model), &opts).unwrap();
        let c = &img.rgba[(8 * 16 + 8) * 4..(8 * 16 + 8) * 4 + 4];
        assert!(c[0] > 20 && c[2] > 20, "both red and blue must show: {c:?}");
    }
}
