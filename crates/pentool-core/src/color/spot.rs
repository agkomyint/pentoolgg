//! Bounded, deterministic spot-ink definitions for offline print workflows.
//!
//! This is the data foundation for scene-level spot paints and plate rendering.
//! It deliberately does not fetch commercial libraries or infer licensing.

use super::value::NativeColor;
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_INKS: usize = 512;
pub const MAX_ALIASES: usize = 2048;
pub const MAX_LIBRARY_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_NAME_BYTES: usize = 256;
pub const MAX_PROVENANCE_BYTES: usize = 4096;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    /// User-declared origin. This is descriptive data, never fetched.
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ink {
    /// Stable document/library identifier; aliases and display names are not identity.
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    /// Process/RGB alternate used for preview only, never as a plate substitute.
    pub alternate: Value,
    pub provenance: Provenance,
    /// `allowed`, `restricted`, or `unknown`; never inferred from source text.
    pub redistribution: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Library {
    pub schema: u32,
    #[serde(default)]
    pub inks: BTreeMap<String, Ink>,
}

impl Default for Library {
    fn default() -> Self {
        Self {
            schema: 1,
            inks: BTreeMap::new(),
        }
    }
}

fn validate_id(value: &str, label: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 64
            && value.as_bytes()[0].is_ascii_alphanumeric()
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)),
        "[invalid-input] {label} must match ^[A-Za-z0-9][A-Za-z0-9._-]{{0,63}}$"
    );
    Ok(())
}

fn bounded_text(value: &str, label: &str, max: usize, nonempty: bool) -> Result<()> {
    ensure!(
        value.len() <= max,
        "[limit-exceeded] {label} exceeds {max} UTF-8 bytes"
    );
    ensure!(
        !nonempty || !value.trim().is_empty(),
        "[invalid-input] {label} cannot be empty"
    );
    ensure!(
        !value.chars().any(char::is_control),
        "[invalid-input] {label} contains control characters"
    );
    Ok(())
}

fn alias_key(value: &str) -> String {
    value.to_ascii_lowercase()
}

impl Ink {
    pub fn validate(&self) -> Result<()> {
        validate_id(&self.id, "ink ID")?;
        bounded_text(&self.name, "ink name", MAX_NAME_BYTES, true)?;
        ensure!(
            self.aliases.len() <= 64,
            "[limit-exceeded] an ink may have at most 64 aliases"
        );
        let mut own_aliases = BTreeSet::new();
        for alias in &self.aliases {
            validate_id(alias, "ink alias")?;
            ensure!(
                alias_key(alias) != alias_key(&self.id),
                "[invalid-input] ink alias duplicates its stable ID"
            );
            ensure!(
                own_aliases.insert(alias_key(alias)),
                "[conflict] duplicate alias on ink {}",
                self.id
            );
        }
        let alternate = NativeColor::parse(&self.alternate)
            .context("[invalid-input] spot ink alternate must be a supported native color")?;
        ensure!(
            alternate.destination().space() != "spot",
            "[invalid-input] a spot alternate cannot reference another spot ink"
        );
        bounded_text(
            &self.provenance.source,
            "ink provenance source",
            MAX_PROVENANCE_BYTES,
            true,
        )?;
        for (label, value) in [
            ("publisher", self.provenance.publisher.as_deref()),
            ("revision", self.provenance.revision.as_deref()),
            ("license", self.provenance.license.as_deref()),
        ] {
            if let Some(value) = value {
                bounded_text(
                    value,
                    &format!("ink provenance {label}"),
                    MAX_PROVENANCE_BYTES,
                    true,
                )?;
            }
        }
        ensure!(
            matches!(
                self.redistribution.as_str(),
                "allowed" | "restricted" | "unknown"
            ),
            "[invalid-input] ink redistribution must be allowed, restricted or unknown"
        );
        Ok(())
    }

