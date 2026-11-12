//! In-memory 3D model shared by every importer and the renderer.

use super::math::{self, Mat4, Vec3};
use anyhow::{bail, Result};

/// Hard import limits, checked before allocation.
pub struct Limits {
    pub max_bytes: usize,
    pub max_vertices: usize,
    pub max_triangles: usize,
    pub max_primitives: usize,
    pub max_materials: usize,
    pub max_textures: usize,
    pub max_nodes: usize,
    pub max_depth: usize,
    pub max_instances: usize,
    pub max_texture_bytes: usize,
}

pub const LIMITS: Limits = Limits {
    max_bytes: 256 * 1024 * 1024,
    max_vertices: 2_000_000,
    max_triangles: 1_000_000,
    max_primitives: 4096,
    max_materials: 1024,
    max_textures: 64,
    max_nodes: 20_000,
    max_depth: 32,
    max_instances: 20_000,
    max_texture_bytes: 256 * 1024 * 1024,
};

/// Resolves an external buffer or texture URI to bytes. The caller enforces
/// containment inside the project root and the byte limit.
pub type Resolver<'a> = &'a dyn Fn(&str) -> Result<Vec<u8>>;

#[derive(Clone, Debug, PartialEq)]
pub enum AlphaMode {
    Opaque,
    Mask(f32),
    Blend,
    GraphicBlend(GraphicBlend),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphicBlend {
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum WrapMode {
    #[default]
    Repeat,
    Clamp,
    Mirror,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TexRef {
    pub texture: usize,
    pub uv_set: usize,
    pub wrap: WrapMode,
    pub projection: TextureProjection,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextureProjection {
    #[default]
    Uv,
    Triplanar,
}

#[derive(Clone, Debug)]
pub struct Material {
    pub name: String,
    pub base_color: [f32; 4],
    pub base_color_texture: Option<TexRef>,
    pub metallic: f32,
    pub roughness: f32,
    pub metallic_roughness_texture: Option<TexRef>,
    pub normal_texture: Option<TexRef>,
    pub normal_scale: f32,
    pub occlusion_texture: Option<TexRef>,
    pub occlusion_strength: f32,
    pub emissive: [f32; 3],
    pub emissive_texture: Option<TexRef>,
    pub alpha_mode: AlphaMode,
    pub double_sided: bool,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            name: String::new(),
            base_color: [1.0, 1.0, 1.0, 1.0],
            base_color_texture: None,
            metallic: 1.0,
            roughness: 1.0,
            metallic_roughness_texture: None,
            normal_texture: None,
            normal_scale: 1.0,
            occlusion_texture: None,
            occlusion_strength: 1.0,
            emissive: [0.0; 3],
            emissive_texture: None,
            alpha_mode: AlphaMode::Opaque,
            double_sided: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Texture {
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// Row-major RGBA8, sRGB-encoded for color textures.
    pub rgba: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct Primitive {
    pub name: String,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// Up to two UV sets; each has one entry per vertex.
    pub uvs: Vec<Vec<[f32; 2]>>,
    pub indices: Vec<u32>,
    pub material: Option<usize>,
}

impl Primitive {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

#[derive(Clone, Debug)]
pub struct Instance {
    /// Stable, human-meaningful ID (node name, or `node-<n>` / `object-<n>`).
    pub id: String,
    pub primitive: usize,
    pub transform: Mat4,
}

#[derive(Clone, Debug, Default)]
pub struct ImportReport {
    pub format: String,
    pub vertices: usize,
    pub triangles: usize,
    pub unsupported: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Model {
    pub primitives: Vec<Primitive>,
    pub materials: Vec<Material>,
    pub textures: Vec<Texture>,
    pub instances: Vec<Instance>,
    pub report: ImportReport,
}

impl Model {
    /// World-space bounds over every instance, or `None` for an empty model.
    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        let mut min = [f64::INFINITY; 3];
        let mut max = [f64::NEG_INFINITY; 3];
        let mut any = false;
        for instance in &self.instances {
            for p in &self.primitives[instance.primitive].positions {
                let w = math::transform_point(
                    &instance.transform,
                    [p[0] as f64, p[1] as f64, p[2] as f64],
                );
                for axis in 0..3 {
                    min[axis] = min[axis].min(w[axis]);
                    max[axis] = max[axis].max(w[axis]);
                }
                any = true;
            }
        }
        any.then_some((min, max))
    }

    pub fn rendered_triangles(&self) -> usize {
        self.instances
            .iter()
            .map(|i| self.primitives[i.primitive].triangle_count())
            .sum()
    }

    /// Final structural checks shared by every importer.
    pub fn finish(mut self) -> Result<Model> {
        if self.instances.is_empty() {
            bail!("[malformed-resource] model contains no renderable geometry");
        }
        if self.rendered_triangles() > LIMITS.max_triangles {
            bail!(
                "[limit-exceeded] model renders {} triangles; the limit is {}",
                self.rendered_triangles(),
                LIMITS.max_triangles
            );
        }
        for primitive in &mut self.primitives {
            if primitive.normals.len() != primitive.positions.len() {
                primitive.normals = smooth_normals(&primitive.positions, &primitive.indices);
            }
        }
        self.report.vertices = self.primitives.iter().map(|p| p.positions.len()).sum();
        self.report.triangles = self.primitives.iter().map(|p| p.triangle_count()).sum();
        Ok(self)
    }
}

/// Area-weighted smooth vertex normals, accumulated in index order.
pub fn smooth_normals(positions: &[[f32; 3]], indices: &[u32]) -> Vec<[f32; 3]> {
    let mut acc = vec![[0.0f64; 3]; positions.len()];
    for tri in indices.chunks_exact(3) {
        let p: Vec<Vec3> = tri
            .iter()
            .map(|&i| {
                let v = positions[i as usize];
                [v[0] as f64, v[1] as f64, v[2] as f64]
            })
            .collect();
        let n = math::cross(math::sub(p[1], p[0]), math::sub(p[2], p[0]));
        for &i in tri {
            let a = &mut acc[i as usize];
            for axis in 0..3 {
                a[axis] += n[axis];
            }
        }
    }
    acc.into_iter()
        .map(|n| {
            let n = math::normalize(n);
            [n[0] as f32, n[1] as f32, n[2] as f32]
        })
        .collect()
}
