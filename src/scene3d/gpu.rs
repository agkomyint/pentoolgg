//! Optional `wgpu` adapter discovery and accelerator boundary.
//!
//! CPU pixels remain normative. This module never claims acceleration until a
//! complete GPU frame has been produced; callers receive an explicit fallback
//! reason otherwise.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Availability {
    pub available: bool,
    pub adapter: Option<String>,
    pub backend: Option<String>,
    pub reason: Option<String>,
}

#[cfg(feature = "gpu")]
pub fn availability() -> Availability {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }));
    match adapter {
        Some(adapter) => {
            let info = adapter.get_info();
            Availability {
                // Adapter discovery alone is not a renderer. Keep fallback
                // explicit until the GPU raster pipeline returns final pixels.
                available: false,
                adapter: Some(info.name),
                backend: Some(format!("{:?}", info.backend).to_ascii_lowercase()),
                reason: Some(
                    "GPU adapter found, but the scene3d GPU raster pipeline is not complete".into(),
                ),
            }
        }
        None => Availability {
            available: false,
            adapter: None,
            backend: None,
            reason: Some("no compatible GPU adapter was found".into()),
        },
    }
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

#[cfg(test)]
mod tests {
    #[cfg(not(feature = "gpu"))]
    #[test]
    fn default_build_reports_feature_fallback() {
        let status = super::availability();
        assert!(!status.available);
        assert!(status.reason.unwrap().contains("without the `gpu`"));
    }
}