    /// A process preview alternate at a tint. This preserves the ink identity
    /// and reports the alternate separately; it does not claim plate values.
    pub fn spot_paint(&self, tint: f64, alpha: f64) -> Result<Value> {
        self.validate()?;
        ensure!(
            tint.is_finite() && (0.0..=1.0).contains(&tint),
            "[invalid-input] spot tint must be finite and in [0,1]"
        );
        ensure!(
            alpha.is_finite() && (0.0..=1.0).contains(&alpha),
            "[invalid-input] spot alpha must be finite and in [0,1]"
        );
        Ok(json!({"space":"spot","ink":self.id,"tint":tint,"alpha":alpha}))
    }
}

impl Library {
    pub fn from_value(value: &Value) -> Result<Self> {
        let mut library: Self = serde_json::from_value(value.clone())
            .context("[malformed-resource] invalid spot ink library")?;
        for ink in library.inks.values_mut() {
            ink.aliases.sort_by_key(|alias| alias_key(alias));
        }
        library.validate()?;
        Ok(library)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema == 1,
            "[unsupported-capability] unsupported spot ink library schema"
        );
        ensure!(
            self.inks.len() <= MAX_INKS,
            "[limit-exceeded] spot ink library exceeds {MAX_INKS} inks"
        );
        let mut aliases = BTreeMap::<String, String>::new();
        let mut alias_count = 0usize;
        for (id, ink) in &self.inks {
            ensure!(
                id == &ink.id,
                "[malformed-resource] spot ink map key must equal stable ink ID"
            );
            ink.validate().with_context(|| format!("spot ink {id}"))?;
            let canonical = alias_key(&ink.name);
            ensure!(
                aliases.insert(canonical, id.clone()).is_none(),
                "[conflict] duplicate canonical spot ink name"
            );
            for alias in &ink.aliases {
                alias_count += 1;
                ensure!(
                    aliases.insert(alias_key(alias), id.clone()).is_none(),
                    "[conflict] duplicate spot ink alias {alias}"
                );
            }
            ensure!(
                aliases.insert(alias_key(id), id.clone()).is_none(),
                "[conflict] spot ink ID conflicts with a name or alias: {id}"
            );
        }
        ensure!(
            alias_count <= MAX_ALIASES,
            "[limit-exceeded] spot ink library exceeds {MAX_ALIASES} aliases"
        );
        let bytes = serde_json::to_vec(self)?;
        ensure!(
            bytes.len() <= MAX_LIBRARY_BYTES,
            "[limit-exceeded] spot ink library exceeds {MAX_LIBRARY_BYTES} serialized bytes"
        );
        Ok(())
    }

    /// Add a definition. Re-importing the identical stable ID is idempotent;
    /// changing a definition or taking another ink's alias is a hard conflict.
    pub fn insert(&mut self, mut ink: Ink) -> Result<bool> {
        ink.aliases.sort_by_key(|alias| alias_key(alias));
        ink.validate()?;
        if let Some(existing) = self.inks.get(&ink.id) {
            ensure!(existing == &ink, "[conflict] spot ink ID {} already has a different definition; resolve it explicitly", ink.id);
            return Ok(false);
        }
        for existing in self.inks.values() {
            let mut occupied = vec![existing.name.as_str(), existing.id.as_str()];
            occupied.extend(existing.aliases.iter().map(String::as_str));
            let incoming = std::iter::once(ink.name.as_str())
                .chain(std::iter::once(ink.id.as_str()))
                .chain(ink.aliases.iter().map(String::as_str));
            for new_name in incoming {
                ensure!(
                    !occupied
                        .iter()
                        .any(|old| alias_key(old) == alias_key(new_name)),
                    "[conflict] spot ink name or alias {new_name} is already assigned to {}",
                    existing.id
                );
            }
        }
        ensure!(
            self.inks.len() < MAX_INKS,
            "[limit-exceeded] spot ink library exceeds {MAX_INKS} inks"
        );
        let mut candidate = self.clone();
        candidate.inks.insert(ink.id.clone(), ink);
        candidate.validate()?;
        *self = candidate;
        Ok(true)
    }

    pub fn resolve(&self, name_or_id: &str) -> Result<&Ink> {
        bounded_text(name_or_id, "ink reference", MAX_NAME_BYTES, true)?;
        let key = alias_key(name_or_id);
        self.inks
            .values()
            .find(|ink| {
                alias_key(&ink.id) == key
                    || alias_key(&ink.name) == key
                    || ink.aliases.iter().any(|alias| alias_key(alias) == key)
            })
            .context("[missing-resource] spot ink reference was not found")
    }

    /// Canonical, bounded bytes suitable for embedding in an offline package.
    /// The returned digest pins the exact library snapshot.
    pub fn package_snapshot(&self) -> Result<(String, Vec<u8>)> {
        self.validate()?;
        let mut canonical = self.clone();
        for ink in canonical.inks.values_mut() {
            ink.aliases.sort_by_key(|alias| alias_key(alias));
        }
        let bytes = serde_json::to_vec(&canonical)?;
        ensure!(
            bytes.len() <= MAX_LIBRARY_BYTES,
            "[limit-exceeded] spot ink library exceeds package limit"
        );
        Ok((crate::resource::sha256(&bytes), bytes))
    }

    pub fn verify_package_snapshot(bytes: &[u8], expected_digest: &str) -> Result<Self> {
        ensure!(
            bytes.len() <= MAX_LIBRARY_BYTES,
            "[limit-exceeded] packaged spot ink library exceeds limit"
        );
        crate::resource::verify(bytes, expected_digest)
            .context("packaged spot ink library hash mismatch")?;
        let value: Value = serde_json::from_slice(bytes)
            .context("[malformed-resource] packaged spot ink library is not JSON")?;
        Self::from_value(&value)
    }
}

