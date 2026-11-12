//! Read-only, sample-level process ink analysis. A paint or gradient stop is one
//! sample, irrespective of its rendered area. Production coverage and plates
//! require the future scene separation renderer.
use super::{
    pcs,
    roles::{self, Role},
    scene::{self, Scope},
    value::{Evaluator, ProfileSet},
};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::path::Path;

const PLATE_EPSILON: f64 = 0.02;
const RICH_K_MIN: f64 = 0.8;
const RICH_CMY_MIN: f64 = 0.2;
const REGISTRATION_MIN: f64 = 0.95;

/// Recipe inputs are explicit. `units_per_inch` converts stored text sizes to
/// points; no page or profile name supplies an implicit physical scale.
#[derive(Debug, Clone, Copy)]
pub struct Thresholds {
    pub tac_percent: f64,
    pub small_text_points: f64,
    pub units_per_inch: f64,
    pub neutral_chroma: f64,
}

impl Thresholds {
    fn validate(self) -> Result<()> {
        ensure!(
            self.tac_percent.is_finite() && (0.0..=400.0).contains(&self.tac_percent),
            "[invalid-input] ink TAC limit must be finite and in [0,400] percent"
        );
        ensure!(
            self.small_text_points.is_finite()
                && self.small_text_points > 0.0
                && self.small_text_points <= 1000.0,
            "[invalid-input] small-text threshold must be finite and in (0,1000] points"
        );
        ensure!(
            self.units_per_inch.is_finite()
                && self.units_per_inch > 0.0
                && self.units_per_inch <= 1_000_000.0,
            "[invalid-input] document units per inch must be finite and in (0,1000000]"
        );
        ensure!(
            self.neutral_chroma.is_finite() && (0.0..=150.0).contains(&self.neutral_chroma),
            "[invalid-input] neutral CIE76 chroma threshold must be finite and in [0,150]"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
struct Classes {
    tac: f64,
    plates: usize,
    above_limit: bool,
    rich_black: bool,
    registration_black: bool,
    neutral: bool,
    neutral_multi_ink: bool,
    small_text_multi_plate: bool,
}

fn classify(
    channels: [f64; 4],
    chroma: f64,
    text_points: Option<f64>,
    limits: Thresholds,
) -> Classes {
    let tac = channels.iter().sum::<f64>() * 100.0;
    let plates = channels.iter().filter(|&&v| v > PLATE_EPSILON).count();
    let neutral = chroma <= limits.neutral_chroma;
    Classes {
        tac,
        plates,
        above_limit: tac > limits.tac_percent,
        rich_black: channels[3] >= RICH_K_MIN && channels[..3].iter().sum::<f64>() >= RICH_CMY_MIN,
        registration_black: channels.iter().all(|&v| v >= REGISTRATION_MIN),
        neutral,
        neutral_multi_ink: neutral && plates > 1,
        small_text_multi_plate: text_points
            .is_some_and(|size| size < limits.small_text_points && plates > 1),
    }
}

fn text_points(raw: &Value, pointer: &str, units_per_inch: f64) -> Option<f64> {
    let node_pointer = pointer
        .split_once("/style/")
        .or_else(|| pointer.split_once("/effects/"))?
        .0;
    let node = raw.pointer(node_pointer)?;
    if node["kind"] != "text" {
        return None;
    }
    node["font_size"]
        .as_f64()
        .map(|size| size * 72.0 / units_per_inch)
}

/// Analyze visible native paints on one page through its assigned CMYK output
/// condition. Results are deterministic, bounded, and never mutate the document.
/// `offset` and `limit` page records after the complete summary is computed.
pub fn analyze_page(
    raw: &Value,
    document: &Path,
    page: Option<&str>,
    thresholds: Thresholds,
    offset: usize,
    limit: usize,
) -> Result<Value> {
    thresholds.validate()?;
    ensure!(
        (1..=256).contains(&limit),
        "[invalid-input] ink report limit must be in 1..=256"
    );
    let output = roles::effective(raw, Role::Output).context(
        "ink analysis needs an assigned CMYK output role; assign a verified press profile",
    )?;
    ensure!(output.destination.space() == "icc-cmyk", "[unsupported-capability] ink analysis needs a CMYK ICC output role; assign a CMYK press profile");
    let slots = scene::paints(
        raw,
        Scope::Page {
            page,
            visible_only: true,
        },
    )?;
    let references = slots
        .iter()
        .filter_map(|slot| slot.color.destination().profile())
        .chain(output.destination.profile());
    let profiles = ProfileSet::load(raw, document, references)?;
    let mut evaluator = Evaluator::new(&profiles, output.intent)?;
    let mut minima = [f64::INFINITY; 4];
    let mut maxima = [f64::NEG_INFINITY; 4];
    let mut maximum_tac: f64 = 0.0;
    let mut above_limit = 0usize;
    let mut rich_black = 0usize;
    let mut registration_black = 0usize;
    let mut neutral_multi_ink = 0usize;
    let mut small_text_multi_plate = 0usize;
    let mut clipped = 0usize;
    let mut records = Vec::with_capacity(limit.min(slots.len().saturating_sub(offset)));
    for (index, slot) in slots.iter().enumerate() {
        let evaluated = evaluator
            .convert(&slot.color, &output.destination)
            .with_context(|| format!("ink analysis {} at {}", slot.owner, slot.pointer))?;
        let channels: [f64; 4] = evaluated.color.channels().try_into().context(
            "[unsupported-capability] output profile did not return four process channels",
        )?;
        let (xyz, source_clipped) = evaluator.to_xyz(&slot.color)?;
        let lab = pcs::xyz_to_lab(xyz)?;
        let chroma = lab[1].hypot(lab[2]);
        let points = text_points(raw, &slot.pointer, thresholds.units_per_inch);
        let classes = classify(channels, chroma, points, thresholds);
        for plate in 0..4 {
            minima[plate] = minima[plate].min(channels[plate]);
            maxima[plate] = maxima[plate].max(channels[plate]);
        }
        maximum_tac = maximum_tac.max(classes.tac);
        above_limit += usize::from(classes.above_limit);
        rich_black += usize::from(classes.rich_black);
        registration_black += usize::from(classes.registration_black);
        neutral_multi_ink += usize::from(classes.neutral_multi_ink);
        small_text_multi_plate += usize::from(classes.small_text_multi_plate);
        clipped += usize::from(evaluated.clipped || source_clipped);
        if index >= offset && records.len() < limit {
            records.push(json!({
                "pointer":slot.pointer,"owner":slot.owner,
                "channels":{"c":channels[0],"m":channels[1],"y":channels[2],"k":channels[3]},
                "alpha":slot.color.alpha(),"tac_percent":classes.tac,
                "active_plates":classes.plates,"above_tac_limit":classes.above_limit,
                "rich_black":classes.rich_black,"registration_black":classes.registration_black,
                "source_neutral":classes.neutral,"neutral_multi_ink":classes.neutral_multi_ink,
                "text_points":points,"small_text_multi_plate":classes.small_text_multi_plate,
                "conversion_clipped":evaluated.clipped || source_clipped,
                "path":evaluated.path,"link":evaluated.link,
            }));
        }
    }
    let matches = slots.len();
    let returned = records.len();
    Ok(json!({
        "version":1,"scope":"visible-paint-samples","page":page.unwrap_or_else(|| raw["pages"][0]["id"].as_str().unwrap_or("")),
        "output_profile":output.destination.profile(),"intent":output.intent,
        "thresholds":{"tac_percent":thresholds.tac_percent,"small_text_points":thresholds.small_text_points,"units_per_inch":thresholds.units_per_inch,"neutral_chroma":thresholds.neutral_chroma,
            "plate_epsilon":PLATE_EPSILON,"rich_k_min":RICH_K_MIN,"rich_cmy_min":RICH_CMY_MIN,"registration_min":REGISTRATION_MIN},
        "summary":{"sample_count":matches,"channel_min":if matches==0 {None} else {Some(minima)},"channel_max":if matches==0 {None} else {Some(maxima)},
            "maximum_tac_percent":if matches==0 {None} else {Some(maximum_tac)},
            "above_tac_limit_samples":above_limit,"rich_black_samples":rich_black,
            "registration_black_samples":registration_black,"neutral_multi_ink_samples":neutral_multi_ink,
            "small_text_multi_plate_samples":small_text_multi_plate,"conversion_clipped_samples":clipped},
        "matches":matches,"returned":returned,"offset":offset,"limit":limit,"has_more":offset.saturating_add(returned)<matches,
        "samples":records,"mutated":false,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn limits() -> Thresholds {
        Thresholds {
            tac_percent: 300.0,
            small_text_points: 8.0,
            units_per_inch: 72.0,
            neutral_chroma: 2.0,
        }
    }
    #[test]
    fn classes_have_explicit_boundaries() {
        let rich = classify([0.4, 0.4, 0.3, 0.9], 0.0, Some(7.0), limits());
        assert!(rich.neutral_multi_ink && rich.rich_black && rich.small_text_multi_plate);
        assert!(!rich.above_limit && !rich.registration_black);
        let registration = classify([1.0; 4], 8.0, Some(9.0), limits());
        assert!(registration.registration_black && registration.above_limit);
        assert!(!registration.small_text_multi_plate && !registration.neutral);
    }
    #[test]
    fn invalid_recipe_values_fail() {
        let mut value = limits();
        value.tac_percent = f64::NAN;
        assert!(value.validate().is_err());
        value = limits();
        value.units_per_inch = 0.0;
        assert!(value.validate().is_err());
    }

    #[test]
    fn text_size_comes_from_the_owner_node() {
        let raw = json!({"pages":[{"layers":[{"nodes":[{"kind":"text","font_size":7.0,"style":{"fill":{}}},{"kind":"rect","font_size":7.0}]}]}]});
        assert_eq!(
            text_points(&raw, "/pages/0/layers/0/nodes/0/style/fill/fallback", 72.0),
            Some(7.0)
        );
        assert_eq!(
            text_points(
                &raw,
                "/pages/0/layers/0/nodes/0/effects/0/params/color",
                144.0
            ),
            Some(3.5)
        );
        assert_eq!(
            text_points(&raw, "/pages/0/layers/0/nodes/1/style/fill", 72.0),
            None
        );
        assert_eq!(text_points(&raw, "/styles/ink/value", 72.0), None);
    }
}
