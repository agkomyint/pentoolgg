//! Explicit document role targets. Input interpretations remain attached to
//! native samples; changing a role never reinterprets those stored channels.
use super::{
    catalog,
    icc::{Direction, Intent, Profile, Transform},
    value::Destination,
};
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};
use std::{path::Path, str::FromStr};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Working,
    Proof,
    Display,
    Output,
}
pub const ALL: [Role; 4] = [Role::Working, Role::Proof, Role::Display, Role::Output];
impl Role {
    pub fn name(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Proof => "proof",
            Self::Display => "display",
            Self::Output => "output",
        }
    }
    fn directions(self) -> &'static [Direction] {
        match self {
            Self::Working | Self::Proof => &[Direction::DeviceToPcs, Direction::PcsToDevice],
            Self::Display | Self::Output => &[Direction::PcsToDevice],
        }
    }
}
impl FromStr for Role {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> Result<Self> {
        match value {
            "working" => Ok(Self::Working),
            "proof" => Ok(Self::Proof),
            "display" => Ok(Self::Display),
            "output" => Ok(Self::Output),
            _ => bail!("[invalid-input] color role must be working, proof, display or output"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Setting {
    pub destination: Destination,
    pub intent: Intent,
}
impl Setting {
    fn parse(value: &Value) -> Result<Self> {
        let object = value
            .as_object()
            .context("[invalid-input] color role setting must be an object")?;
        ensure!(
            object.len() == 2
                && object.contains_key("destination")
                && object.contains_key("intent"),
            "[invalid-input] color role setting requires only destination and intent"
        );
        Ok(Self {
            destination: Destination::parse(&value["destination"])?,
            intent: value["intent"]
                .as_str()
                .context("[invalid-input] color role intent must be text")?
                .parse()?,
        })
    }
    pub fn json(&self) -> Value {
        json!({"destination":self.destination.json(),"intent":self.intent})
    }
}

fn check(catalog: &Value, role: Role, setting: &Setting) -> Result<()> {
    let destination = &setting.destination;
    if role == Role::Display {
        ensure!(
            destination.space() != "lab-d50"
                && (!destination.space().starts_with("icc-") || destination.space() == "icc-rgb"),
            "[unsupported-capability] display role requires named RGB or an RGB monitor profile"
        );
    }
    if role == Role::Proof {
        ensure!(
            destination.profile().is_some(),
            "[unsupported-capability] proof role requires an explicit ICC printing condition"
        );
    }
    if role == Role::Output {
        ensure!(
            destination.space() != "lab-d50",
            "[unsupported-capability] output role requires named RGB or an ICC device profile"
        );
    }
    if let Some(reference) = destination.profile() {
        let (_, record) = catalog::resolve_in(catalog, reference)?;
        ensure!(
            record["color_space"].as_str() == destination.signature(),
            "[invalid-input] {} role profile {reference} channel space disagrees with {}",
            role.name(),
            destination.space()
        );
        ensure!(record["class"]!="link","[unsupported-capability] {} role cannot use a device-link as an input/output interpretation",role.name());
        if matches!(role, Role::Proof | Role::Output) {
            ensure!(
                record["class"] == "prtr",
                "[unsupported-capability] {} role requires an ICC output-class printing profile",
                role.name()
            );
        }
        if role == Role::Display {
            ensure!(
                record["class"] == "mntr",
                "[unsupported-capability] display role requires an ICC monitor-class RGB profile"
            );
        }
    }
    Ok(())
}

/// Structural validation stays offline: external original bytes are opened only
/// by role assignment or a consuming operation, using the shared loader.
pub(crate) fn validate_in(catalog: &Value) -> Result<()> {
    if let Some(roles) = catalog.get("roles") {
        let roles = roles
            .as_object()
            .context("[invalid-input] color roles must be an object")?;
        ensure!(
            roles.len() <= 4,
            "[limit-exceeded] color roles can contain only four explicit roles"
        );
        for (name, value) in roles {
            let role: Role = name.parse()?;
            check(catalog, role, &Setting::parse(value)?)
                .with_context(|| format!("color role {name}"))?;
        }
    }
    Ok(())
}

fn require(raw: &Value) -> Result<()> {
    crate::scene::validate(raw)?;
    ensure!(
        crate::color::has_v8(raw),
        "[unsupported-capability] color roles require explicit migration to v8"
    );
    Ok(())
}

pub fn effective(raw: &Value, role: Role) -> Result<Setting> {
    require(raw)?;
    effective_validated(raw, role)
}
fn effective_validated(raw: &Value, role: Role) -> Result<Setting> {
    if let Some(value) = raw["color_management"]["roles"].get(role.name()) {
        return Setting::parse(value);
    }
    let space=match role { Role::Working=>"srgb:linear",Role::Display=>"srgb",_=>bail!("[missing-resource] {} color role is not configured; use `color role assign` with a verified destination",role.name()) };
    Ok(Setting {
        destination: Destination::parse(&json!({"space":space}))?,
        intent: Intent::RelativeColorimetric,
    })
}

pub fn list(raw: &Value) -> Result<Value> {
    require(raw)?;
    let mut roles = Vec::new();
    for role in ALL {
        let configured = raw["color_management"]["roles"].get(role.name()).is_some();
        let setting = if configured || matches!(role, Role::Working | Role::Display) {
            Some(effective_validated(raw, role)?)
        } else {
            None
        };
        let profile = setting
            .as_ref()
            .and_then(|value| value.destination.profile())
            .map(|reference| {
                catalog::resolve_in(&raw["color_management"], reference)
                    .map(|(identity, _)| identity)
            })
            .transpose()?;
        roles.push(json!({"role":role.name(),"configured":configured,"effective":setting.map(|value|value.json()),"profile":profile}));
    }
    Ok(
        json!({"version":1,"scope":"document-color-roles","matches":4,"returned":4,"offset":0,"limit":4,"has_more":false,"roles":roles}),
    )
}

pub fn assign(
    raw: &mut Value,
    document: &Path,
    role: Role,
    destination: &Value,
    intent: Intent,
) -> Result<Value> {
    require(raw)?;
    let mut setting = Setting {
        destination: Destination::parse(destination)?,
        intent,
    };
    check(&raw["color_management"], role, &setting)?;
    let mut paths = Vec::new();
    if let Some(reference) = setting.destination.profile() {
        let (identity, bytes) = catalog::load(raw, document, reference)?;
        paths = compiled_paths(&bytes, role, intent, &identity)?;
        let mut destination = setting.destination.json();
        destination["profile"] = json!(identity);
        setting.destination = Destination::parse(&destination)?;
    } else {
        for direction in role.directions() {
            paths.push(json!({"direction":direction,"path":if setting.destination.space()=="lab-d50" {"physical-d50-lab"}else{"named-rgb-matrix-trc"},"intent":intent,"colorimetric_equivalent":true}));
        }
    }
    let previous = raw["color_management"]["roles"].get(role.name()).cloned();
    let mut planned = raw.clone();
    planned["color_management"]
        .as_object_mut()
        .unwrap()
        .entry("roles")
        .or_insert(json!({}));
    planned["color_management"]["roles"][role.name()] = setting.json();
    crate::scene::validate(&planned)?;
    catalog::size_gate(&planned)?;
    *raw = planned;
    Ok(
        json!({"version":1,"role":role.name(),"setting":setting.json(),"changed":previous.as_ref()!=Some(&setting.json()),"previous":previous,"validated_paths":paths,"paint_samples_changed":false}),
    )
}

fn compiled_paths(bytes: &[u8], role: Role, intent: Intent, identity: &str) -> Result<Vec<Value>> {
    let profile = Profile::parse(bytes)?;
    role.directions()
        .iter()
        .map(|&direction| {
            let transform = Transform::compile(&profile, direction, intent)
                .with_context(|| format!("{} role, ICC {identity}, {direction:?}", role.name()))?;
            Ok(serde_json::to_value(transform.description())?)
        })
        .collect()
}

/// Package caller has already validated the document and required embedded,
/// redistributable original ICC bytes. Compile each explicitly required path,
/// including settings written directly in JSON rather than assigned by the CLI.
pub(crate) fn validate_embedded_paths(raw: &Value) -> Result<()> {
    if let Some(roles) = raw["color_management"]["roles"].as_object() {
        for (name, value) in roles {
            let role: Role = name.parse()?;
            let setting = Setting::parse(value)?;
            if let Some(reference) = setting.destination.profile() {
                let (identity, record) = catalog::resolve_in(&raw["color_management"], reference)?;
                compiled_paths(&catalog::embedded(record)?, role, setting.intent, identity)?;
            }
        }
    }
    Ok(())
}

pub fn clear(raw: &mut Value, role: Role) -> Result<Value> {
    require(raw)?;
    let previous = raw["color_management"]["roles"]
        .get(role.name())
        .with_context(|| {
            format!(
                "[missing-resource] {} color role has no explicit setting to clear",
                role.name()
            )
        })?
        .clone();
    let mut planned = raw.clone();
    let roles = planned["color_management"]["roles"]
        .as_object_mut()
        .unwrap();
    roles.remove(role.name());
    if roles.is_empty() {
        planned["color_management"]
            .as_object_mut()
            .unwrap()
            .remove("roles");
    }
    crate::scene::validate(&planned)?;
    catalog::size_gate(&planned)?;
    *raw = planned;
    Ok(
        json!({"version":1,"role":role.name(),"cleared":true,"previous":previous,"paint_samples_changed":false}),
    )
}

pub fn evaluate(raw: &Value, document: &Path, role: Role, color: &Value) -> Result<Value> {
    let setting = effective(raw, role)?;
    let mut result = super::value::evaluate(
        raw,
        document,
        color,
        Some(&setting.destination.json()),
        setting.intent,
    )?;
    result["role"] = json!(role.name());
    result["scope"] = json!("native-sample-role-conversion");
    Ok(result)
}

pub(crate) fn reject_removal(raw: &Value, identity: &str) -> Result<()> {
    if let Some(roles) = raw["color_management"]["roles"].as_object() {
        for (name, value) in roles {
            if let Some(reference) = Setting::parse(value)?.destination.profile() {
                let (used, _) = catalog::resolve_in(&raw["color_management"], reference)?;
                ensure!(used!=identity,"[resource-in-use] ICC {identity} is assigned to the {name} role; clear or reassign that role before removing the profile");
            }
        }
    }
    Ok(())
}
