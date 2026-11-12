//! Atomic paint assignment/conversion planning. The caller commits the planned
//! document through transaction/history with its original on-disk revision.
use super::{
    catalog,
    icc::Intent,
    scene::{self, Scope},
    value::{Destination, Evaluator, NativeColor, ProfileSet},
};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::path::Path;

#[derive(Debug, Clone, Copy)]
pub enum Operation {
    Assign,
    Convert,
}
#[derive(Debug, Clone)]
pub enum Target {
    Native(Destination),
    LegacySrgb,
}
pub struct Options<'a> {
    pub operation: Operation,
    pub target: Target,
    pub intent: Intent,
    pub page: Option<&'a str>,
    pub all_pages: bool,
    pub include_shared: bool,
    pub offset: usize,
    pub limit: usize,
}
#[derive(Debug)]
pub struct Plan {
    pub document: Value,
    pub report: Value,
}

pub fn plan(raw: &Value, document: &Path, options: &Options<'_>) -> Result<Plan> {
    ensure!(
        (1..=256).contains(&options.limit),
        "[invalid-input] paint change limit must be 1..=256"
    );
    ensure!(
        !(options.all_pages && options.page.is_some()),
        "[invalid-input] --all-pages conflicts with --page"
    );
    ensure!(
        crate::color::has_v8(raw),
        "[unsupported-capability] paint assignment/conversion requires explicit migration to v8"
    );
    crate::scene::validate(raw)?;
    let destination = match &options.target {
        Target::Native(value) => value.clone(),
        Target::LegacySrgb => Destination::parse(&json!({"space":"srgb"}))?,
    };
    ensure!(
        !matches!(
            (&options.operation, &options.target),
            (Operation::Assign, Target::LegacySrgb)
        ),
        "[invalid-input] legacy sRGB output requires conversion, because it quantizes channels"
    );
    let scope = if options.all_pages {
        Scope::Document
    } else {
        Scope::Page {
            page: options.page,
            visible_only: false,
        }
    };
    let paints = scene::paints(raw, scope)?;
    ensure!(
        !paints.is_empty(),
        "[invalid-input] selected paint scope contains no color samples"
    );
    let locked_styles = if paints.iter().any(|paint| paint.shared) {
        scene::locked_styles(raw)?
    } else {
        Default::default()
    };
    let locked_resources = if paints.iter().any(|paint| paint.resource.is_some()) {
        scene::locked_resources(raw)?
    } else {
        Default::default()
    };
    for paint in &paints {
        ensure!(!paint.resource.as_ref().is_some_and(|resource|locked_resources.contains(resource)),"[locked] {}, color {}: assignment/conversion would affect a mask used by locked content",paint.owner,paint.pointer);
        ensure!(!paint.locked && !paint.style.as_ref().is_some_and(|style|locked_styles.contains(style)),"[locked] {}, color {}: assignment/conversion would affect locked paint; unlock its node/layer or style users first",paint.owner,paint.pointer);
        ensure!(!paint.shared || options.include_shared,"[shared-resource] {}, color {}: changing a shared resource requires --include-shared and can affect other pages",paint.owner,paint.pointer);
        if matches!(options.operation, Operation::Assign) {
            ensure!(paint.color.channels().len()==destination.channel_count(),"[invalid-input] {}, color {}: assignment preserves channel numbers and cannot change channel count; use conversion",paint.owner,paint.pointer);
        }
    }
    let mut references = Vec::new();
    if let Some(reference) = destination.profile() {
        references.push(reference);
    }
    if matches!(options.operation, Operation::Convert) {
        references.extend(
            paints
                .iter()
                .filter_map(|paint| paint.color.destination().profile()),
        );
    }
    let profiles = ProfileSet::load(raw, document, references)?;
    let mut engine = Evaluator::new(&profiles, options.intent)?;
    let mut planned = raw.clone();
    let mut changed = 0usize;
    let mut clipped = 0usize;
    let mut quantized = 0usize;
    let mut changes = Vec::new();
    for paint in &paints {
        let (converted, was_clipped, path) = match options.operation {
            Operation::Assign => {
                let mut value = destination.json();
                value["channels"] = json!(paint.color.channels());
                value["alpha"] = json!(paint.color.alpha());
                let color = NativeColor::parse(&value).with_context(|| {
                    format!(
                        "{}, color {}: assignment cannot represent the original channel values",
                        paint.owner, paint.pointer
                    )
                })?;
                engine.to_xyz(&color).with_context(|| {
                    format!(
                        "{}, color {}: assigned interpretation is not executable",
                        paint.owner, paint.pointer
                    )
                })?;
                (color, false, "assign".to_owned())
            }
            Operation::Convert => {
                let result = engine
                    .convert(&paint.color, &destination)
                    .with_context(|| format!("{}, color {}", paint.owner, paint.pointer))?;
                (result.color, result.clipped, result.path)
            }
        };
        clipped += usize::from(was_clipped);
        let value = if matches!(options.target, Target::LegacySrgb) {
            let values = [
                converted.channels()[0],
                converted.channels()[1],
                converted.channels()[2],
                converted.alpha(),
            ];
            let bytes = values.map(|value| (value.clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u8);
            quantized += usize::from(
                values
                    .iter()
                    .zip(bytes)
                    .any(|(value, byte)| (value - f64::from(byte) / 255.0).abs() > 1e-12),
            );
            json!(if bytes[3] == 255 {
                format!("#{:02x}{:02x}{:02x}", bytes[0], bytes[1], bytes[2])
            } else {
                format!(
                    "#{:02x}{:02x}{:02x}{:02x}",
                    bytes[0], bytes[1], bytes[2], bytes[3]
                )
            })
        } else {
            converted.json()
        };
        let original = raw
            .pointer(&paint.pointer)
            .context("paint source pointer disappeared")?;
        if original != &value {
            if changed >= options.offset && changes.len() < options.limit {
                changes.push(json!({"pointer":paint.pointer,"owner":paint.owner,"shared":paint.shared,"before":original,"after":value,"clipped":was_clipped,"path":path}));
            }
            changed += 1;
            *planned
                .pointer_mut(&paint.pointer)
                .context("paint target pointer disappeared")? = value;
        }
    }
    let resources = scene::replace_mask_resources(&mut planned)?;
    crate::scene::validate(&planned)?;
    catalog::size_gate(&planned)?;
    let report = json!({"version":1,"operation":match options.operation {Operation::Assign=>"assign",Operation::Convert=>"convert"},"scope":"scene-paints","all_pages":options.all_pages,"include_shared":options.include_shared,"intent":options.intent,"target":if matches!(options.target,Target::LegacySrgb){json!("legacy-srgb8")}else{destination.json()},"matched":paints.len(),"matches":changed,"changed":changed,"unchanged":paints.len()-changed,"clipped":clipped,"quantized":quantized,"returned":changes.len(),"offset":options.offset,"limit":options.limit,"has_more":options.offset.saturating_add(changes.len())<changed,"changes":changes});
    let mut report = report;
    report["resource_replacements"] = json!(resources);
    Ok(Plan {
        document: planned,
        report,
    })
}
