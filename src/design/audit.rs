//! `pentool design audit` (`docs/design-systems-v1.md` section 9): read-only
//! comparison of literal values against the document's own tokens, unresolved or
//! invalid references, and brand usage rules. One pass over the scanned pages;
//! output is deterministic (document order) and paginated.
use super::brand;
use crate::{
    resource,
    tokens::{self, DesignError, Status, Usage},
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};

type Result<T> = std::result::Result<T, DesignError>;

pub const DEFAULT_LIMIT: usize = 50;
pub const MAX_LIMIT: usize = 500;
/// Nearest-colour search is skipped above this many candidate tokens per type.
pub const NEAREST_CANDIDATES: usize = 256;
pub const NEAREST_DELTA_E: f64 = 2.0;

const RULES: [&str; 6] = ["color", "font", "spacing", "radius", "refs", "brand"];

pub struct Options<'a> {
    pub document: &'a str,
    pub page: Option<&'a str>,
    pub all_pages: bool,
    pub rules: Option<&'a str>,
    pub library: Option<&'a str>,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
    pub fix_plan: bool,
    pub fix_nearest: bool,
}

fn invalid(message: impl Into<String>) -> DesignError {
    DesignError::new("invalid-input", message)
}

/// `(r, g, b, a)` of `#RGB`, `#RGBA`, `#RRGGBB` or `#RRGGBBAA`.
fn parse_color(text: &str) -> Option<[u8; 4]> {
    let hex = text.strip_prefix('#')?;
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let digit = |i: usize| u8::from_str_radix(&hex[i..i + 1], 16).ok().map(|d| d * 17);
    let pair = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    match hex.len() {
        3 => Some([digit(0)?, digit(1)?, digit(2)?, 255]),
        4 => Some([digit(0)?, digit(1)?, digit(2)?, digit(3)?]),
        6 => Some([pair(0)?, pair(2)?, pair(4)?, 255]),
        8 => Some([pair(0)?, pair(2)?, pair(4)?, pair(6)?]),
        _ => None,
    }
}

/// Canonical lowercase form used to compare colours exactly.
fn normalize(text: &str) -> Option<String> {
    let [r, g, b, a] = parse_color(text)?;
    Some(if a == 255 {
        format!("#{r:02x}{g:02x}{b:02x}")
    } else {
        format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
    })
}

