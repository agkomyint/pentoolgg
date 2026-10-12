//! Native fill interpolation shares the numeric sampler and CMM. Geometry and
//! antialiasing remain in the existing fill renderer.
use super::{
    icc::Intent,
    value::{Destination, Evaluator, NativeColor},
};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::path::Path;

pub(crate) fn setting(raw: &Value, fill: &Value) -> Result<Option<(Destination, Intent)>> {
    if !fill["space"].is_object() {
        return Ok(None);
    }
    ensure!(
        crate::color::has_v8(raw),
        "[unsupported-capability] explicit native gradient interpolation requires version 8"
    );
    let destination = Destination::parse(&fill["space"])?;
    let intent: Intent = fill["intent"]
        .as_str()
        .context("[invalid-fill] native gradient requires an explicit intent")?
        .parse()?;
    Ok(Some((destination, intent)))
}

/// Resolve only the supported paint grammar, never arbitrary extensions.
pub(crate) fn color(value: &Value, styles: &Value, depth: usize) -> Result<NativeColor> {
    ensure!(
        depth <= 64,
        "[invalid-fill] gradient color token depth exceeded"
    );
    if value.get("space").is_some() {
        return NativeColor::parse(value);
    }
    if let Some(reference) = value["ref"].as_str() {
        if let Some(value) = styles.get(reference).and_then(|token| token.get("value")) {
            return color(value, styles, depth + 1);
        }
    }
    if let Some(fallback) = value.get("fallback") {
        return color(fallback, styles, depth + 1);
    }
    NativeColor::from_legacy(value)?.map_or_else(
        || NativeColor::parse(&json!({"space":"srgb","channels":[0,0,0],"alpha":0})),
        Ok,
    )
}

pub(crate) fn sources(fill: &Value, styles: &Value) -> Result<Vec<(f64, NativeColor)>> {
    let stops = fill["stops"]
        .as_array()
        .filter(|s| (2..=64).contains(&s.len()))
        .context("[invalid-fill] gradients require 2..64 stops")?;
    let out: Vec<_> = stops
        .iter()
        .map(|stop| {
            let position = stop["position"]
                .as_f64()
                .filter(|p| (0.0..=1.0).contains(p))
                .context("[invalid-fill] gradient position must be in 0..1")?;
            Ok((position, color(&stop["color"], styles, 0)?))
        })
        .collect::<Result<_>>()?;
    ensure!(
        out[0].0 == 0.0 && out.last().unwrap().0 == 1.0 && out.windows(2).all(|w| w[0].0 < w[1].0),
        "[invalid-fill] gradient stops must increase strictly from 0 to 1"
    );
    Ok(out)
}

pub(crate) struct Gradient {
    stops: Vec<(f64, NativeColor)>,
    sample: NativeColor,
    conversion_clipped: bool,
}
impl Gradient {
    pub(crate) fn prepare(
        sources: Vec<(f64, NativeColor)>,
        destination: &Destination,
        engine: &mut Evaluator<'_>,
        working: Option<&Destination>,
    ) -> Result<Self> {
        let mut conversion_clipped = false;
        let stops: Vec<_> = sources
            .into_iter()
            .map(|(position, color)| {
                let evaluated = engine.convert(&color, destination)?;
                conversion_clipped |= evaluated.clipped;
                Ok((position, evaluated.color))
            })
            .collect::<Result<_>>()?;
        let sample = stops[0].1.clone();
        // Compile the destination appearance path before beginning pixel work,
        // including when conversion of every endpoint was an identity.
        if let Some(working) = working {
            conversion_clipped |= engine.working_rgba(&sample, working)?.1;
        } else {
            engine.preview_rgba(&sample)?;
        }
        Ok(Self {
            stops,
            sample,
            conversion_clipped,
        })
    }
    pub(crate) fn conversion_clipped(&self) -> bool {
        self.conversion_clipped
    }

    pub(crate) fn sample(
        &mut self,
        engine: &mut Evaluator<'_>,
        position: f64,
        spread: &str,
    ) -> Result<[f64; 4]> {
        engine.preview_rgba(self.sample_color(position, spread)?)
    }

    pub(crate) fn sample_working(
        &mut self,
        engine: &mut Evaluator<'_>,
        working: &Destination,
        position: f64,
        spread: &str,
    ) -> Result<([f64; 4], bool)> {
        engine.working_rgba(self.sample_color(position, spread)?, working)
    }

