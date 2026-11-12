//! Shared bounded ICC loading and discovery for CLI and document catalogs.
use super::icc::{
    DeviceLinkTransform, Direction, Intent, Metadata, Profile, Summary, Transform,
    TransformDescription, MAX_PROFILE_BYTES,
};
use anyhow::{ensure, Result};
use serde::Serialize;
use std::path::Path;

pub const MAX_TAG_LIMIT: usize = 256;

pub fn read_file(path: &Path) -> Result<Vec<u8>> {
    crate::resource::read_bounded(path, MAX_PROFILE_BYTES)
}

#[derive(Debug, Serialize)]
pub struct Capability {
    pub direction: Direction,
    pub intent: Intent,
    pub supported: bool,
    pub transform: Option<TransformDescription>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Inspection {
    pub format: &'static str,
    pub version: u32,
    pub structurally_readable: bool,
    #[serde(flatten)]
    pub profile: Summary,
    pub metadata: Metadata,
    pub matches: usize,
    pub returned: usize,
    pub offset: usize,
    pub limit: usize,
    pub has_more: bool,
    pub capabilities: Vec<Capability>,
    /// Device links expose their recorded forward path separately.
    pub device_link_supported: Option<bool>,
    pub device_link_error: Option<String>,
}

const INTENTS: [Intent; 4] = [
    Intent::Perceptual,
    Intent::RelativeColorimetric,
    Intent::Saturation,
    Intent::AbsoluteColorimetric,
];

pub fn inspect(bytes: &[u8], offset: usize, limit: usize) -> Result<Inspection> {
    ensure!(
        (1..=MAX_TAG_LIMIT).contains(&limit),
        "[invalid-input] ICC tag limit must be 1..={MAX_TAG_LIMIT}"
    );
    let parsed = Profile::parse(bytes)?;
    let metadata = parsed.metadata()?;
    let mut summary = parsed.summary();
    let matches = summary.tags.len();
    summary.tags = summary.tags.into_iter().skip(offset).take(limit).collect();
    let returned = summary.tags.len();
    let has_more = offset.saturating_add(returned) < matches;
    let mut capabilities = Vec::new();
    let mut device_link_supported = None;
    let mut device_link_error = None;
    if summary.class == "link" {
        match DeviceLinkTransform::compile(&parsed, INTENTS[summary.rendering_intent as usize]) {
            Ok(_) => device_link_supported = Some(true),
            Err(error) => {
                device_link_supported = Some(false);
                device_link_error = Some(format!("{error:#}"));
            }
        }
    } else {
        for direction in [Direction::DeviceToPcs, Direction::PcsToDevice] {
            for intent in INTENTS {
                let (transform, error) = match Transform::compile(&parsed, direction, intent) {
                    Ok(transform) => (Some(transform.description().clone()), None),
                    Err(error) => (None, Some(format!("{error:#}"))),
                };
                capabilities.push(Capability {
                    direction,
                    intent,
                    supported: transform.is_some(),
                    transform,
                    error,
                });
            }
        }
    }
    Ok(Inspection {
        format: "pentool-icc-inspection",
        version: 1,
        structurally_readable: true,
        profile: summary,
        metadata,
        matches,
        returned,
        offset,
        limit,
        has_more,
        capabilities,
        device_link_supported,
        device_link_error,
    })
}

/// Validate only the explicitly requested executable path; other paths are not
/// silently substituted and a readable container is not sufficient for success.
pub fn validate(
    bytes: &[u8],
    direction: Option<Direction>,
    intent: Intent,
) -> Result<serde_json::Value> {
    let parsed = Profile::parse(bytes)?;
    let metadata = parsed.metadata()?;
    let transform = match direction {
        Some(direction) => Some(
            Transform::compile(&parsed, direction, intent)?
                .description()
                .clone(),
        ),
        None => {
            DeviceLinkTransform::compile(&parsed, intent)?;
            None
        }
    };
    Ok(
        serde_json::json!({"format":"pentool-icc-validation","version":1,"ok":true,"sha256":parsed.summary().sha256,"metadata":metadata,"intent":intent,"direction":direction,"device_link":direction.is_none(),"transform":transform,"scope":"requested-profile-path"}),
    )
}
