//! Deterministic shadow maps for the CPU renderer.
//!
//! Each light gets one or more depth maps rasterized in a fixed instance and
//! triangle order. Directional lights use an orthographic map fitted to the
//! scene bounds, spot lights a perspective map over the cone, and point lights
//! a six-face cube. Lookups use a fixed 3x3 percentage-closer filter.

use super::math::{self, Mat4, Vec3};
use super::model::{AlphaMode, Model};
use super::render::Light;

/// Total depth texels across every map of a render (f32 each).
pub const MAX_SHADOW_TEXELS: u64 = 16 * 1024 * 1024;
pub const MIN_SHADOW_SIZE: u32 = 64;
pub const MAX_SHADOW_SIZE: u32 = 2048;
pub const DEFAULT_SHADOW_SIZE: u32 = 1024;

#[derive(Clone, Copy)]
enum Kind {
    /// Half extent of the orthographic window in world units.
    Ortho(f64),
    /// Tangent of the half field of view.
    Persp(f64),
}

struct Map {
    view: Mat4,
    kind: Kind,
    size: usize,
    depth: Vec<f32>,
}

pub struct ShadowSet {
    maps: Vec<Map>,
    /// For point lights: the light position, used to pick a cube face.
    point: Option<Vec3>,
}

/// Number of depth maps a light needs.
pub fn maps_for(light: &Light) -> u64 {
    match light {
        Light::Point { .. } => 6,
        _ => 1,
    }
}

fn project(map: &Map, q: Vec3) -> Option<(f64, f64, f64)> {
    let z = -q[2];
    let half = map.size as f64 * 0.5;
    match map.kind {
        Kind::Ortho(h) => Some((q[0] / h * half + half, half - q[1] / h * half, z)),
        Kind::Persp(t) => {
            if z <= 1e-9 {
                return None;
            }
            let k = z * t;
            Some((q[0] / k * half + half, half - q[1] / k * half, z))
        }
    }
}

impl Map {
    fn new(view: Mat4, kind: Kind, size: usize) -> Map {
        Map {
            view,
            kind,
            size,
            depth: vec![f32::INFINITY; size * size],
        }
    }

    /// Rasterize one world-space triangle into the depth map.
    fn draw(&mut self, tri: [Vec3; 3]) {
        let mut poly: Vec<Vec3> = tri
            .iter()
            .map(|p| math::transform_point(&self.view, *p))
            .collect();
        if let Kind::Persp(_) = self.kind {
            // Clip against the plane just in front of the light.
            let eps = 1e-3;
            let mut out = Vec::with_capacity(4);
            for i in 0..poly.len() {
                let a = poly[i];
                let b = poly[(i + 1) % poly.len()];
                let (da, db) = (-a[2] - eps, -b[2] - eps);
                if da >= 0.0 {
                    out.push(a);
                }
                if (da >= 0.0) != (db >= 0.0) {
                    let t = da / (da - db);
                    out.push(math::add(a, math::scale(math::sub(b, a), t)));
                }
            }
            poly = out;
        }
        if poly.len() < 3 {
            return;
        }
        for k in 1..poly.len() - 1 {
            self.fill([poly[0], poly[k], poly[k + 1]]);
        }
    }

    fn fill(&mut self, tri: [Vec3; 3]) {
        let persp = matches!(self.kind, Kind::Persp(_));
        let mut s = [(0.0, 0.0, 0.0); 3];
        for (slot, q) in s.iter_mut().zip(tri.iter()) {
            match project(self, *q) {
                Some(v) => *slot = v,
                None => return,
            }
        }
        let area = (s[1].0 - s[0].0) * (s[2].1 - s[0].1) - (s[2].0 - s[0].0) * (s[1].1 - s[0].1);
        if area.abs() < 1e-12 {
            return;
        }
        let size = self.size as f64;
        let min_x = s.iter().map(|p| p.0).fold(f64::INFINITY, f64::min).max(0.0);
        let max_x = s
            .iter()
            .map(|p| p.0)
            .fold(f64::NEG_INFINITY, f64::max)
            .min(size - 1.0);
        let min_y = s.iter().map(|p| p.1).fold(f64::INFINITY, f64::min).max(0.0);
        let max_y = s
            .iter()
            .map(|p| p.1)
            .fold(f64::NEG_INFINITY, f64::max)
            .min(size - 1.0);
        if min_x > max_x || min_y > max_y {
            return;
        }
        let inv = 1.0 / area;
        for y in min_y.floor() as usize..=max_y.floor() as usize {
            for x in min_x.floor() as usize..=max_x.floor() as usize {
                let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                let w0 = ((s[1].0 - px) * (s[2].1 - py) - (s[2].0 - px) * (s[1].1 - py)) * inv;
                let w1 = ((s[2].0 - px) * (s[0].1 - py) - (s[0].0 - px) * (s[2].1 - py)) * inv;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let z = if persp {
                    1.0 / (w0 / s[0].2 + w1 / s[1].2 + w2 / s[2].2)
                } else {
                    w0 * s[0].2 + w1 * s[1].2 + w2 * s[2].2
                };
                let cell = &mut self.depth[y * self.size + x];
                if (z as f32) < *cell {
                    *cell = z as f32;
                }
            }
        }
    }