/// Return the optional v8 ink library as an empty library when absent. This
/// keeps existing v8 documents valid while giving package/catalog callers one
/// strict parser for the new optional dependency.
pub fn document_library(raw: &Value) -> Result<Library> {
    if !crate::color::has_v8(raw) {
        return Ok(Library::default());
    }
    let Some(value) = raw.get("color_management").and_then(|v| v.get("inks")) else {
        return Ok(Library::default());
    };
    Library::from_value(value)
}

/// Deterministic manifest audit table. Definitions remain in the packaged
/// document; the table records and verifies exactly what was collected.
pub fn package_table(raw: &Value) -> Result<Vec<Value>> {
    let library = document_library(raw)?;
    library.validate()?;
    ensure!(
        library
            .inks
            .values()
            .all(|ink| ink.redistribution == "allowed"),
        "[redistribution-restricted] packaged spot inks require declared allowed redistribution"
    );
    let table: Vec<Value> = library
        .inks
        .values()
        .map(|ink| {
            let definition = serde_json::to_vec(ink)?;
            Ok::<_, anyhow::Error>(json!({
                "ink": ink,
                "definition_sha256": crate::resource::sha256(&definition)
            }))
        })
        .collect::<Result<Vec<_>>>()?;
    validate_package_table(&table)?;
    Ok(table)
}

