//! Reusable print recipes: a small, strictly validated JSON file that supplies
//! defaults to `pentool color pdfx` and `pentool color preflight`.
//! See "Print recipes (implemented subset)" in `docs/professional-color-v1.md`.
//!
//! ```json
//! {"version":1,"name":"magazine-print","pdf_standard":"PDF/X-4",
//!  "intent":"relative-colorimetric","ppi":300,"min_ppi":200,
//!  "min_bleed_inches":0.125,"bleed_scale":1,"max_total_ink":300,
//!  "plate_bits":16,"auto_trap":0.25,"profile":"press"}
//! ```
//!
//! Every field except `version` and `name` is optional; unknown fields are
//! rejected so a typo never silently loses a threshold. Explicit command-line
//! flags always override a recipe.
use super::icc::Intent;
use anyhow::{bail, ensure, Context, Result};
use serde_json::Value;
use std::path::Path;

pub const MAX_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct Recipe {
    pub name: String,
    pub intent: Option<Intent>,
    pub ppi: Option<f64>,
    pub min_ppi: Option<f64>,
    pub min_bleed_inches: Option<f64>,
    pub bleed_scale: Option<f32>,
    pub max_total_ink: Option<f64>,
    /// Plate PNG bit depth for `color separate` (8 or 16).
    pub plate_bits: Option<u8>,
    /// Automatic trap width in document units for `color separate`.
    pub auto_trap: Option<f64>,
    /// Plate render scale for `color separate` and `color package` (0.1..=8).
    pub scale: Option<f32>,
    /// Spot ink IDs preflight accepts; others are errors.
    pub allowed_spots: Option<Vec<String>>,
    /// `"k-only-text"`: text in ICC CMYK may carry only K (preflight error).
    pub k_only_text: bool,
    /// Require page production marks (`marks-missing` preflight error).
    pub require_marks: bool,
    /// PDF stream compression: `false` for `"none"`; `None` keeps Flate.
    pub compress: Option<bool>,
    /// `spot_policy: "reject"`: spot paints that PDF/X would flatten are errors.
    pub reject_spot_flattening: bool,
    /// Spot ink substitutions as sorted `(from, to)` pairs: every paint of the
    /// first ink is separated onto the second ink's plate.
    pub spot_substitutions: Vec<(String, String)>,
    /// Catalog name of the ICC CMYK output profile (used as an `icc-cmyk`
    /// destination when no explicit `--to` is given).
    pub profile: Option<String>,
}

impl Recipe {
    /// Move every spot paint of a substituted ink onto its target ink, in memory
    /// only. Both inks must exist in the document's library, so a misspelled ID
    /// is an error rather than a silent no-op. Returns the paints changed.
    pub fn substitute_spots(&self, raw: &mut serde_json::Value) -> Result<usize> {
        if self.spot_substitutions.is_empty() {
            return Ok(0);
        }
        let library = super::spot::document_library(raw)?;
        let mut resolved = Vec::new();
        for (from, to) in &self.spot_substitutions {
            let from = library.resolve(from).with_context(|| {
                format!("[missing-resource] recipe spot_substitutions names unknown ink {from}")
            })?;
            let to = library.resolve(to).with_context(|| {
                format!("[missing-resource] recipe spot_substitutions targets unknown ink {to}")
            })?;
            resolved.push((from.id.clone(), to.id.clone()));
        }
        fn walk(
            nodes: &mut [serde_json::Value],
            resolved: &[(String, String)],
            library: &super::spot::Library,
            changed: &mut usize,
            depth: usize,
        ) {
            if depth > 64 {
                return;
            }
            for node in nodes {
                for slot in ["fill", "stroke"] {
                    let Some(ink) = node["style"][slot]["spot"]["ink"].as_str() else {
                        continue;
                    };
                    let Ok(found) = library.resolve(ink) else {
                        continue;
                    };
                    if let Some((_, to)) = resolved.iter().find(|(from, _)| *from == found.id) {
                        node["style"][slot]["spot"]["ink"] = serde_json::json!(to);
                        *changed += 1;
                    }
                }
                if let Some(children) = node.get_mut("children").and_then(|c| c.as_array_mut()) {
                    walk(children, resolved, library, changed, depth + 1);
                }
            }
        }
        let mut changed = 0;
        if let Some(pages) = raw.get_mut("pages").and_then(|p| p.as_array_mut()) {
            for page in pages {
                for layer in page["layers"].as_array_mut().into_iter().flatten() {
                    if let Some(nodes) = layer.get_mut("nodes").and_then(|n| n.as_array_mut()) {
                        walk(nodes, &resolved, &library, &mut changed, 0);
                    }
                }
            }
        }
        Ok(changed)
    }

