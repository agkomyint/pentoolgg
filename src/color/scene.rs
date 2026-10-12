//! Native paint slots shared by validation, preview, and production planning.
//! Traversal follows supported scene fields; arbitrary document extensions and
//! original profile/image bytes are never interpreted as paint declarations.
use super::{
    catalog,
    icc::Intent,
    value::{Destination, Evaluator, NativeColor, ProfileSet},
};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

pub const MAX_NATIVE_SLOTS: usize = 65_536;
pub const MAX_COLOR_POINTER_BYTES: usize = 4096;
fn label(value: &Value) -> String {
    short(value.as_str().unwrap_or("?"))
}
fn short(text: &str) -> String {
    let mut short: String = text.chars().take(128).collect();
    if text.chars().nth(128).is_some() {
        short.push('…');
    }
    short
}

#[derive(Debug)]
pub struct ColorSlot {
    pub pointer: String,
    pub owner: String,
    pub color: NativeColor,
    pub locked: bool,
    pub shared: bool,
    pub style: Option<String>,
    pub resource: Option<String>,
}
#[derive(Debug)]
pub struct DestinationSlot {
    pub pointer: String,
    pub owner: String,
    pub destination: Destination,
    pub intent: Intent,
}
struct Collected {
    slots: Vec<ColorSlot>,
    destinations: Vec<DestinationSlot>,
    locked_styles: BTreeSet<String>,
}
#[derive(Clone, Copy)]
pub enum Scope<'a> {
    Document,
    Page {
        page: Option<&'a str>,
        visible_only: bool,
    },
    Node {
        page: Option<&'a str>,
        id: &'a str,
    },
    Mask {
        page: Option<&'a str>,
        name: &'a str,
    },
}
fn key(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}
struct Visitor<'a> {
    raw: &'a Value,
    slots: Vec<ColorSlot>,
    destinations: Vec<DestinationSlot>,
    styles: Vec<String>,
    seen: BTreeSet<String>,
    visible_only: bool,
    legacy: bool,
    locked: bool,
    shared: bool,
    current_style: Option<String>,
    references: Vec<(Option<String>, String, bool)>,
    required_styles: BTreeSet<String>,
    resources: BTreeMap<String, bool>,
    current_resource: Option<String>,
    target: Option<&'a str>,
    target_active: bool,
    target_found: bool,
    gradient_stops: usize,
}
impl Visitor<'_> {
    fn reference(&mut self, reference: &str) {
        self.required_styles.insert(reference.into());
        self.styles.push(reference.into());
        self.references
            .push((self.current_style.clone(), reference.into(), self.locked));
    }
    fn add(&mut self, color: NativeColor, pointer: &str, owner: &str) -> Result<()> {
        ensure!(
            pointer.len() <= MAX_COLOR_POINTER_BYTES,
            "[limit-exceeded] native color path exceeds {MAX_COLOR_POINTER_BYTES} bytes"
        );
        ensure!(
            self.slots.len() + self.destinations.len() < MAX_NATIVE_SLOTS,
            "[limit-exceeded] native color traversal exceeds {MAX_NATIVE_SLOTS} slots"
        );
        self.slots.push(ColorSlot {
            pointer: pointer.into(),
            owner: owner.into(),
            color,
            locked: self.locked,
            shared: self.shared,
            style: self.current_style.clone(),
            resource: self.current_resource.clone(),
        });
        Ok(())
    }
    fn paint(&mut self, value: &Value, pointer: &str, owner: &str, depth: usize) -> Result<()> {
        ensure!(
            depth <= crate::scene::MAX_DEPTH,
            "[limit-exceeded] {owner}: color reference nesting exceeds 64"
        );
        if value.get("space").is_some() {
            let color =
                NativeColor::parse(value).with_context(|| format!("{owner}, color {pointer}"))?;
            self.add(color, pointer, owner)?;
            return Ok(());
        }
        if let Some(reference) = value.get("ref").and_then(Value::as_str) {
            self.reference(reference);
        }
        if let Some(fallback) = value.get("fallback") {
            self.paint(fallback, &format!("{pointer}/fallback"), owner, depth + 1)?;
        } else if self.legacy && (value.is_string() || value.is_array()) {
            if let Some(color) = NativeColor::from_legacy(value)
                .with_context(|| format!("{owner}, color {pointer}"))?
            {
                self.add(color, pointer, owner)?;
            }
        }
        Ok(())
    }
    fn style(&mut self, value: &Value, pointer: &str, owner: &str) -> Result<()> {
        for slot in ["fill", "stroke"] {
            if let Some(value) = value.get(slot) {
                self.paint(value, &format!("{pointer}/{slot}"), owner, 0)?;
            }
        }
        Ok(())
    }
    fn fill(&mut self, value: &Value, pointer: &str, owner: &str, depth: usize) -> Result<()> {
        ensure!(
            depth <= crate::scene::MAX_DEPTH,
            "[limit-exceeded] {owner}: fill reference nesting exceeds 64"
        );
        if let Some(reference) = value.get("ref").and_then(Value::as_str) {
            self.reference(reference);
        }
        if let Some(fallback) = value.get("fallback") {
            self.fill(fallback, &format!("{pointer}/fallback"), owner, depth + 1)?;
        }
        match value["kind"].as_str() {
            Some("solid") => self.paint(&value["color"], &format!("{pointer}/color"), owner, 0)?,
            Some("linear" | "radial" | "conic") => {
                if let Some((destination, intent)) = super::gradient::setting(self.raw, value)
                    .with_context(|| format!("{owner}, gradient {pointer}"))?
                {
                    ensure!(
                        self.slots.len() + self.destinations.len() < MAX_NATIVE_SLOTS,
                        "[limit-exceeded] native color traversal exceeds {MAX_NATIVE_SLOTS} slots"
                    );
                    ensure!(pointer.len()+6<=MAX_COLOR_POINTER_BYTES,"[limit-exceeded] native gradient path exceeds {MAX_COLOR_POINTER_BYTES} bytes");
                    self.destinations.push(DestinationSlot {
                        pointer: format!("{pointer}/space"),
                        owner: owner.into(),
                        destination,
                        intent,
                    });
                    self.gradient_stops += value["stops"].as_array().map_or(0, Vec::len);
                    ensure!(
                        self.gradient_stops <= MAX_NATIVE_SLOTS,
                        "[limit-exceeded] native gradients exceed {MAX_NATIVE_SLOTS} total stops"
                    );
                }
                if let Some(stops) = value["stops"].as_array() {
                    for (i, stop) in stops.iter().enumerate() {
                        self.paint(
                            &stop["color"],
                            &format!("{pointer}/stops/{i}/color"),
                            owner,
                            0,
                        )?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
    fn effects(&mut self, value: &Value, pointer: &str, owner: &str, depth: usize) -> Result<()> {
        ensure!(
            depth <= crate::scene::MAX_DEPTH,
            "[limit-exceeded] {owner}: effect reference nesting exceeds 64"
        );
        if let Some(reference) = value.get("ref").and_then(Value::as_str) {
            self.reference(reference);
        }
        if let Some(fallback) = value.get("fallback") {
            self.effects(fallback, &format!("{pointer}/fallback"), owner, depth + 1)?;
        }
        if let Some(effects) = value.as_array() {
            for (i, effect) in effects.iter().enumerate() {
                if self.visible_only && effect["enabled"] == false {
                    continue;
                }
                let pointer = format!("{pointer}/{i}/params");
                match effect["kind"].as_str() {
                    Some(
                        "drop-shadow" | "inner-shadow" | "outer-glow" | "inner-glow" | "stroke"
                        | "color-overlay",
                    ) => self.paint(
                        &effect["params"]["color"],
                        &format!("{pointer}/color"),
                        owner,
                        0,
                    )?,
                    Some("gradient-overlay") => self.fill(
                        &effect["params"]["fill"],
                        &format!("{pointer}/fill"),
                        owner,
                        0,
                    )?,
                    _ => {}
                }
            }
        }
        Ok(())
    }
    fn node(&mut self, value: &Value, pointer: &str, context: &str, depth: usize) -> Result<()> {
        ensure!(
            depth <= crate::scene::MAX_DEPTH,
            "[limit-exceeded] {context}: native paint node nesting exceeds 64"
        );
        if self.visible_only && value["visible"] == false {
            return Ok(());
        }
        if let Some(target) = self.target.filter(|_| !self.target_active) {
            if self.target_found {
                return Ok(());
            }
            if value["id"] != target {
                let inherited = self.locked;
                self.locked |= value["locked"] == true;
                if let Some(children) = value["children"].as_array() {
                    for (i, child) in children.iter().enumerate() {
                        self.node(
                            child,
                            &format!("{pointer}/children/{i}"),
                            context,
                            depth + 1,
                        )?;
                    }
                }
                self.locked = inherited;
                return Ok(());
            }
            self.target_found = true;
        }
        let target_active = self.target_active;
        self.target_active = true;
        let inherited = self.locked;
        self.locked |= value["locked"] == true;
        let owner = format!("{context}, object {}", label(&value["id"]));
        self.style(&value["style"], &format!("{pointer}/style"), &owner)?;
        self.effects(&value["effects"], &format!("{pointer}/effects"), &owner, 0)?;
        if let Some(resource) = value["mask"]["resource"].as_str() {
            *self.resources.entry(resource.into()).or_default() |= self.locked;
        }
        if value["kind"] == "fill" {
            self.fill(&value["fill"], &format!("{pointer}/fill"), &owner, 0)?;
        }
        if let Some(children) = value["children"].as_array() {
            for (i, child) in children.iter().enumerate() {
                self.node(
                    child,
                    &format!("{pointer}/children/{i}"),
                    context,
                    depth + 1,
                )?;
            }
        }
        if value["kind"] == "instance" {
            self.node(
                &value["fallback"],
                &format!("{pointer}/fallback"),
                context,
                depth + 1,
            )?;
        }
        if value["mask"]["kind"] == "vector" {
            self.node(
                &value["mask"]["node"],
                &format!("{pointer}/mask/node"),
                context,
                depth + 1,
            )?;
        }
        self.locked = inherited;
        self.target_active = target_active;
        Ok(())
    }
    fn token(&mut self, name: &str) -> Result<()> {
        if !self.seen.insert(name.into()) {
            return Ok(());
        }
        let raw = self.raw;
        self.current_style = Some(name.into());
        self.shared = true;
        self.locked = false;
        let token = &raw["styles"][name];
        let value = &token["value"];
        let pointer = format!("/styles/{}/value", key(name));
        let owner = format!("shared style {}", short(name));
        if let Some(reference) = value.get("ref").and_then(Value::as_str) {
            self.reference(reference);
        }
        match token["type"].as_str() {
            Some("color") => self.paint(value, &pointer, &owner, 0)?,
            Some("fill") => self.fill(value, &pointer, &owner, 0)?,
            Some("effects") => self.effects(value, &pointer, &owner, 0)?,
            Some("stroke" | "typography") => {
                if self.required_styles.contains(name)
                    && (value.is_string() || value.is_array() || value.get("space").is_some())
                {
                    self.paint(value, &pointer, &owner, 0)?;
                }
                self.style(value, &pointer, &owner)?;
                if let Some(color) = value.get("color") {
                    self.paint(color, &format!("{pointer}/color"), &owner, 0)?;
                }
            }
            Some("shadow") => {
                self.paint(&value["color"], &format!("{pointer}/color"), &owner, 0)?
            }
            Some("appearance") => {
                self.style(&value["style"], &format!("{pointer}/style"), &owner)?;
                self.effects(&value["effects"], &format!("{pointer}/effects"), &owner, 0)?;
                self.fill(&value["fill"], &format!("{pointer}/fill"), &owner, 0)?;
            }
            _ => {
                ensure!(!self.legacy || !self.required_styles.contains(name) || token.is_null(), "[unsupported-capability] paint assignment/conversion requires a supported color-bearing style type for {name}");
            }
        }
        Ok(())
    }
}

pub fn slots(raw: &Value, scope: Scope<'_>) -> Result<Vec<ColorSlot>> {
    Ok(collect(raw, scope, false)?.slots)
}
pub fn paints(raw: &Value, scope: Scope<'_>) -> Result<Vec<ColorSlot>> {
    Ok(collect(raw, scope, true)?.slots)
}
pub fn destinations(raw: &Value, scope: Scope<'_>) -> Result<Vec<DestinationSlot>> {
    Ok(collect(raw, scope, false)?.destinations)
}
pub(crate) fn locked_styles(raw: &Value) -> Result<BTreeSet<String>> {
    Ok(collect(raw, Scope::Document, false)?.locked_styles)
}
pub(crate) fn locked_resources(raw: &Value) -> Result<BTreeSet<String>> {
    Ok(paints(raw, Scope::Document)?
        .into_iter()
        .filter(|slot| slot.locked)
        .filter_map(|slot| slot.resource)
        .collect())
}
fn collect(raw: &Value, scope: Scope<'_>, legacy: bool) -> Result<Collected> {
    let mut visitor = Visitor {
        raw,
        slots: Vec::new(),
        destinations: Vec::new(),
        styles: Vec::new(),
        seen: BTreeSet::new(),
        visible_only: false,
        legacy,
        locked: false,
        shared: false,
        current_style: None,
        references: Vec::new(),
        required_styles: BTreeSet::new(),
        resources: BTreeMap::new(),
        current_resource: None,
        target: None,
        target_active: false,
        target_found: false,
        gradient_stops: 0,
    };
    let pages = raw["pages"].as_array().context("document has no pages")?;
    let selected = match scope {
        Scope::Document => None,
        Scope::Page { page, .. } | Scope::Node { page, .. } | Scope::Mask { page, .. } => {
            if let Scope::Page { visible_only, .. } = scope {
                visitor.visible_only = visible_only;
            }
            if let Scope::Node { id, .. } = scope {
                visitor.target = Some(id);
            }
            if let Scope::Mask { name, .. } = scope {
                let digest = raw["masks"][name]
                    .as_str()
                    .or_else(|| raw["mask_resources"].get(name).map(|_| name))
                    .with_context(|| format!("mask not found: {name}"))?;
                visitor.resources.insert(digest.into(), false);
            }
            Some(match page {
                Some(id) => pages
                    .iter()
                    .position(|p| p["id"] == id)
                    .with_context(|| format!("page not found: {id}"))?,
                None => 0,
            })
        }
    };
    for (p, page) in pages.iter().enumerate() {
        if selected.is_some_and(|selected| selected != p) {
            continue;
        }
        let owner = format!("page {}", label(&page["id"]));
        visitor.locked = false;
        if matches!(scope, Scope::Page { .. } | Scope::Document) {
            visitor.paint(
                &page["canvas"]["background"],
                &format!("/pages/{p}/canvas/background"),
                &owner,
                0,
            )?;
        }
        if matches!(scope, Scope::Mask { .. }) {
            continue;
        }
        if let Some(layers) = page["layers"].as_array() {
            for (l, layer) in layers.iter().enumerate() {
                if visitor.visible_only && layer["visible"] == false {
                    continue;
                }
                visitor.locked = layer["locked"] == true;
                let owner = format!("{owner}, layer {}", label(&layer["id"]));
                if let Some(nodes) = layer["nodes"].as_array() {
                    for (n, node) in nodes.iter().enumerate() {
                        visitor.node(
                            node,
                            &format!("/pages/{p}/layers/{l}/nodes/{n}"),
                            &owner,
                            0,
                        )?;
                    }
                }
            }
        }
    }
    ensure!(
        visitor.target.is_none() || visitor.target_found,
        "[invalid-input] node not found: {}",
        visitor.target.unwrap_or("?")
    );
    // Resource snapshots are dependencies, not a second search for the target.
    visitor.target = None;
    if selected.is_none() {
        visitor.locked = false;
        visitor.shared = true;
        if let Some(styles) = raw["styles"].as_object() {
            for name in styles.keys() {
                visitor.styles.push(name.clone());
            }
        }
        if let Some(components) = raw["components"].as_array() {
            for (i, component) in components.iter().enumerate() {
                if let Some(root) = component.get("root") {
                    visitor.node(
                        root,
                        &format!("/components/{i}/root"),
                        &format!("component {}", label(&component["id"])),
                        0,
                    )?;
                }
            }
        }
        if let Some(resources) = raw["mask_resources"].as_object() {
            for digest in resources.keys() {
                visitor.resources.entry(digest.clone()).or_default();
            }
        }
    }
    for (digest, locked) in std::mem::take(&mut visitor.resources) {
        let resource = &raw["mask_resources"][&digest];
        if resource["kind"] == "vector" {
            visitor.shared = true;
            visitor.locked = locked;
            visitor.current_resource = Some(digest.clone());
            visitor.node(
                &resource["node"],
                &format!("/mask_resources/{}/node", key(&digest)),
                &format!("vector mask {}", short(&digest)),
                0,
            )?;
        }
    }
    visitor.current_resource = None;
    while let Some(name) = visitor.styles.pop() {
        visitor.token(&name)?;
    }
    if legacy {
        for name in &visitor.required_styles {
            let token = &raw["styles"][name];
            ensure!(token.is_null() || matches!(token["type"].as_str(),Some("color"|"fill"|"effects"|"stroke"|"typography"|"shadow"|"appearance")), "[unsupported-capability] paint assignment/conversion requires a supported color-bearing style type for {name}");
        }
    }
    let mut edges: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut queue = Vec::new();
    for (source, target, locked) in visitor.references {
        if locked {
            queue.push(target.clone());
        }
        if let Some(source) = source {
            edges.entry(source).or_default().push(target);
        }
    }
    let mut locked_styles = BTreeSet::new();
    while let Some(name) = queue.pop() {
        if !locked_styles.insert(name.clone()) {
            continue;
        }
        if let Some(targets) = edges.get(&name) {
            queue.extend(targets.iter().cloned());
        }
    }
    for slot in &mut visitor.slots {
        slot.locked |= slot
            .style
            .as_ref()
            .is_some_and(|style| locked_styles.contains(style));
    }
    Ok(Collected {
        slots: visitor.slots,
        destinations: visitor.destinations,
        locked_styles,
    })
}

/// Structure/header validation is offline and does not open external profiles.
pub fn validate(raw: &Value) -> Result<()> {
    if !crate::color::has_v8(raw) {
        return Ok(());
    }
    let collected = collect(raw, Scope::Document, false)?;
    for (destination, owner, pointer) in collected
        .slots
        .iter()
        .map(|slot| (slot.color.destination(), &slot.owner, &slot.pointer))
        .chain(
            collected
                .destinations
                .iter()
                .map(|slot| (&slot.destination, &slot.owner, &slot.pointer)),
        )
    {
        if let Some(reference) = destination.profile() {
            let (_, record) = catalog::resolve_in(&raw["color_management"], reference)
                .with_context(|| format!("{owner}, color {pointer}"))?;
            let signature = destination.signature().unwrap();
            ensure!(record["color_space"]==signature,"[invalid-input] {owner}, color {pointer}: profile {reference} channel space is incompatible with {}",destination.space());
            ensure!(record["class"]!="link","[unsupported-capability] {owner}, color {pointer}: device-link profile {reference} cannot interpret native paint");
        }
    }
    super::separation::validate(raw)?;
    super::production::validate(raw)
}

/// Materialize only an ephemeral selected-page RGB preview. Native stored
/// values and unrelated page/profile dependencies remain untouched.
pub(crate) fn preview(raw: &mut Value, document: &Path, page: Option<&str>) -> Result<()> {
    preview_scope(
        raw,
        document,
        Scope::Page {
            page,
            visible_only: true,
        },
    )
    .map(|_| ())
}
pub(crate) fn preview_scope(
    raw: &mut Value,
    document: &Path,
    scope: Scope<'_>,
) -> Result<Vec<Value>> {
    if !crate::color::has_v8(raw) {
        return Ok(Vec::new());
    }
    // Style resolution can replace snapshot fallbacks in this private clone.
    // Restore content identities before catalog validation/profile loading.
    let mut name = if let Scope::Mask { name, .. } = scope {
        Some(name.to_owned())
    } else {
        None
    };
    let identity_name = name
        .as_ref()
        .is_some_and(|name| raw["masks"].get(name).is_none());
    let mut replacements = replace_mask_resources(raw)?;
    if identity_name {
        if let Some(name) = &mut name {
            *name = remap_identity(name, &replacements);
        }
    }
    let current = match (scope, &name) {
        (Scope::Mask { page, .. }, Some(name)) => Scope::Mask { page, name },
        _ => scope,
    };
    let destinations = destinations(raw, current)?;
    super::gradient::materialize(raw, &destinations)?;
    let changed = replace_mask_resources(raw)?;
    if identity_name {
        if let Some(name) = &mut name {
            *name = remap_identity(name, &changed);
        }
    }
    replacements.extend(changed);
    let current = match (scope, &name) {
        (Scope::Mask { page, .. }, Some(name)) => Scope::Mask { page, name },
        _ => scope,
    };
    let slots = slots(raw, current)?
        .into_iter()
        .filter(|slot| !super::gradient::preserves_slot(raw, &slot.pointer))
        .collect::<Vec<_>>();
    if slots.is_empty() {
        return Ok(replacements);
    }
    let profiles = ProfileSet::load(
        raw,
        document,
        slots
            .iter()
            .filter_map(|slot| slot.color.destination().profile()),
    )?;
    let mut engine = Evaluator::new(&profiles, Intent::RelativeColorimetric)?;
    let target = Destination::parse(&json!({"space":"srgb"}))?;
    for slot in slots {
        let converted = engine
            .convert(&slot.color, &target)
            .with_context(|| format!("{}, color {}", slot.owner, slot.pointer))?;
        let channels = converted.color.channels();
        let byte = |value: f64| (value.clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u8;
        let color = format!(
            "#{:02x}{:02x}{:02x}{:02x}",
            byte(channels[0]),
            byte(channels[1]),
            byte(channels[2]),
            byte(converted.color.alpha())
        );
        *raw.pointer_mut(&slot.pointer)
            .context("native color slot disappeared during preview planning")? = json!(color);
    }
    replacements.extend(replace_mask_resources(raw)?);
    Ok(replacements)
}

pub(crate) fn remap_identity(identity: &str, replacements: &[Value]) -> String {
    let mut identity = identity.to_owned();
    for replacement in replacements {
        if replacement["before"] == identity {
            identity = replacement["after"].as_str().unwrap().into();
        }
    }
    identity
}

/// Replace changed immutable snapshots by their new identity and update every
/// supported attachment and alias. Callers perform this only on a private plan
/// or preview clone; history retains the original snapshots for persistent edits.
pub(crate) fn replace_mask_resources(raw: &mut Value) -> Result<Vec<Value>> {
    let mut replacements = BTreeMap::new();
    if let Some(resources) = raw["mask_resources"].as_object() {
        for (old, resource) in resources {
            let new = crate::asset::hash_bytes(&serde_json::to_vec(resource)?);
            if old != &new {
                replacements.insert(old.clone(), new);
            }
        }
    }
    if replacements.is_empty() {
        return Ok(Vec::new());
    }
    let resources = raw["mask_resources"]
        .as_object_mut()
        .context("mask resource table missing")?;
    let moved: Vec<_> = replacements
        .iter()
        .map(|(old, new)| (new.clone(), resources.remove(old).expect("known resource")))
        .collect();
    for (new, resource) in moved {
        if let Some(existing) = resources.get(&new) {
            ensure!(
                existing == &resource,
                "[invalid-mask] replacement resource hash collision"
            );
        }
        resources.insert(new, resource);
    }
    fn node(value: &mut Value, replacements: &BTreeMap<String, String>) {
        if let Some(new) = value["mask"]["resource"]
            .as_str()
            .and_then(|id| replacements.get(id))
        {
            value["mask"]["resource"] = json!(new);
        }
        if let Some(children) = value["children"].as_array_mut() {
            for child in children {
                node(child, replacements);
            }
        }
        if value["kind"] == "instance" {
            node(&mut value["fallback"], replacements);
        }
    }
    if let Some(pages) = raw["pages"].as_array_mut() {
        for page in pages {
            if let Some(layers) = page["layers"].as_array_mut() {
                for layer in layers {
                    if let Some(nodes) = layer["nodes"].as_array_mut() {
                        for value in nodes {
                            node(value, &replacements);
                        }
                    }
                }
            }
        }
    }
    if let Some(components) = raw["components"].as_array_mut() {
        for component in components {
            if let Some(root) = component.get_mut("root") {
                node(root, &replacements);
            }
        }
    }
    if let Some(names) = raw["masks"].as_object_mut() {
        for digest in names.values_mut() {
            if let Some(new) = digest.as_str().and_then(|id| replacements.get(id)) {
                *digest = json!(new);
            }
        }
    }
    Ok(replacements
        .into_iter()
        .map(|(before, after)| json!({"before":before,"after":after}))
        .collect())
}