/// Check the package manifest's table independently, with the same limits as
/// document libraries, before it is compared with the validated asset.
pub fn validate_package_table(table: &[Value]) -> Result<()> {
    ensure!(
        table.len() <= MAX_INKS,
        "[limit-exceeded] package spot ink table exceeds {MAX_INKS} inks"
    );
    let bytes = serde_json::to_vec(table)?;
    ensure!(
        bytes.len() <= MAX_LIBRARY_BYTES,
        "[limit-exceeded] package spot ink table exceeds {MAX_LIBRARY_BYTES} bytes"
    );
    let mut library = Library::default();
    for row in table {
        let fields = row
            .as_object()
            .context("[malformed-resource] package spot ink row must be an object")?;
        ensure!(
            fields.len() == 2
                && fields.contains_key("ink")
                && fields.contains_key("definition_sha256"),
            "[malformed-resource] package spot ink row has unsupported fields"
        );
        let ink: Ink = serde_json::from_value(fields["ink"].clone())
            .context("[malformed-resource] invalid packaged spot ink")?;
        let expected = fields["definition_sha256"]
            .as_str()
            .context("[malformed-resource] packaged spot ink digest must be text")?;
        let actual = crate::resource::sha256(&serde_json::to_vec(&ink)?);
        ensure!(
            actual == expected,
            "[malformed-resource] packaged spot ink definition hash mismatch"
        );
        ensure!(ink.redistribution == "allowed", "[redistribution-restricted] package contains spot ink {} without allowed redistribution", ink.id);
        library.insert(ink)?;
    }
    library.validate()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ink(id: &str, name: &str, aliases: &[&str]) -> Ink {
        Ink {
            id: id.into(),
            name: name.into(),
            aliases: aliases.iter().map(|s| (*s).into()).collect(),
            alternate: json!({"space":"srgb","channels":[0.9,0.1,0.2]}),
            provenance: Provenance {
                source: "user-supplied-library".into(),
                publisher: None,
                revision: None,
                license: Some("proprietary".into()),
            },
            redistribution: "restricted".into(),
        }
    }

    #[test]
    fn stable_identity_alias_resolution_and_tint_are_deterministic() {
        let mut library = Library::default();
        assert!(library
            .insert(ink("pantone-185-c", "Red 185", &["brand-red"]))
            .unwrap());
        assert_eq!(library.resolve("BRAND-RED").unwrap().id, "pantone-185-c");
        assert_eq!(library.resolve("red 185").unwrap().id, "pantone-185-c");
        assert_eq!(
            library
                .resolve("pantone-185-c")
                .unwrap()
                .spot_paint(0.25, 0.8)
                .unwrap(),
            json!({"space":"spot","ink":"pantone-185-c","tint":0.25,"alpha":0.8})
        );
        assert!(!library
            .insert(ink("pantone-185-c", "Red 185", &["brand-red"]))
            .unwrap());
        assert!(library.resolve("red").is_err());
    }

    #[test]
    fn conflicting_ids_aliases_and_case_variants_fail_closed() {
        let mut library = Library::default();
        library.insert(ink("ink-a", "Ink A", &["warm"])).unwrap();
        assert!(library
            .insert(ink("ink-a", "Different", &["other"]))
            .is_err());
        assert!(library.insert(ink("ink-b", "Ink B", &["WARM"])).is_err());
        assert!(library.insert(ink("ink-c", "ink a", &[])).is_err());
    }

    #[test]
    fn malformed_or_unlicensed_metadata_is_never_inferred_or_accepted() {
        let mut value = serde_json::to_value(ink("ink-a", "Ink A", &[])).unwrap();
        value["redistribution"] = json!("allowed-by-name");
        assert!(serde_json::from_value::<Ink>(value.clone())
            .unwrap()
            .validate()
            .is_err());
        value["redistribution"] = json!("unknown");
        value["alternate"] = json!({"space":"spot","ink":"ink-b","tint":1.0});
        assert!(serde_json::from_value::<Ink>(value)
            .unwrap()
            .validate()
            .is_err());
    }

    #[test]
    fn package_snapshot_is_content_addressed_and_verified_offline() {
        let mut library = Library::default();
        library.insert(ink("ink-a", "Ink A", &["a"])).unwrap();
        let (digest, bytes) = library.package_snapshot().unwrap();
        assert_eq!(
            Library::verify_package_snapshot(&bytes, &digest).unwrap(),
            library
        );
        assert!(Library::verify_package_snapshot(
            &bytes,
            "sha256:0000000000000000000000000000000000000000000000000000000000000000"
        )
        .is_err());
    }

    #[test]
    fn tint_and_alpha_are_explicit_and_bounded() {
        let ink = ink("ink-a", "Ink A", &[]);
        assert!(ink.spot_paint(-0.1, 1.0).is_err());
        assert!(ink.spot_paint(0.5, f64::NAN).is_err());
    }
}