    /// The output destination the recipe names, if any.
    pub fn destination(&self) -> Option<super::value::Destination> {
        let profile = self.profile.as_ref()?;
        super::value::Destination::parse(&serde_json::json!({"space":"icc-cmyk","profile":profile}))
            .ok()
    }
}

fn range(value: &Value, key: &str, min: f64, max: f64) -> Result<f64> {
    value
        .as_f64()
        .filter(|v| v.is_finite() && (min..=max).contains(v))
        .with_context(|| format!("[invalid-input] recipe {key} must be a number in {min}..={max}"))
}

pub fn parse(bytes: &[u8]) -> Result<Recipe> {
    ensure!(
        bytes.len() <= MAX_BYTES,
        "[limit-exceeded] a print recipe is limited to {MAX_BYTES} bytes"
    );
    let value: Value = serde_json::from_slice(bytes).context("invalid print recipe JSON")?;
    let fields = value
        .as_object()
        .context("[invalid-input] a print recipe must be a JSON object")?;
    ensure!(
        fields.get("version") == Some(&Value::from(1)),
        "[unsupported-version] print recipe version must be 1"
    );
    let name = fields
        .get("name")
        .and_then(Value::as_str)
        .filter(|n| {
            !n.is_empty()
                && n.len() <= 80
                && n.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        })
        .context("[invalid-input] recipe name must be 1..=80 characters of A-Z a-z 0-9 - _ .")?
        .to_owned();
    let mut recipe = Recipe {
        name,
        intent: None,
        ppi: None,
        min_ppi: None,
        min_bleed_inches: None,
        bleed_scale: None,
        max_total_ink: None,
        plate_bits: None,
        auto_trap: None,
        scale: None,
        allowed_spots: None,
        k_only_text: false,
        require_marks: false,
        compress: None,
        reject_spot_flattening: false,
        spot_substitutions: Vec::new(),
        profile: None,
    };
    for (key, value) in fields {
        match key.as_str() {
            "version" | "name" => {}
            "pdf_standard" => ensure!(
                value == "PDF/X-4",
                "[unsupported-capability] recipe pdf_standard {value} is not supported; only \"PDF/X-4\" is"
            ),
            "intent" => {
                recipe.intent = Some(
                    value
                        .as_str()
                        .context("[invalid-input] recipe intent must be a string")?
                        .parse()?,
                )
            }
            "ppi" => recipe.ppi = Some(range(value, key, 36.0, 1200.0)?),
            "min_ppi" => recipe.min_ppi = Some(range(value, key, 1.0, 2400.0)?),
            "min_bleed_inches" => recipe.min_bleed_inches = Some(range(value, key, 0.0, 4.0)?),
            "bleed_scale" => recipe.bleed_scale = Some(range(value, key, 0.0, 8.0)? as f32),
            "max_total_ink" => recipe.max_total_ink = Some(range(value, key, 100.0, 400.0)?),
            "plate_bits" => {
                recipe.plate_bits = Some(match value.as_u64() {
                    Some(8) => 8,
                    Some(16) => 16,
                    _ => bail!("[invalid-input] recipe plate_bits must be 8 or 16"),
                })
            }
            "spot_policy" => match value.as_str() {
                Some("reject") => recipe.reject_spot_flattening = true,
                Some("convert") => recipe.reject_spot_flattening = false,
                _ => bail!("[invalid-input] recipe spot_policy must be \"convert\" or \"reject\""),
            },
            "compression" => match value.as_str() {
                Some("flate") => recipe.compress = Some(true),
                Some("none") => recipe.compress = Some(false),
                _ => bail!("[invalid-input] recipe compression must be \"flate\" or \"none\""),
            },
            "require_marks" => {
                recipe.require_marks = value
                    .as_bool()
                    .context("[invalid-input] recipe require_marks must be a boolean")?
            }
            "black_policy" => match value.as_str() {
                Some("k-only-text") => recipe.k_only_text = true,
                Some("any") => recipe.k_only_text = false,
                _ => bail!(
                    "[invalid-input] recipe black_policy must be \"any\" or \"k-only-text\""
                ),
            },
            "allowed_spots" => {
                let list = value
                    .as_array()
                    .filter(|a| a.len() <= 32)
                    .context("[invalid-input] recipe allowed_spots must be an array of at most 32 ink IDs")?;
                let mut ids = Vec::new();
                for item in list {
                    let id = item
                        .as_str()
                        .filter(|s| {
                            !s.is_empty()
                                && s.len() <= 64
                                && s.chars()
                                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
                        })
                        .context("[invalid-input] recipe allowed_spots entries must be ink IDs of 1..=64 characters of A-Z a-z 0-9 - _ .")?;
                    ensure!(
                        !ids.iter().any(|existing| existing == id),
                        "[invalid-input] recipe allowed_spots repeats {id}"
                    );
                    ids.push(id.to_owned());
                }
                recipe.allowed_spots = Some(ids);
            }
            "spot_substitutions" => {
                let map = value
                    .as_object()
                    .filter(|m| m.len() <= 16)
                    .context("[invalid-input] recipe spot_substitutions must be an object of at most 16 ink IDs")?;
                let valid = |s: &str| {
                    !s.is_empty()
                        && s.len() <= 64
                        && s.chars()
                            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
                };
                for (from, to) in map {
                    let to = to
                        .as_str()
                        .filter(|t| valid(t) && valid(from))
                        .context("[invalid-input] recipe spot_substitutions maps ink IDs of 1..=64 characters of A-Z a-z 0-9 - _ . to ink IDs")?;
                    ensure!(
                        from != to,
                        "[invalid-input] recipe spot_substitutions maps {from} to itself"
                    );
                    ensure!(
                        !map.contains_key(to),
                        "[invalid-input] recipe spot_substitutions chains {from} through {to}; map each ink directly to its final ink"
                    );
                    recipe.spot_substitutions.push((from.clone(), to.to_owned()));
                }
                recipe.spot_substitutions.sort();
            }
            "scale" => recipe.scale = Some(range(value, key, 0.1, 8.0)? as f32),
            "auto_trap" => recipe.auto_trap = Some(range(value, key, 0.001, 1024.0)?),
            "profile" => {
                let name = value
                    .as_str()
                    .filter(|n| !n.is_empty() && n.len() <= 128 && !n.chars().any(char::is_control))
                    .context("[invalid-input] recipe profile must be a catalog profile name of 1..=128 characters")?;
                recipe.profile = Some(name.to_owned());
            }
            other => bail!("[invalid-input] unknown print recipe field {other}"),
        }
    }
    ensure!(
        recipe.profile.is_none() || recipe.intent.is_some(),
        "[invalid-input] a recipe that names a profile must also name an intent"
    );
    Ok(recipe)
}

