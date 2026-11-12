//! Optional `wgpu` frame resolve with explicit fallback.

use super::render::Image;
#[cfg(feature = "gpu")]
use anyhow::Context;
use anyhow::{bail, Result};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Availability {
    pub available: bool,
    pub adapter: Option<String>,
    pub backend: Option<String>,
    pub reason: Option<String>,
}

#[cfg(feature = "gpu")]
fn adapter() -> Result<(wgpu::Adapter, Availability)> {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .context("no compatible GPU adapter was found")?;
    let info = adapter.get_info();
    Ok((
        adapter,
        Availability {
            available: true,
            adapter: Some(info.name),
            backend: Some(format!("{:?}", info.backend).to_ascii_lowercase()),
            reason: None,
        },
    ))
}

#[cfg(feature = "gpu")]
pub fn availability() -> Availability {
    adapter().map(|(_, s)| s).unwrap_or_else(|e| Availability {
        available: false,
        adapter: None,
        backend: None,
        reason: Some(e.to_string()),
    })
}

/// Dispatch the completed reference frame through a real GPU compute pipeline.
/// Geometry and shading remain deterministic on the CPU in the v1 backend; the
/// diagnostic calls this `reference-raster+wgpu-resolve` rather than implying a
/// GPU rasterizer. The returned pixels must be byte-identical.
#[cfg(feature = "gpu")]
pub fn resolve(reference: Image) -> Result<(Image, Availability)> {
    use std::sync::mpsc;
    use wgpu::util::DeviceExt;

    if reference.rgba.is_empty() || reference.rgba.len() % 4 != 0 {
        bail!("GPU resolve requires a nonempty RGBA8 frame")
    }
    let (adapter, status) = adapter()?;
    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("pentool-scene3d"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults(),
            memory_hints: wgpu::MemoryHints::MemoryUsage,
        },
        None,
    ))
    .context("could not create the GPU device")?;
    let bytes = reference.rgba.len() as u64;
    let input = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("pentool-scene3d-reference-frame"),
        contents: &reference.rgba,
        usage: wgpu::BufferUsages::STORAGE,
    });
    let output = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("pentool-scene3d-resolved-frame"),
        size: bytes,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("pentool-scene3d-readback"),
        size: bytes,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("pentool-scene3d-resolve"),
        source: wgpu::ShaderSource::Wgsl(
            r#"
@group(0) @binding(0) var<storage, read> source: array<u32>;
@group(0) @binding(1) var<storage, read_write> destination: array<u32>;
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x < arrayLength(&source)) { destination[id.x] = source[id.x]; }
}
"#
            .into(),
        ),
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("pentool-scene3d-resolve-layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("pentool-scene3d-resolve-pipeline-layout"),
        bind_group_layouts: &[&layout],
        push_constant_ranges: &[],
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("pentool-scene3d-resolve-pipeline"),
        layout: Some(&pipeline_layout),
        module: &shader,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("pentool-scene3d-resolve-bindings"),
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: input.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: output.as_entire_binding(),
            },
        ],
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("pentool-scene3d-resolve-commands"),
    });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("pentool-scene3d-resolve-pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bindings, &[]);
        let words = reference.rgba.len().div_ceil(4) as u32;
        pass.dispatch_workgroups(words.div_ceil(64), 1, 1);
    }
    encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, bytes);
    let submitted = queue.submit(Some(encoder.finish()));
    let slice = readback.slice(..);
    let (tx, rx) = mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    let _ = device.poll(wgpu::Maintain::WaitForSubmissionIndex(submitted));
    rx.recv()
        .context("GPU readback callback did not run")?
        .context("GPU frame readback failed")?;
    let rgba = slice.get_mapped_range().to_vec();
    readback.unmap();
    Ok((
        Image {
            width: reference.width,
            height: reference.height,
            rgba,
        },
        status,
    ))
}

#[cfg(not(feature = "gpu"))]
pub fn availability() -> Availability {
    Availability {
        available: false,
        adapter: None,
        backend: None,
        reason: Some("this binary was built without the `gpu` Cargo feature".into()),
    }
}

#[cfg(not(feature = "gpu"))]
pub fn resolve(_reference: Image) -> Result<(Image, Availability)> {
    bail!("this binary was built without the `gpu` Cargo feature")
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "gpu")]
    #[test]
    fn gpu_resolve_preserves_reference_pixels_when_an_adapter_is_available() {
        let source = super::Image {
            width: 2,
            height: 2,
            rgba: vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 252, 253, 254, 255],
        };
        if !super::availability().available {
            return;
        }
        let expected = source.rgba.clone();
        let (resolved, status) = super::resolve(source).expect("GPU resolve");
        assert!(status.available);
        assert_eq!(resolved.width, 2);
        assert_eq!(resolved.height, 2);
        assert_eq!(resolved.rgba, expected);
    }

    #[cfg(not(feature = "gpu"))]
    #[test]
    fn default_build_reports_feature_fallback() {
        let status = super::availability();
        assert!(!status.available);
        assert!(status.reason.unwrap().contains("without the `gpu`"));
    }
}