    fn sample_color(&mut self, position: f64, spread: &str) -> Result<&NativeColor> {
        ensure!(
            position.is_finite(),
            "[invalid-fill] gradient geometry produced a nonfinite sample position"
        );
        let position = crate::fill::spread_position(position, spread);
        let index = self.stops.partition_point(|(p, _)| *p < position);
        if index == 0 {
            return Ok(&self.stops[0].1);
        }
        if index == self.stops.len() {
            return Ok(&self.stops[index - 1].1);
        }
        let (a, b) = (&self.stops[index - 1], &self.stops[index]);
        self.sample
            .blend(&a.1, &b.1, (position - a.0) / (b.0 - a.0))?;
        Ok(&self.sample)
    }
}

/// Native gradient stops must reach the fill renderer before RGB lowering.
pub(crate) fn preserves_slot(raw: &Value, pointer: &str) -> bool {
    pointer
        .rsplit_once("/stops/")
        .and_then(|(fill, _)| raw.pointer(fill))
        .is_some_and(|fill| {
            matches!(fill["kind"].as_str(), Some("linear" | "radial" | "conic"))
                && fill["space"].is_object()
        })
}

/// Freeze token values only in the private render clone, before ordinary style
/// values are lowered to RGB. Stored bindings and fallbacks remain editable.
pub(crate) fn materialize(raw: &mut Value, slots: &[super::scene::DestinationSlot]) -> Result<()> {
    let mut replacements = Vec::new();
    for slot in slots {
        let pointer = slot.pointer.strip_suffix("/space").unwrap();
        let fill = raw
            .pointer(pointer)
            .context("native gradient disappeared")?;
        let colors = sources(fill, &raw["styles"])?
            .into_iter()
            .map(|(_, color)| color.json())
            .collect::<Vec<_>>();
        replacements.push((pointer.to_owned(), colors));
    }
    for (pointer, colors) in replacements {
        let fill = raw
            .pointer_mut(&pointer)
            .context("native gradient disappeared")?;
        for (stop, color) in fill["stops"].as_array_mut().unwrap().iter_mut().zip(colors) {
            stop["color"] = color;
        }
    }
    Ok(())
}

/// One profile snapshot per render, reused by every gradient and intent cache.
pub(crate) fn profiles(
    raw: &Value,
    document: &Path,
    scope: super::scene::Scope<'_>,
) -> Result<super::value::ProfileSet> {
    if !crate::color::has_v8(raw) {
        return Ok(super::value::ProfileSet::default());
    }
    let destinations = super::scene::destinations(raw, scope)?;
    if destinations.is_empty() {
        return Ok(super::value::ProfileSet::default());
    }
    let colors = super::scene::slots(raw, scope)?;
    super::value::ProfileSet::load(
        raw,
        document,
        destinations
            .iter()
            .filter_map(|slot| slot.destination.profile())
            .chain(
                colors
                    .iter()
                    .filter(|slot| preserves_slot(raw, &slot.pointer))
                    .filter_map(|slot| slot.color.destination().profile()),
            ),
    )
}

pub(crate) fn engine<'a, 'b>(
    profiles: &'a super::value::ProfileSet,
    engines: &'b mut Vec<(Intent, Evaluator<'a>)>,
    intent: Intent,
) -> Result<&'b mut Evaluator<'a>> {
    let index = if let Some(index) = engines.iter().position(|(used, _)| *used == intent) {
        index
    } else {
        engines.push((intent, Evaluator::new(profiles, intent)?));
        engines.len() - 1
    };
    Ok(&mut engines[index].1)
}

/// Caller has verified that the complete catalog is embedded and shareable.
/// Compile every declared gradient, including unused shared definitions, using
/// one snapshot and at most four bounded intent contexts.
pub(crate) fn validate_embedded_paths(raw: &Value) -> Result<()> {
    let destinations = super::scene::destinations(raw, super::scene::Scope::Document)?;
    if destinations.is_empty() {
        return Ok(());
    }
    let colors = super::scene::slots(raw, super::scene::Scope::Document)?;
    let profiles = super::value::ProfileSet::load(
        raw,
        Path::new("embedded-print-package.pen"),
        destinations
            .iter()
            .filter_map(|slot| slot.destination.profile())
            .chain(
                colors
                    .iter()
                    .filter_map(|slot| slot.color.destination().profile()),
            ),
    )?;
    let mut engines = Vec::new();
    for slot in destinations {
        let fill = raw
            .pointer(slot.pointer.strip_suffix("/space").unwrap())
            .unwrap();
        let sources = sources(fill, &raw["styles"])?;
        Gradient::prepare(
            sources,
            &slot.destination,
            engine(&profiles, &mut engines, slot.intent)?,
            None,
        )
        .with_context(|| format!("{}, gradient {}", slot.owner, slot.pointer))?;
    }
    Ok(())
}