pub fn load(path: &Path) -> Result<Recipe> {
    parse(&crate::resource::read_bounded(path, MAX_BYTES)?)
}

/// Most recipes a document may embed under `print_recipes`.
pub const MAX_EMBEDDED: usize = 16;

/// The recipe named `name` in the document's top-level `print_recipes` array.
pub fn embedded(raw: &Value, name: &str) -> Result<Recipe> {
    let list = raw
        .get("print_recipes")
        .and_then(Value::as_array)
        .with_context(|| {
            format!("[missing-resource] document embeds no print_recipes; cannot resolve @{name}")
        })?;
    ensure!(
        list.len() <= MAX_EMBEDDED,
        "[limit-exceeded] a document may embed at most {MAX_EMBEDDED} print recipes"
    );
    let found = list
        .iter()
        .find(|item| item.get("name").and_then(Value::as_str) == Some(name))
        .with_context(|| format!("[missing-resource] document has no embedded recipe {name:?}"))?;
    parse(&serde_json::to_vec(found)?)
}

/// Resolve a `--recipe` argument: `@NAME` names a recipe embedded in `raw`;
/// anything else is a recipe file path.
pub fn resolve(spec: &Path, raw: &Value) -> Result<Recipe> {
    match spec.to_str().and_then(|text| text.strip_prefix('@')) {
        Some(name) => embedded(raw, name),
        None => load(spec),
    }
}