fn lab(rgb: [u8; 4]) -> [f64; 3] {
    let lin = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let (r, g, b) = (lin(rgb[0]), lin(rgb[1]), lin(rgb[2]));
    let x = (0.4124564 * r + 0.3575761 * g + 0.1804375 * b) / 0.95047;
    let y = 0.2126729 * r + 0.7151522 * g + 0.0721750 * b;
    let z = (0.0193339 * r + 0.1191920 * g + 0.9503041 * b) / 1.08883;
    let f = |t: f64| {
        if t > 216.0 / 24389.0 {
            t.cbrt()
        } else {
            (24389.0 / 27.0 * t + 16.0) / 116.0
        }
    };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

fn delta_e(a: [u8; 4], b: [u8; 4]) -> f64 {
    let (a, b) = (lab(a), lab(b));
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn luminance(rgb: [u8; 4]) -> f64 {
    let lin = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(rgb[0]) + 0.7152 * lin(rgb[1]) + 0.0722 * lin(rgb[2])
}

fn contrast(a: [u8; 4], b: [u8; 4]) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

fn round(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// Token tables built once per audit.
struct Tables {
    colors: Vec<(String, String)>,
    color_exact: HashMap<String, String>,
    nearest_skipped: bool,
    fonts: Vec<(String, Value)>,
    radii: Vec<(String, (f64, f64))>,
}

fn radius_pair(value: &Value) -> Option<(f64, f64)> {
    match value {
        Value::Number(n) => n.as_f64().map(|r| (r, r)),
        Value::Array(items) if items.len() == 2 => Some((items[0].as_f64()?, items[1].as_f64()?)),
        _ => None,
    }
}

fn typography_matches(
    token: &Value,
    font: &str,
    size: Option<f64>,
    weight: Option<f64>,
    italic: bool,
) -> bool {
    let Some(object) = token.as_object() else {
        return false;
    };
    if object.is_empty() {
        return false;
    }
    object.iter().all(|(key, value)| match key.as_str() {
        "font" => value.as_str() == Some(font),
        "size" => value.as_f64().is_some() && value.as_f64() == size,
        "weight" => value.as_f64().is_some() && value.as_f64() == weight,
        "italic" => value.as_bool() == Some(italic),
        _ => true,
    })
}

fn build_tables(styles: &Value, library: Option<&str>) -> Tables {
    let mut tables = Tables {
        colors: Vec::new(),
        color_exact: HashMap::new(),
        nearest_skipped: false,
        fonts: Vec::new(),
        radii: Vec::new(),
    };
    let Some(map) = styles.as_object() else {
        return tables;
    };
    let mut names: Vec<&String> = map.keys().collect();
    names.sort();
    for name in names {
        if let Some(wanted) = library {
            let from = map[name].pointer("/source/library").and_then(Value::as_str);
            if from != Some(wanted) {
                continue;
            }
        }
        let resolution = tokens::resolve(styles, name);
        if resolution.status != Status::Resolved {
            continue;
        }
        let Some(value) = resolution.value else {
            continue;
        };
        match resolution.token_type.as_deref() {
            Some("color") => {
                if let Some(color) = value.as_str().and_then(normalize) {
                    tables
                        .color_exact
                        .entry(color.clone())
                        .or_insert_with(|| name.clone());
                    tables.colors.push((name.clone(), color));
                }
            }
            Some("typography") => tables.fonts.push((name.clone(), value)),
            Some("radius") => {
                if let Some(pair) = radius_pair(&value) {
                    tables.radii.push((name.clone(), pair));
                }
            }
            _ => {}
        }
    }
    tables.nearest_skipped = tables.colors.len() > NEAREST_CANDIDATES;
    tables
}

struct Finding {
    value: Value,
}

struct Scan<'a> {
    styles: &'a Value,
    tables: &'a Tables,
    rules: &'a BTreeSet<&'a str>,
    brand: Option<&'a Value>,
    findings: Vec<Finding>,
    /// `(page, id, property, token, exact)` candidates for `--fix-plan`.
    fixes: Vec<(String, String, String, String, bool, bool)>,
}

struct NodeContext<'a> {
    page: &'a str,
    background: Option<[u8; 4]>,
    layer_hidden: bool,
    layer_locked: bool,
}

impl Scan<'_> {
    fn effective_color(&self, slot: &Value) -> Option<String> {
        if let Some(reference) = slot.get("ref").and_then(Value::as_str) {
            let resolution = tokens::resolve(self.styles, reference);
            if resolution.status == Status::Resolved {
                return resolution
                    .value
                    .and_then(|v| v.as_str().and_then(normalize));
            }
        }
        slot.get("fallback")
            .and_then(Value::as_str)
            .and_then(normalize)
    }

    fn token_color(&self, token: &str) -> Option<String> {
        let resolution = tokens::resolve(self.styles, token);
        if resolution.status != Status::Resolved {
            return None;
        }
        resolution.value?.as_str().and_then(normalize)
    }

    fn node(&mut self, node: &Value, context: &NodeContext<'_>, depth: usize) {
        let id = node.get("id").and_then(Value::as_str).unwrap_or("");
        let kind = node.get("kind").and_then(Value::as_str).unwrap_or("group");
        let hidden =
            context.layer_hidden || node.get("visible").and_then(Value::as_bool) == Some(false);
        let locked =
            context.layer_locked || node.get("locked").and_then(Value::as_bool) == Some(true);
        let mut local: Vec<(String, &'static str, Value)> = Vec::new();
        let mut add = |path: &str, code: &'static str, severity: &str, extra: Value| {
            let mut value = json!({
                "code": code, "severity": severity, "page": context.page, "id": id,
                "kind": kind, "path": path, "depth": depth,
            });
            if hidden {
                value["hidden"] = json!(true);
            }
            if locked {
                value["locked"] = json!(true);
            }
            for (key, item) in extra.as_object().into_iter().flatten() {
                value[key] = item.clone();
            }
            local.push((path.to_owned(), code, value));
        };

        let mut paints: BTreeMap<&str, Option<String>> = BTreeMap::new();
        if let Some(style) = node.get("style").and_then(Value::as_object) {
            for property in ["fill", "stroke"] {
                let Some(slot) = style.get(property) else {
                    continue;
                };
                let path = format!("style.{property}");
                let mut literal = true;
                if let Some(reference) = slot.get("ref").and_then(Value::as_str) {
                    let resolution = tokens::resolve(self.styles, reference);
                    match resolution.status {
                        Status::Resolved => literal = false,
                        Status::Unresolved if self.rules.contains("refs") => add(
                            &path,
                            "unresolved-token",
                            "error",
                            json!({"token": reference, "reason": resolution.reason}),
                        ),
                        Status::Invalid if self.rules.contains("refs") => add(
                            &path,
                            "invalid-token",
                            "error",
                            json!({"token": reference, "reason": resolution.reason}),
                        ),
                        _ => {}
                    }
                }
                paints.insert(property, self.effective_color(slot));
                if !literal || !self.rules.contains("color") || self.tables.colors.is_empty() {
                    continue;
                }
                let Some(text) = slot.get("fallback").and_then(Value::as_str) else {
                    continue;
                };
                let Some(rgba) = parse_color(text) else {
                    continue;
                };
                if text == "none" || rgba[3] == 0 {
                    continue;
                }
                let normal = normalize(text).unwrap_or_default();
                if let Some(token) = self.tables.color_exact.get(&normal) {
                    add(
                        &path,
                        "off-token-color",
                        "warn",
                        json!({"value": text, "suggest": token, "exact": true}),
                    );
                    self.fixes.push((
                        context.page.to_owned(),
                        id.to_owned(),
                        property.to_owned(),
                        token.clone(),
                        true,
                        locked,
                    ));
                } else if !self.tables.nearest_skipped {
                    let mut best: Option<(f64, &str)> = None;
                    for (name, color) in &self.tables.colors {
                        let Some(other) = parse_color(color) else {
                            continue;
                        };
                        let distance = delta_e(rgba, other);
                        if best.is_none_or(|(d, _)| distance < d) {
                            best = Some((distance, name));
                        }
                    }
                    match best {
                        Some((distance, name)) if distance <= NEAREST_DELTA_E => {
                            add(
                                &path,
                                "off-token-color",
                                "warn",
                                json!({"value": text, "suggest": name, "exact": false,
                                       "delta_e": round(distance)}),
                            );
                            self.fixes.push((
                                context.page.to_owned(),
                                id.to_owned(),
                                property.to_owned(),
                                name.to_owned(),
                                false,
                                locked,
                            ));
                        }
                        _ => add(&path, "off-token-color", "warn", json!({"value": text})),
                    }
                } else {
                    add(
                        &path,
                        "off-token-color",
                        "warn",
                        json!({"value": text, "nearest": "skipped"}),
                    );
                }
            }
        }

        if kind == "text" {
            self.text_node(node, context, &paints, &mut add);
        }
        if kind == "rect" && self.rules.contains("radius") && !self.tables.radii.is_empty() {
            let rx = node.get("radius_x").and_then(Value::as_f64).unwrap_or(0.0);
            let ry = node.get("radius_y").and_then(Value::as_f64).unwrap_or(0.0);
            if rx != 0.0 || ry != 0.0 {
                let hit = self
                    .tables
                    .radii
                    .iter()
                    .find(|(_, (a, b))| *a == rx && *b == ry);
                let value = if rx == ry { json!(rx) } else { json!([rx, ry]) };
                if hit.is_none() {
                    add(
                        "radius",
                        "off-token-radius",
                        "warn",
                        json!({"value": value}),
                    );
                }
            }
        }
        if self.rules.contains("brand") {
            self.brand_node(node, context, &paints, &mut add);
        }
        local.sort_by(|a, b| (a.0.as_str(), a.1).cmp(&(b.0.as_str(), b.1)));
        self.findings
            .extend(local.into_iter().map(|(_, _, value)| Finding { value }));
    }

    fn font_match(&self, node: &Value, candidates: &[(String, Value)]) -> Option<String> {
        let font = node
            .get("font_family")
            .and_then(Value::as_str)
            .unwrap_or("");
        let size = node.get("font_size").and_then(Value::as_f64);
        let weight = node.get("font_weight").and_then(Value::as_f64);
        let italic = node.get("italic").and_then(Value::as_bool).unwrap_or(false);
        candidates
            .iter()
            .find(|(_, token)| typography_matches(token, font, size, weight, italic))
            .map(|(name, _)| name.clone())
    }

    fn text_node(
        &self,
        node: &Value,
        _context: &NodeContext<'_>,
        _paints: &BTreeMap<&str, Option<String>>,
        add: &mut impl FnMut(&str, &'static str, &str, Value),
    ) {
        if self.rules.contains("font") && !self.tables.fonts.is_empty() {
            let describe = json!({
                "font": node.get("font_family"), "size": node.get("font_size"),
                "weight": node.get("font_weight"), "italic": node.get("italic"),
            });
            if self.font_match(node, &self.tables.fonts).is_none() {
                add("font", "off-token-font", "warn", json!({"value": describe}));
            }
        }
    }

    fn brand_node(
        &self,
        node: &Value,
        context: &NodeContext<'_>,
        paints: &BTreeMap<&str, Option<String>>,
        add: &mut impl FnMut(&str, &'static str, &str, Value),
    ) {
        let Some(brand) = self.brand else { return };
        let kind = node.get("kind").and_then(Value::as_str).unwrap_or("group");
        let rules = brand.get("rules").and_then(Value::as_array);
        for rule in rules.into_iter().flatten() {
            let rule_id = rule.get("id").and_then(Value::as_str).unwrap_or("");
            match rule.get("kind").and_then(Value::as_str) {
                Some("contrast") if kind == "text" => {
                    let min = rule.get("min").and_then(Value::as_f64).unwrap_or(1.0);
                    let color = paints.get("fill").cloned().flatten();
                    if let (Some(text), Some(background)) =
                        (color.as_deref().and_then(parse_color), context.background)
                    {
                        if text[3] == 255 {
                            let ratio = contrast(text, background);
                            if ratio < min {
                                add(
                                    "style.fill",
                                    "brand-contrast",
                                    "warn",
                                    json!({"rule": rule_id, "ratio": round(ratio), "min": min}),
                                );
                            }
                        }
                    }
                }
                Some("forbidden_pairs") => {
                    let fill = paints.get("fill").cloned().flatten();
                    let stroke = paints.get("stroke").cloned().flatten();
                    let background = context.background.map(|c| {
                        normalize(&format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]))
                            .unwrap_or_default()
                    });
                    for pair in rule
                        .get("pairs")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                    {
                        let names: Vec<&str> = pair
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(Value::as_str)
                            .collect();
                        let [Some(a), Some(b)] = [
                            names.first().and_then(|n| self.token_color(n)),
                            names.get(1).and_then(|n| self.token_color(n)),
                        ] else {
                            continue;
                        };
                        let mut used = |x: &Option<String>, y: &Option<String>, path: &str| {
                            let hit = matches!((x, y), (Some(x), Some(y))
                                if (x == &a && y == &b) || (x == &b && y == &a));
                            if hit {
                                add(
                                    path,
                                    "brand-pair",
                                    "warn",
                                    json!({"rule": rule_id, "pair": names}),
                                );
                            }
                        };
                        used(&fill, &stroke, "style.stroke");
                        if kind == "text" {
                            used(&fill, &background, "style.fill");
                        }
                    }
                }
                Some("fonts") if kind == "text" => {
                    let allowed: Vec<(String, Value)> = rule
                        .get("allowed")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .filter_map(|name| {
                            let r = tokens::resolve(self.styles, name);
                            if r.status == Status::Resolved {
                                r.value.map(|v| (name.to_owned(), v))
                            } else {
                                None
                            }
                        })
                        .collect();
                    if !allowed.is_empty() && self.font_match(node, &allowed).is_none() {
                        add(
                            "font",
                            "brand-font",
                            "warn",
                            json!({"rule": rule_id,
                                   "value": {"font": node.get("font_family"),
                                             "size": node.get("font_size"),
                                             "weight": node.get("font_weight")}}),
                        );
                    }
                }
                Some("logo") if kind == "image" => {
                    let id = node.get("id").and_then(Value::as_str).unwrap_or("");
                    let asset = node.get("asset").and_then(Value::as_str).unwrap_or("\u{0}");
                    for logo in brand
                        .get("logos")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                    {
                        let matches = logo.get("id").and_then(Value::as_str) == Some(id)
                            || logo.get("asset").and_then(Value::as_str) == Some(asset);
                        if !matches {
                            continue;
                        }
                        let width = node.get("width").and_then(Value::as_f64);
                        if let (Some(min), Some(width)) =
                            (logo.get("min_width").and_then(Value::as_f64), width)
                        {
                            if width < min {
                                add(
                                    "width",
                                    "brand-logo",
                                    "warn",
                                    json!({"rule": rule_id, "reason": "below min_width",
                                           "width": width, "min_width": min}),
                                );
                            }
                        }
                        let background = context.background.map(|c| {
                            normalize(&format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]))
                                .unwrap_or_default()
                        });
                        for token in logo
                            .get("forbidden_backgrounds")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                            .filter_map(Value::as_str)
                        {
                            if background.is_some() && self.token_color(token) == background {
                                add(
                                    "canvas.background",
                                    "brand-logo",
                                    "warn",
                                    json!({"rule": rule_id, "reason": "forbidden background",
                                           "token": token}),
                                );
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn walk(&mut self, nodes: &[Value], context: &NodeContext<'_>, depth: usize) {
        for node in nodes {
            self.node(node, context, depth);
            let kind = node.get("kind").and_then(Value::as_str);
            // Instances are audited by their exposed properties, not their internals.
            if kind == Some("instance") {
                continue;
            }
            if let Some(children) = node.get("children").and_then(Value::as_array) {
                // A locked group locks its children.
                if node.get("locked").and_then(Value::as_bool) == Some(true) {
                    let inner = NodeContext {
                        page: context.page,
                        background: context.background,
                        layer_hidden: context.layer_hidden,
                        layer_locked: true,
                    };
                    self.walk(children, &inner, depth + 1);
                } else {
                    self.walk(children, context, depth + 1);
                }
            }
        }
    }
}

fn parse_rules(rules: Option<&str>) -> Result<BTreeSet<&'static str>> {
    let Some(rules) = rules else {
        return Ok(RULES.iter().copied().collect());
    };
    let mut out = BTreeSet::new();
    for item in rules.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        match RULES.iter().find(|known| **known == item) {
            Some(known) => {
                out.insert(*known);
            }
            None => {
                return Err(invalid(format!("unknown audit rule {item:?}"))
                    .action(format!("use --rules with a subset of {}", RULES.join(","))))
            }
        }
    }
    if out.is_empty() {
        return Err(invalid("--rules is empty").action("name at least one rule"));
    }
    Ok(out)
}

/// Run the audit over parsed document bytes. Never mutates anything.
pub fn run(bytes: &[u8], raw: &Value, options: &Options<'_>) -> Result<Value> {
    let limit = options.limit.unwrap_or(DEFAULT_LIMIT);
    if limit == 0 || limit > MAX_LIMIT {
        return Err(invalid(format!(
            "--limit must be between 1 and {MAX_LIMIT}, got {limit}"
        ))
        .action(format!("use --limit 1..={MAX_LIMIT} and --offset to page")));
    }
    let offset = options.offset.unwrap_or(0);
    if options.page.is_some() && options.all_pages {
        return Err(invalid("--page and --all-pages cannot be combined")
            .action("pass either one page ID or --all-pages"));
    }
    let rules = parse_rules(options.rules)?;
    let styles = raw.get("styles").cloned().unwrap_or_else(|| json!({}));
    if let Some(library) = options.library {
        if raw
            .pointer(&format!("/design/libraries/{library}"))
            .is_none()
            && !styles.as_object().is_some_and(|m| {
                m.values()
                    .any(|r| r.pointer("/source/library").and_then(Value::as_str) == Some(library))
            })
        {
            return Err(invalid(format!(
                "library {library:?} is not installed in this document"
            ))
            .action("run `pentool design library list` or omit --library"));
        }
    }
    let pages = raw
        .get("pages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let scanned: Vec<&Value> = if options.all_pages {
        pages.iter().collect()
    } else {
        let wanted = options.page.or_else(|| {
            pages
                .first()
                .and_then(|p| p.get("id"))
                .and_then(Value::as_str)
        });
        let found = pages
            .iter()
            .find(|p| p.get("id").and_then(Value::as_str) == wanted);
        match found {
            Some(page) => vec![page],
            None => {
                return Err(
                    invalid(format!("page {:?} does not exist", wanted.unwrap_or("")))
                        .action("list pages with `pentool pages` or pass --all-pages"),
                )
            }
        }
    };

    let tables = build_tables(&styles, options.library);
    let brand = raw.pointer("/design/brand");
    let mut scan = Scan {
        styles: &styles,
        tables: &tables,
        rules: &rules,
        brand,
        findings: Vec::new(),
        fixes: Vec::new(),
    };
    for page in &scanned {
        let page_id = page.get("id").and_then(Value::as_str).unwrap_or("");
        let background = page
            .pointer("/canvas/background")
            .and_then(Value::as_str)
            .and_then(parse_color)
            .filter(|c| c[3] == 255);
        for layer in page
            .get("layers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let context = NodeContext {
                page: page_id,
                background,
                layer_hidden: layer.get("visible").and_then(Value::as_bool) == Some(false),
                layer_locked: layer.get("locked").and_then(Value::as_bool) == Some(true),
            };
            if let Some(nodes) = layer.get("nodes").and_then(Value::as_array) {
                scan.walk(nodes, &context, 0);
            }
        }
    }
    let mut findings = std::mem::take(&mut scan.findings);
    let fixes = std::mem::take(&mut scan.fixes);

    // Document-level token findings come after the page findings, sorted by token.
    if rules.contains("refs") {
        let usage = Usage::build(raw);
        let mut used: BTreeSet<String> = BTreeSet::new();
        if let Some(brand) = brand {
            used.extend(brand::referenced_tokens(brand));
        }
        let mut names: Vec<&String> = styles
            .as_object()
            .into_iter()
            .flat_map(|m| m.keys())
            .collect();
        names.sort();
        for name in names {
            let resolution = tokens::resolve(&styles, name);
            if resolution.status == Status::Invalid {
                findings.push(Finding {
                    value: json!({"code": "invalid-token", "severity": "error", "token": name,
                                  "path": format!("styles.{name}"), "reason": resolution.reason}),
                });
            }
        }
        let mut unused = Vec::new();
        if let Some(map) = styles.as_object() {
            let mut names: Vec<&String> = map.keys().collect();
            names.sort();
            for name in names {
                let from = map[name].pointer("/source/library").and_then(Value::as_str);
                if options.library.is_some_and(|wanted| from != Some(wanted)) {
                    continue;
                }
                let (nodes, tokens_) = usage.dependents(name);
                if nodes.is_empty() && tokens_.is_empty() && !used.contains(name.as_str()) {
                    unused.push(name.clone());
                }
            }
        }
        for name in unused {
            findings.push(Finding {
                value: json!({"code": "unused-token", "severity": "info", "token": name,
                              "path": format!("styles.{name}")}),
            });
        }
    }

    let mut summary: BTreeMap<String, usize> = BTreeMap::new();
    let mut severities: BTreeMap<&str, usize> = [("error", 0), ("warn", 0), ("info", 0)]
        .into_iter()
        .collect();
    for finding in &findings {
        let code = finding.value["code"].as_str().unwrap_or("");
        *summary.entry(code.to_owned()).or_default() += 1;
        if let Some(count) = severities.get_mut(finding.value["severity"].as_str().unwrap_or("")) {
            *count += 1;
        }
    }
    let matches = findings.len();
    let returned: Vec<Value> = findings
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|f| f.value)
        .collect();
    let mut report = json!({
        "document": options.document,
        "revision": resource::sha256(bytes),
        "scope": {"pages": scanned.iter().filter_map(|p| p.get("id").and_then(Value::as_str)).collect::<Vec<_>>(),
                  "rules": rules},
        "matches": matches,
        "returned": returned.len(),
        "offset": offset,
        "limit": limit,
        "has_more": offset + returned.len() < matches,
        "summary": summary,
        "severities": severities,
        "findings": returned,
    });
    if tables.nearest_skipped {
        report["nearest"] = json!("skipped");
    }
    if options.fix_plan {
        report["plan"] = fix_plan(&report["findings"], &fixes, options.fix_nearest);
    }
    Ok(report)
}

/// Batch operations for the returned findings only (pagination-consistent).
/// Operations apply to a single page (`pentool --page ID batch ...`), so the plan
/// covers the first page that has fixable findings and names the rest.
fn fix_plan(
    returned: &Value,
    fixes: &[(String, String, String, String, bool, bool)],
    nearest: bool,
) -> Value {
    let mut by_page: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut skipped = Vec::new();
    let mut seen = BTreeSet::new();
    for finding in returned.as_array().into_iter().flatten() {
        if finding["code"] != "off-token-color" || finding.get("suggest").is_none() {
            continue;
        }
        let page = finding["page"].as_str().unwrap_or("").to_owned();
        let id = finding["id"].as_str().unwrap_or("").to_owned();
        let property = finding["path"]
            .as_str()
            .and_then(|p| p.strip_prefix("style."))
            .unwrap_or("")
            .to_owned();
        let Some((_, _, _, token, exact, locked)) = fixes
            .iter()
            .find(|f| f.0 == page && f.1 == id && f.2 == property)
        else {
            continue;
        };
        if !seen.insert((page.clone(), id.clone(), property.clone())) {
            continue;
        }
        if !exact && !nearest {
            skipped.push(json!({"page": page, "id": id, "property": property,
                                "reason": "near match; pass --fix-nearest to include it"}));
            continue;
        }
        if *locked {
            skipped.push(json!({"page": page, "id": id, "property": property,
                                "reason": "locked"}));
            continue;
        }
        if !by_page.contains_key(&page) {
            order.push(page.clone());
        }
        by_page
            .entry(page)
            .or_default()
            .push(json!({"type": "bind-token", "id": id, "property": property, "token": token}));
    }
    let first = order.first().cloned();
    let operations = first
        .as_ref()
        .and_then(|p| by_page.remove(p))
        .unwrap_or_default();
    let mut plan = json!({"page": first, "operations": operations, "skipped": skipped});
    if order.len() > 1 {
        plan["other_pages"] = json!(order[1..]);
    }
    plan
}

/// Exit status requested by `--fail-on` for a finished report: 3 when matches of
/// that severity (or worse) exist.
pub fn fail_status(report: &Value, fail_on: &str) -> i32 {
    let count = |name: &str| report["severities"][name].as_u64().unwrap_or(0);
    let failed = match fail_on {
        "error" => count("error") > 0,
        _ => count("error") + count("warn") > 0,
    };
    if failed {
        3
    } else {
        0
    }
}