    /// World size of one texel at view depth `z`.
    fn texel(&self, z: f64) -> f64 {
        match self.kind {
            Kind::Ortho(h) => 2.0 * h / self.size as f64,
            Kind::Persp(t) => 2.0 * z.max(1e-6) * t / self.size as f64,
        }
    }

    /// Fraction of the 3x3 neighborhood that is lit.
    fn lit(&self, p: Vec3, n: Vec3) -> f64 {
        let q0 = math::transform_point(&self.view, p);
        let Some((_, _, z0)) = project(self, q0) else {
            return 1.0;
        };
        // Offset along the normal by a couple of texels to avoid acne.
        let texel = self.texel(z0);
        let pushed = math::add(p, math::scale(n, texel * 1.5));
        let q = math::transform_point(&self.view, pushed);
        let Some((sx, sy, z)) = project(self, q) else {
            return 1.0;
        };
        let bias = texel * 1.5;
        let (cx, cy) = (sx.floor() as i64, sy.floor() as i64);
        let size = self.size as i64;
        let mut lit = 0.0;
        for dy in -1..=1 {
            for dx in -1..=1 {
                let (x, y) = (cx + dx, cy + dy);
                if x < 0 || y < 0 || x >= size || y >= size {
                    lit += 1.0;
                    continue;
                }
                let stored = self.depth[(y * size + x) as usize] as f64;
                if z - bias <= stored {
                    lit += 1.0;
                }
            }
        }
        lit / 9.0
    }
}

fn up_for(dir: Vec3) -> Vec3 {
    if dir[1].abs() > 0.99 {
        [0.0, 0.0, 1.0]
    } else {
        [0.0, 1.0, 0.0]
    }
}

/// World-space center and radius enclosing every rendered vertex.
pub fn scene_sphere(model: &Model) -> Option<(Vec3, f64)> {
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    for inst in &model.instances {
        for p in &model.primitives[inst.primitive].positions {
            let w = math::transform_point(&inst.transform, [p[0] as f64, p[1] as f64, p[2] as f64]);
            for k in 0..3 {
                min[k] = min[k].min(w[k]);
                max[k] = max[k].max(w[k]);
            }
        }
    }
    if min[0] > max[0] {
        return None;
    }
    let center = [
        (min[0] + max[0]) * 0.5,
        (min[1] + max[1]) * 0.5,
        (min[2] + max[2]) * 0.5,
    ];
    let radius = (math::length(math::sub(max, min)) * 0.5).max(1e-6);
    Some((center, radius))
}

fn casters(model: &Model, map: &mut Map) {
    for inst in &model.instances {
        let prim = &model.primitives[inst.primitive];
        let blend = prim
            .material
            .and_then(|m| model.materials.get(m))
            .is_some_and(|m| matches!(m.alpha_mode, AlphaMode::Blend | AlphaMode::GraphicBlend(_)));
        if blend {
            continue;
        }
        let world: Vec<Vec3> = prim
            .positions
            .iter()
            .map(|p| {
                math::transform_point(&inst.transform, [p[0] as f64, p[1] as f64, p[2] as f64])
            })
            .collect();
        for tri in prim.indices.chunks_exact(3) {
            map.draw([
                world[tri[0] as usize],
                world[tri[1] as usize],
                world[tri[2] as usize],
            ]);
        }
    }
}

/// Build the shadow maps for a light, or `None` for degenerate input.
pub fn build(model: &Model, light: &Light, size: u32, sphere: (Vec3, f64)) -> Option<ShadowSet> {
    let size = size.clamp(MIN_SHADOW_SIZE, MAX_SHADOW_SIZE) as usize;
    let (center, radius) = sphere;
    match light {
        Light::Directional { direction, .. } => {
            let dir = math::normalize(*direction);
            let eye = math::sub(center, math::scale(dir, radius * 2.0));
            let view = math::look_at(eye, center, up_for(dir));
            let mut map = Map::new(view, Kind::Ortho(radius * 1.05), size);
            casters(model, &mut map);
            Some(ShadowSet {
                maps: vec![map],
                point: None,
            })
        }
        Light::Spot {
            position,
            direction,
            outer_cos,
            ..
        } => {
            let dir = math::normalize(*direction);
            let outer = math::acos(*outer_cos).clamp(0.05, 1.4);
            let t = math::tan(outer) * 1.05;
            let view = math::look_at(*position, math::add(*position, dir), up_for(dir));
            let mut map = Map::new(view, Kind::Persp(t), size);
            casters(model, &mut map);
            Some(ShadowSet {
                maps: vec![map],
                point: None,
            })
        }
        Light::Point { position, .. } => {
            let faces: [(Vec3, Vec3); 6] = [
                ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
                ([-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
                ([0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
                ([0.0, -1.0, 0.0], [0.0, 0.0, 1.0]),
                ([0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
                ([0.0, 0.0, -1.0], [0.0, 1.0, 0.0]),
            ];
            let mut maps = Vec::new();
            for (dir, up) in faces {
                let view = math::look_at(*position, math::add(*position, dir), up);
                let mut map = Map::new(view, Kind::Persp(1.05), size);
                casters(model, &mut map);
                maps.push(map);
            }
            Some(ShadowSet {
                maps,
                point: Some(*position),
            })
        }
    }
}

impl ShadowSet {
    /// 1.0 fully lit .. 0.0 fully shadowed, for a surface point and normal.
    pub fn visibility(&self, p: Vec3, n: Vec3) -> f64 {
        match self.point {
            None => self.maps[0].lit(p, n),
            Some(pos) => {
                let d = math::sub(p, pos);
                let (ax, ay, az) = (d[0].abs(), d[1].abs(), d[2].abs());
                let face = if ax >= ay && ax >= az {
                    if d[0] >= 0.0 {
                        0
                    } else {
                        1
                    }
                } else if ay >= az {
                    if d[1] >= 0.0 {
                        2
                    } else {
                        3
                    }
                } else if d[2] >= 0.0 {
                    4
                } else {
                    5
                };
                self.maps[face].lit(p, n)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene3d::model::{Instance, Primitive};

    fn quad(y: f32, half: f32) -> Primitive {
        Primitive {
            name: "q".into(),
            positions: vec![
                [-half, y, -half],
                [half, y, -half],
                [half, y, half],
                [-half, y, half],
            ],
            normals: vec![[0.0, 1.0, 0.0]; 4],
            uvs: vec![],
            indices: vec![0, 2, 1, 0, 3, 2],
            material: None,
        }
    }

    fn scene() -> Model {
        let mut m = Model::default();
        m.primitives.push(quad(0.0, 3.0));
        m.primitives.push(quad(1.0, 0.5));
        for (i, id) in ["ground", "blocker"].iter().enumerate() {
            m.instances.push(Instance {
                id: (*id).into(),
                primitive: i,
                transform: math::IDENTITY,
            });
        }
        m.finish().unwrap()
    }

    #[test]
    fn directional_shadow_falls_below_the_blocker_only() {
        let model = scene();
        let sphere = scene_sphere(&model).unwrap();
        let light = Light::Directional {
            direction: [0.0, -1.0, 0.0],
            color: [1.0; 3],
            intensity: 1.0,
        };
        let set = build(&model, &light, 512, sphere).unwrap();
        let up = [0.0, 1.0, 0.0];
        assert!(set.visibility([0.0, 0.0, 0.0], up) < 0.01, "under blocker");
        assert!(set.visibility([2.0, 0.0, 2.0], up) > 0.99, "open ground");
        // Deterministic: rebuilding gives identical answers.
        let again = build(&model, &light, 512, sphere).unwrap();
        for p in [[0.2, 0.0, 0.1], [0.55, 0.0, 0.0], [-1.5, 0.0, 0.7]] {
            assert_eq!(set.visibility(p, up), again.visibility(p, up));
        }
    }

    #[test]
    fn spot_and_point_lights_shadow_too() {
        let model = scene();
        let sphere = scene_sphere(&model).unwrap();
        let up = [0.0, 1.0, 0.0];
        let spot = Light::Spot {
            position: [0.0, 4.0, 0.0],
            direction: [0.0, -1.0, 0.0],
            color: [1.0; 3],
            intensity: 1.0,
            inner_cos: math::cos(0.5),
            outer_cos: math::cos(0.8),
        };
        let set = build(&model, &spot, 512, sphere).unwrap();
        assert!(set.visibility([0.0, 0.0, 0.0], up) < 0.01);
        assert!(set.visibility([1.5, 0.0, 0.0], up) > 0.99);
        let point = Light::Point {
            position: [0.0, 4.0, 0.0],
            color: [1.0; 3],
            intensity: 1.0,
        };
        let set = build(&model, &point, 256, sphere).unwrap();
        assert!(set.visibility([0.0, 0.0, 0.0], up) < 0.01);
        assert!(set.visibility([2.0, 0.0, 0.0], up) > 0.99);
    }
}
