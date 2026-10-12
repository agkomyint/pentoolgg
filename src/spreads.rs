//! Format version 9, part one: spreads, parent pages, sections and page
//! numbering (docs/layout-v1.md sections 2-3). Stories, frames and composition
//! arrive with later v0.14.0 tasks; this module never lays out text.
//!
//! Everything here edits an in-memory document. Callers commit through the
//! shared transaction path, so a failed operation leaves the file untouched.
use crate::page::PageAction;
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};

pub const VERSION: u64 = 9;
pub const ENGINE: u64 = 1;
pub const MAX_PAGES: usize = 5000;
pub const MAX_PAGES_PER_SPREAD: usize = 10;
pub const MAX_PARENTS: usize = 500;
pub const MAX_PARENT_DEPTH: usize = 16;
pub const MAX_SECTIONS: usize = 5000;
const MAX_ID_LEN: usize = 128;
const MAX_START_NUMBER: u64 = 100_000;
const MAX_PREFIX: usize = 16;
const STYLES: [&str; 5] = [
    "arabic",
    "upper-roman",
    "lower-roman",
    "upper-alpha",
    "lower-alpha",
];

pub fn is_v9(raw: &Value) -> bool {
    raw.get("version").and_then(Value::as_u64) == Some(VERSION)
}

// ---------------------------------------------------------------- numbering

fn roman(mut n: u64) -> String {
    const TABLE: [(u64, &str); 13] = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut out = String::new();
    for (value, text) in TABLE {
        while n >= value {
            out.push_str(text);
            n -= value;
        }
    }
    out
}

/// Page number text per docs/layout-v1.md section 3.4. `n` is at least 1.
pub fn format_number(style: &str, n: u64) -> String {
    match style {
        "lower-roman" => roman(n),
        "upper-roman" => roman(n).to_uppercase(),
        "lower-alpha" | "upper-alpha" => {
            let letter = (b'a' + ((n - 1) % 26) as u8) as char;
            let text: String = std::iter::repeat_n(letter, ((n - 1) / 26 + 1) as usize).collect();
            if style == "upper-alpha" {
                text.to_uppercase()
            } else {
                text
            }
        }
        _ => n.to_string(),
    }
}

// ---------------------------------------------------------------- accessors

fn pages(raw: &Value) -> &[Value] {
    raw.get("pages")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn page_id(page: &Value) -> &str {
    page.get("id").and_then(Value::as_str).unwrap_or_default()
}

fn page_index(raw: &Value, id: &str) -> Result<usize> {
    pages(raw)
        .iter()
        .position(|page| page_id(page) == id)
        .with_context(|| format!("[missing-resource] page not found: {id}"))
}

fn list<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value
        .get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn check_id(what: &str, id: &str) -> Result<()> {
    ensure!(
        !id.is_empty()
            && id.chars().count() <= MAX_ID_LEN
            && !id.chars().any(|c| c.is_control() || c == '/' || c == '\\'),
        "[invalid-input] {what} ID {id:?} must be 1-{MAX_ID_LEN} characters without control characters, '/' or '\\'"
    );
    Ok(())
}

fn number_in(value: &Value, min: f64, max: f64) -> bool {
    value
        .as_f64()
        .is_some_and(|n| n.is_finite() && (min..=max).contains(&n))
}

fn check_geometry(value: &Value, what: &str) -> Result<()> {
    let object = value
        .as_object()
        .with_context(|| format!("[malformed-resource] {what} must be an object"))?;
    if let Some(margins) = object.get("margins") {
        let ok = margins
            .as_array()
            .is_some_and(|m| m.len() == 4 && m.iter().all(|v| number_in(v, 0.0, 1_000_000.0)));
        ensure!(
            ok,
            "[malformed-resource] {what}.margins must be four numbers from 0 to 1000000"
        );
    }
    if let Some(columns) = object.get("columns") {
        let ok = columns.as_u64().is_some_and(|c| (1..=40).contains(&c));
        ensure!(
            ok,
            "[malformed-resource] {what}.columns must be an integer from 1 to 40"
        );
    }
    if let Some(gutter) = object.get("gutter") {
        ensure!(
            number_in(gutter, 0.0, 100_000.0),
            "[malformed-resource] {what}.gutter must be a number from 0 to 100000"
        );
    }
    Ok(())
}

// --------------------------------------------------------------- validation

/// Structure checks for the v9 `layout` object and per-page `layout` data. A
/// document before v9 keeps `layout` as an uninterpreted extension.
pub fn validate(raw: &Value, version: u64) -> Result<()> {
    if version < VERSION {
        return crate::stories::reject_frames(raw);
    }
    let page_list = pages(raw);
    ensure!(
        page_list.len() <= MAX_PAGES,
        "[limit-exceeded] pages per document: {} exceeds the limit of {MAX_PAGES}",
        page_list.len()
    );
    let order: HashMap<&str, usize> = page_list
        .iter()
        .enumerate()
        .map(|(i, p)| (page_id(p), i))
        .collect();
    let parent_ids = validate_layout(raw, &order)?;
    for page in page_list {
        validate_page_layout(page, &parent_ids)?;
    }
    crate::styles::validate(raw)?;
    crate::stories::validate(raw)?;
    crate::grids::validate(raw)?;
    crate::wrap::validate(raw)?;
    crate::tables::validate(raw)?;
    Ok(())
}

fn validate_layout<'a>(raw: &'a Value, order: &HashMap<&str, usize>) -> Result<HashSet<&'a str>> {
    let mut parent_ids = HashSet::new();
    let Some(layout) = raw.get("layout") else {
        return Ok(parent_ids);
    };
    ensure!(
        layout.is_object(),
        "[malformed-resource] layout must be an object"
    );
    let engine = layout
        .get("engine")
        .and_then(Value::as_u64)
        .filter(|e| *e >= 1)
        .context("[malformed-resource] layout.engine must be a positive integer")?;
    ensure!(
        engine <= ENGINE,
        "[unsupported-version] layout engine {engine} is newer than this build supports ({ENGINE}); update Pentool"
    );
    if let Some(defaults) = layout.get("page_defaults") {
        check_geometry(defaults, "layout.page_defaults")?;
    }

    if let Some(spreads) = layout.get("spreads") {
        let spreads = spreads
            .as_array()
            .context("[malformed-resource] layout.spreads must be an array")?;
        ensure!(
            spreads.len() <= MAX_PAGES,
            "[limit-exceeded] spreads per document: {} exceeds the limit of {MAX_PAGES}",
            spreads.len()
        );
        let mut seen = HashSet::new();
        let mut flat: Vec<&str> = Vec::new();
        for spread in spreads {
            let id = spread
                .get("id")
                .and_then(Value::as_str)
                .context("[malformed-resource] a spread has no ID")?;
            check_id("spread", id)?;
            ensure!(seen.insert(id), "[invalid-input] duplicate spread ID {id}");
            let members = spread
                .get("pages")
                .and_then(Value::as_array)
                .with_context(|| format!("[malformed-resource] spread {id} has no pages array"))?;
            ensure!(
                (1..=MAX_PAGES_PER_SPREAD).contains(&members.len()),
                "[limit-exceeded] pages per spread {id}: {} (allowed 1-{MAX_PAGES_PER_SPREAD})",
                members.len()
            );
            for member in members {
                let page = member.as_str().with_context(|| {
                    format!("[malformed-resource] spread {id} lists a non-string page")
                })?;
                ensure!(
                    order.contains_key(page),
                    "[missing-resource] spread {id} references missing page {page}"
                );
                flat.push(page);
            }
            if let Some(facing) = spread.get("facing") {
                ensure!(
                    facing.is_boolean(),
                    "[malformed-resource] spread {id} facing must be true or false"
                );
            }
        }
        let expected: Vec<&str> = pages(raw).iter().map(page_id).collect();
        if flat != expected {
            let mut once = HashSet::new();
            if let Some(dup) = flat.iter().find(|p| !once.insert(**p)) {
                bail!("[invalid-input] page {dup} is in more than one spread");
            }
            if let Some(missing) = expected.iter().find(|p| !flat.contains(p)) {
                bail!("[invalid-input] page {missing} is not in any spread");
            }
            bail!(
                "[invalid-input] spread order must match page order; run `pentool spread arrange` or `page move` to reconcile"
            );
        }
    }

    // Parents.
    let parents = list(layout, "parents");
    ensure!(
        parents.len() <= MAX_PARENTS,
        "[limit-exceeded] parents per document: {} exceeds the limit of {MAX_PARENTS}",
        parents.len()
    );
    let mut based: HashMap<&str, Option<&str>> = HashMap::new();
    for parent in parents {
        let id = parent
            .get("id")
            .and_then(Value::as_str)
            .context("[malformed-resource] a parent has no ID")?;
        check_id("parent", id)?;
        ensure!(
            parent_ids.insert(id),
            "[invalid-input] duplicate parent ID {id}"
        );
        match parent.get("based_on") {
            None | Some(Value::Null) => {
                based.insert(id, None);
            }
            Some(Value::String(base)) => {
                based.insert(id, Some(base.as_str()));
            }
            Some(_) => bail!("[malformed-resource] parent {id} based_on must be an ID or null"),
        }
        check_geometry(parent, &format!("parent {id}"))?;
        let members = list(parent, "pages");
        ensure!(
            members.len() <= MAX_PAGES_PER_SPREAD,
            "[limit-exceeded] parent pages of {id}: {} exceeds the limit of {MAX_PAGES_PER_SPREAD}",
            members.len()
        );
        let mut names = HashSet::new();
        for member in members {
            let name = member.as_str().with_context(|| {
                format!("[malformed-resource] parent {id} pages must be strings")
            })?;
            check_id("parent page", name)?;
            ensure!(
                names.insert(name),
                "[invalid-input] parent {id} lists page {name} twice"
            );
        }
        if let Some(layers) = parent.get("layers") {
            let layers = layers.as_array().with_context(|| {
                format!("[malformed-resource] parent {id} layers must be an array")
            })?;
            let mut layer_ids = HashSet::new();
            for layer in layers {
                let layer_id = layer.get("id").and_then(Value::as_str).with_context(|| {
                    format!("[malformed-resource] parent {id} has a layer without an ID")
                })?;
                ensure!(
                    layer_ids.insert(layer_id),
                    "[invalid-input] parent {id} has duplicate layer {layer_id}"
                );
                ensure!(
                    layer.get("nodes").is_some_and(Value::is_array),
                    "[malformed-resource] parent {id} layer {layer_id} has no nodes array"
                );
            }
        }
    }
    for (id, base) in &based {
        if let Some(base) = base {
            ensure!(
                *base != *id,
                "[invalid-reference] parent {id} cannot be based on itself"
            );
            ensure!(
                based.contains_key(base),
                "[missing-resource] parent {id} is based on missing parent {base}"
            );
        }
    }
    for id in based.keys() {
        let mut depth = 1;
        let mut seen = HashSet::from([*id]);
        let mut cursor = based[id];
        while let Some(base) = cursor {
            ensure!(
                seen.insert(base),
                "[invalid-reference] parent chain from {id} cycles at {base}"
            );
            depth += 1;
            ensure!(
                depth <= MAX_PARENT_DEPTH,
                "[limit-exceeded] parent chain depth from {id} exceeds the limit of {MAX_PARENT_DEPTH}"
            );
            cursor = based[base];
        }
    }

    // Sections.
    let sections = list(layout, "sections");
    ensure!(
        sections.len() <= MAX_SECTIONS,
        "[limit-exceeded] sections per document: {} exceeds the limit of {MAX_SECTIONS}",
        sections.len()
    );
    let mut section_ids = HashSet::new();
    let mut starts = HashSet::new();
    for section in sections {
        let id = section
            .get("id")
            .and_then(Value::as_str)
            .context("[malformed-resource] a section has no ID")?;
        check_id("section", id)?;
        ensure!(
            section_ids.insert(id),
            "[invalid-input] duplicate section ID {id}"
        );
        let start = section
            .get("start_page")
            .and_then(Value::as_str)
            .with_context(|| format!("[malformed-resource] section {id} has no start_page"))?;
        ensure!(
            order.contains_key(start),
            "[missing-resource] section {id} starts at missing page {start}"
        );
        ensure!(
            starts.insert(start),
            "[invalid-input] two sections start at page {start}"
        );
        if let Some(number) = section.get("start_number") {
            ensure!(
                number
                    .as_u64()
                    .is_some_and(|n| (1..=MAX_START_NUMBER).contains(&n)),
                "[malformed-resource] section {id} start_number must be an integer from 1 to {MAX_START_NUMBER}"
            );
        }
        if let Some(style) = section.get("style") {
            ensure!(
                style.as_str().is_some_and(|s| STYLES.contains(&s)),
                "[malformed-resource] section {id} style must be one of {}",
                STYLES.join(", ")
            );
        }
        if let Some(prefix) = section.get("prefix") {
            ensure!(
                prefix.as_str().is_some_and(|p| p.chars().count() <= MAX_PREFIX
                    && p.chars().all(|c| (' '..='~').contains(&c))),
                "[malformed-resource] section {id} prefix must be at most {MAX_PREFIX} printable ASCII characters"
            );
        }
        if let Some(cont) = section.get("continue") {
            ensure!(
                cont.is_boolean(),
                "[malformed-resource] section {id} continue must be true or false"
            );
        }
    }
    Ok(parent_ids)
}

fn validate_page_layout(page: &Value, parent_ids: &HashSet<&str>) -> Result<()> {
    let id = page_id(page);
    let Some(layout) = page.get("layout") else {
        return Ok(());
    };
    ensure!(
        layout.is_object(),
        "[malformed-resource] page {id} layout must be an object"
    );
    match layout.get("parent") {
        None | Some(Value::Null) => {}
        Some(Value::String(parent)) => ensure!(
            parent_ids.contains(parent.as_str()),
            "[missing-resource] page {id} references missing parent {parent}"
        ),
        Some(_) => bail!("[malformed-resource] page {id} layout.parent must be an ID or null"),
    }
    check_geometry(layout, &format!("page {id} layout"))?;
    let mut overridden = HashSet::new();
    for entry in list(layout, "overrides") {
        let item = entry
            .get("item")
            .and_then(Value::as_str)
            .filter(|i| !i.is_empty())
            .with_context(|| {
                format!("[malformed-resource] page {id} has an override without an item")
            })?;
        ensure!(
            overridden.insert(item),
            "[invalid-input] page {id} overrides item {item} twice"
        );
        let properties = entry
            .get("properties")
            .and_then(Value::as_object)
            .filter(|p| !p.is_empty())
            .with_context(|| {
                format!(
                    "[malformed-resource] page {id} override of {item} needs a properties object"
                )
            })?;
        ensure!(
            !properties.contains_key("id") && !properties.contains_key("kind"),
            "[invalid-input] page {id} override of {item} cannot change id or kind"
        );
    }
    let mut detached = HashSet::new();
    for entry in list(layout, "detached") {
        let item = entry.as_str().with_context(|| {
            format!("[malformed-resource] page {id} detached must list item IDs")
        })?;
        ensure!(
            detached.insert(item),
            "[invalid-input] page {id} lists detached item {item} twice"
        );
        ensure!(
            !overridden.contains(item),
            "[invalid-input] page {id} item {item} is both overridden and detached"
        );
    }
    Ok(())
}

/// Parent layers of a v9 document, for node validation by the scene validator.
pub(crate) fn parent_layers(raw: &Value) -> Vec<(&str, &Value)> {
    let Some(layout) = raw.get("layout") else {
        return Vec::new();
    };
    list(layout, "parents")
        .iter()
        .flat_map(|parent| {
            let id = parent.get("id").and_then(Value::as_str).unwrap_or_default();
            list(parent, "layers").iter().map(move |layer| (id, layer))
        })
        .collect()
}

// --------------------------------------------------------------- numbering

/// One entry per page in file order: absolute index (1-based), owning section,
/// computed number, label and the neighbouring labels used by the
/// `previous` and `next` page-number markers.
pub fn page_labels(raw: &Value) -> Vec<Value> {
    let page_list = pages(raw);
    let sections = raw.get("layout").map_or(&[][..], |l| list(l, "sections"));
    let mut starts: HashMap<&str, &Value> = HashMap::new();
    for section in sections {
        if let Some(start) = section.get("start_page").and_then(Value::as_str) {
            starts.insert(start, section);
        }
    }
    let mut entries: Vec<(String, Option<String>, u64, String)> = Vec::new();
    let mut active: Option<&Value> = None;
    let mut previous: Option<u64> = None;
    for (index, page) in page_list.iter().enumerate() {
        let mut number = previous.map_or(index as u64 + 1, |n| n + 1);
        if let Some(section) = starts.get(page_id(page)) {
            active = Some(section);
            let continues = section["continue"].as_bool().unwrap_or(false);
            number = match (continues, previous) {
                (true, Some(n)) => n + 1,
                _ => section["start_number"].as_u64().unwrap_or(1),
            };
        }
        let (style, prefix, section_id) = active.map_or(("arabic", "", None), |s| {
            (
                s["style"].as_str().unwrap_or("arabic"),
                s["prefix"].as_str().unwrap_or(""),
                s["id"].as_str().map(str::to_string),
            )
        });
        entries.push((
            page_id(page).to_string(),
            section_id,
            number,
            format!("{prefix}{}", format_number(style, number)),
        ));
        previous = Some(number);
    }
    (0..entries.len())
        .map(|i| {
            let (page, section, number, label) = &entries[i];
            json!({
                "page": page,
                "index": i + 1,
                "section": section,
                "number": number,
                "label": label,
                "previous": if i == 0 { "" } else { entries[i - 1].3.as_str() },
                "next": entries.get(i + 1).map_or("", |e| e.3.as_str()),
            })
        })
        .collect()
}

fn spread_of<'a>(raw: &'a Value, page: &str) -> Option<(&'a Value, usize)> {
    let layout = raw.get("layout")?;
    list(layout, "spreads").iter().find_map(|spread| {
        list(spread, "pages")
            .iter()
            .position(|p| p.as_str() == Some(page))
            .map(|position| (spread, position))
    })
}

// ------------------------------------------------------ parent resolution

fn find_parent<'a>(raw: &'a Value, id: &str) -> Option<&'a Value> {
    raw.get("layout")
        .and_then(|l| list(l, "parents").iter().find(|p| p["id"] == id))
}

/// Parent chain from the root ancestor down to `id`. Validated documents have
/// no cycles, but the walk is still bounded.
fn parent_chain<'a>(raw: &'a Value, id: &str) -> Vec<&'a Value> {
    let mut chain = Vec::new();
    let mut cursor = find_parent(raw, id);
    while let Some(parent) = cursor {
        chain.push(parent);
        if chain.len() > MAX_PARENT_DEPTH {
            break;
        }
        cursor = parent
            .get("based_on")
            .and_then(Value::as_str)
            .and_then(|base| find_parent(raw, base));
    }
    chain.reverse();
    chain
}

/// Resolve what a page receives from its parent chain and its own settings.
pub fn resolve_page(raw: &Value, page: &str) -> Result<Value> {
    let index = page_index(raw, page)?;
    let page_value = &pages(raw)[index];
    let page_layout = page_value.get("layout").unwrap_or(&Value::Null);
    let parent_id = page_layout.get("parent").and_then(Value::as_str);

    let (facing, position, count) = match spread_of(raw, page) {
        Some((spread, position)) => (
            spread["facing"].as_bool().unwrap_or(false),
            position,
            list(spread, "pages").len(),
        ),
        None => (false, 0, 1),
    };
    let side = match (facing, count, position % 2) {
        (false, _, _) => "single",
        (true, 1, _) | (true, _, 1) => "right",
        (true, _, _) => "left",
    };

    let chain = parent_id.map_or_else(Vec::new, |id| parent_chain(raw, id));

    let mut margins = None;
    let mut columns = None;
    let mut gutter = None;
    let defaults = raw
        .get("layout")
        .and_then(|l| l.get("page_defaults"))
        .unwrap_or(&Value::Null);
    let sources = std::iter::once(page_layout)
        .chain(chain.iter().rev().copied())
        .chain(std::iter::once(defaults));
    for source in sources {
        margins = margins.or_else(|| source.get("margins").cloned());
        columns = columns.or_else(|| source.get("columns").cloned());
        gutter = gutter.or_else(|| source.get("gutter").cloned());
    }
    let margins = margins.unwrap_or_else(|| json!([0, 0, 0, 0]));
    let values: Vec<f64> = margins
        .as_array()
        .map(|m| m.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default();
    let [top, a, bottom, b] = [values[0], values[1], values[2], values[3]];
    // Stored [top, inner, bottom, outer] when facing, else [top, left, bottom, right].
    let absolute = if facing {
        if side == "left" {
            [top, b, bottom, a]
        } else {
            [top, a, bottom, b]
        }
    } else {
        [top, a, bottom, b]
    };

    // Items: base parents first; a derived parent replaces an item with the
    // same ID in place.
    let mut items: Vec<(String, String, Value)> = Vec::new();
    for parent in &chain {
        let owner = parent["id"].as_str().unwrap_or_default();
        let parent_pages = list(parent, "pages");
        let chosen = if parent_pages.is_empty() {
            None
        } else {
            parent_pages[position.min(parent_pages.len() - 1)].as_str()
        };
        for layer in list(parent, "layers") {
            let layer_id = layer["id"].as_str().unwrap_or_default();
            for node in list(layer, "nodes") {
                if let (Some(only), Some(chosen)) = (node["parent_page"].as_str(), chosen) {
                    if only != chosen {
                        continue;
                    }
                }
                let id = node["id"].as_str().unwrap_or_default().to_string();
                let entry = (owner.to_string(), layer_id.to_string(), node.clone());
                match items.iter_mut().find(|(_, _, n)| n["id"] == id.as_str()) {
                    Some(slot) => *slot = entry,
                    None => items.push(entry),
                }
            }
        }
    }
    let detached: HashSet<&str> = list(page_layout, "detached")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let overrides: HashMap<&str, &Value> = list(page_layout, "overrides")
        .iter()
        .filter_map(|o| o["item"].as_str().map(|item| (item, &o["properties"])))
        .collect();
    let mut shown = Vec::new();
    let mut seen_items = HashSet::new();
    for (owner, layer, mut node) in items {
        let source = node["id"].as_str().unwrap_or_default().to_string();
        if detached.contains(source.as_str()) {
            continue;
        }
        seen_items.insert(source.clone());
        let mut changed = Vec::new();
        if let (Some(props), Some(object)) = (
            overrides.get(source.as_str()).and_then(|p| p.as_object()),
            node.as_object_mut(),
        ) {
            for (key, value) in props {
                object.insert(key.clone(), value.clone());
                changed.push(key.clone());
            }
        }
        changed.sort();
        node["id"] = json!(format!("{page}/{source}"));
        shown.push(json!({
            "id": format!("{page}/{source}"),
            "source": source,
            "parent": owner,
            "layer": layer,
            "overridden": changed,
            "node": node,
        }));
    }
    let mut orphaned: Vec<&str> = overrides
        .keys()
        .copied()
        .filter(|item| !seen_items.contains(*item))
        .collect();
    orphaned.sort_unstable();

    let mut detached_ids: Vec<&str> = detached.into_iter().collect();
    detached_ids.sort_unstable();
    Ok(json!({
        "page": page,
        "index": index + 1,
        "parent": parent_id,
        "chain": chain.iter().map(|p| p["id"].clone()).collect::<Vec<_>>(),
        "side": side,
        "margins": absolute,
        "columns": columns.unwrap_or_else(|| json!(1)),
        "gutter": gutter.unwrap_or_else(|| json!(0)),
        "items": shown,
        "orphaned_overrides": orphaned,
        "detached": detached_ids,
    }))
}

// ---------------------------------------------------------------- mutation

fn require_v9(raw: &Value) -> Result<()> {
    ensure!(
        is_v9(raw),
        "[unsupported-version] spreads, parents and sections need a version 9 document; run `pentool migrate --target 9` first"
    );
    crate::scene::validate(raw)
}

fn fresh_id(prefix: &str, taken: &HashSet<String>) -> String {
    (1..)
        .map(|n| format!("{prefix}{n}"))
        .find(|candidate| !taken.contains(candidate))
        .expect("an unused identifier exists")
}

fn spread_ids(raw: &Value) -> HashSet<String> {
    raw.get("layout")
        .map(|l| {
            list(l, "spreads")
                .iter()
                .filter_map(|s| s["id"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Make sure a v9 document has an explicit layout object with one spread per
/// page when none is recorded.
fn ensure_layout(raw: &mut Value) {
    let ids: Vec<String> = pages(raw).iter().map(|p| page_id(p).to_string()).collect();
    let object = raw.as_object_mut().expect("document is an object");
    let layout = object
        .entry("layout")
        .or_insert_with(|| json!({"engine": ENGINE}));
    if layout.get("spreads").is_none() {
        let spreads: Vec<Value> = ids
            .iter()
            .enumerate()
            .map(|(i, id)| json!({"id": format!("s{}", i + 1), "pages": [id], "facing": false}))
            .collect();
        layout["spreads"] = Value::Array(spreads);
    }
}

fn layout_mut(raw: &mut Value) -> &mut Map<String, Value> {
    ensure_layout(raw);
    raw["layout"].as_object_mut().expect("layout is an object")
}

fn array_mut<'a>(layout: &'a mut Map<String, Value>, key: &str) -> &'a mut Vec<Value> {
    layout
        .entry(key)
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .expect("layout collections are arrays")
}

/// Rebuild spreads after the page order or membership changed. `keys` maps a
/// page to the old spread it should stay grouped with; consecutive pages with
/// the same key form one spread, and a spread split in two keeps its ID on the
/// first part. Pages without a key become single-page spreads.
fn regroup(raw: &mut Value, keys: &HashMap<String, String>) {
    let old: HashMap<String, bool> = raw
        .get("layout")
        .map(|l| {
            list(l, "spreads")
                .iter()
                .filter_map(|s| {
                    s["id"]
                        .as_str()
                        .map(|id| (id.to_string(), s["facing"].as_bool().unwrap_or(false)))
                })
                .collect()
        })
        .unwrap_or_default();
    let order: Vec<String> = pages(raw).iter().map(|p| page_id(p).to_string()).collect();
    let mut groups: Vec<(Option<String>, Vec<String>)> = Vec::new();
    for page in order {
        let key = keys.get(&page).cloned();
        match groups.last_mut() {
            Some((last, members)) if key.is_some() && *last == key => members.push(page),
            _ => groups.push((key, vec![page])),
        }
    }
    let mut taken: HashSet<String> = old.keys().cloned().collect();
    let mut used: HashSet<String> = HashSet::new();
    let mut spreads = Vec::new();
    for (key, members) in groups {
        let (id, facing) = match &key {
            Some(key) if !used.contains(key) => {
                used.insert(key.clone());
                (key.clone(), old.get(key).copied().unwrap_or(false))
            }
            Some(key) => (
                fresh_id("s", &taken),
                old.get(key).copied().unwrap_or(false),
            ),
            None => (fresh_id("s", &taken), false),
        };
        taken.insert(id.clone());
        spreads.push(json!({"id": id, "pages": members, "facing": facing}));
    }
    layout_mut(raw).insert("spreads".into(), Value::Array(spreads));
}

fn key_map(raw: &Value) -> HashMap<String, String> {
    let mut keys = HashMap::new();
    if let Some(layout) = raw.get("layout") {
        for spread in list(layout, "spreads") {
            let id = spread["id"].as_str().unwrap_or_default();
            for member in list(spread, "pages") {
                if let Some(member) = member.as_str() {
                    keys.insert(member.to_string(), id.to_string());
                }
            }
        }
    }
    keys
}

fn label_map(raw: &Value) -> HashMap<String, String> {
    page_labels(raw)
        .into_iter()
        .map(|entry| {
            (
                entry["page"].as_str().unwrap_or_default().to_string(),
                format!(
                    "{}|{}",
                    entry["label"].as_str().unwrap_or_default(),
                    entry["section"].as_str().unwrap_or_default()
                ),
            )
        })
        .collect()
}

fn new_page_json(id: &str, name: &str, canvas: Value) -> Value {
    json!({
        "id": id,
        "name": name,
        "canvas": canvas,
        "layers": [{"id": "layer-1", "name": "Layer 1", "visible": true, "locked": false, "nodes": []}]
    })
}

/// `page add|rename|duplicate|move|remove` for a v9 document: the ordinary
/// page edit plus spread, section and numbering bookkeeping, with a reflow
/// report (docs/layout-v1.md section 3.6).
pub fn apply_page(raw: &mut Value, action: PageAction) -> Result<Value> {
    require_v9(raw)?;
    ensure_layout(raw);
    let before_labels = label_map(raw);
    let mut keys = key_map(raw);
    let mut added: Vec<String> = Vec::new();
    let mut removed: Vec<String> = Vec::new();
    let mut sections_changed: Vec<String> = Vec::new();
    let mut stories_reflowed = 0usize;
    let mut result;

    match action {
        PageAction::List => return Ok(crate::scene::page_list(raw)),
        PageAction::Rename { id, name } => {
            let index = page_index(raw, &id)?;
            raw["pages"][index]["name"] = json!(name);
            result = json!({"operation": "rename", "page": id});
        }
        PageAction::Add {
            id,
            name,
            width,
            height,
            background,
            after,
            before,
            parent,
            count,
        } => {
            ensure!(
                after.is_none() || before.is_none(),
                "[invalid-input] --after and --before cannot be combined"
            );
            let count = count.unwrap_or(1);
            ensure!(
                (1..=MAX_PAGES).contains(&count),
                "[limit-exceeded] pages added per operation: {count} (allowed 1-{MAX_PAGES})"
            );
            ensure!(
                pages(raw).len() + count <= MAX_PAGES,
                "[limit-exceeded] pages per document: {} exceeds the limit of {MAX_PAGES}",
                pages(raw).len() + count
            );
            if let Some(parent) = &parent {
                ensure!(
                    find_parent(raw, parent).is_some(),
                    "[missing-resource] parent not found: {parent}"
                );
            }
            let insert_at = match (&after, &before) {
                (Some(page), _) => page_index(raw, page)? + 1,
                (_, Some(page)) => page_index(raw, page)?,
                _ => pages(raw).len(),
            };
            // New pages copy the canvas of the page they follow (or precede).
            let neighbour = insert_at.saturating_sub(1).min(pages(raw).len() - 1);
            let mut canvas = pages(raw)[neighbour]["canvas"].clone();
            if let Some(width) = width {
                canvas["width"] = json!(width);
            }
            if let Some(height) = height {
                canvas["height"] = json!(height);
            }
            if let Some(background) = background {
                canvas["background"] = json!(background);
            }
            let taken: HashSet<String> =
                pages(raw).iter().map(|p| page_id(p).to_string()).collect();
            let label = name.unwrap_or_else(|| id.clone());
            for n in 0..count {
                let page_id = if n == 0 {
                    id.clone()
                } else {
                    format!("{id}-{}", n + 1)
                };
                check_id("page", &page_id)?;
                ensure!(
                    !taken.contains(&page_id) && !added.contains(&page_id),
                    "[invalid-input] page ID already exists: {page_id}"
                );
                let page_name = if n == 0 {
                    label.clone()
                } else {
                    format!("{label} {}", n + 1)
                };
                let mut page = new_page_json(&page_id, &page_name, canvas.clone());
                if let Some(parent) = &parent {
                    page["layout"] = json!({"parent": parent});
                }
                raw["pages"]
                    .as_array_mut()
                    .expect("pages array")
                    .insert(insert_at + n, page);
                added.push(page_id);
            }
            result = json!({"operation": "add", "page": id, "pages": added, "index": insert_at});
        }
        PageAction::Duplicate { id, new_id } => {
            check_id("page", &new_id)?;
            ensure!(
                pages(raw).len() < MAX_PAGES,
                "[limit-exceeded] pages per document: exceeds the limit of {MAX_PAGES}"
            );
            ensure!(
                page_index(raw, &new_id).is_err(),
                "[invalid-input] page ID already exists: {new_id}"
            );
            let index = page_index(raw, &id)?;
            let mut copy = pages(raw)[index].clone();
            let name = copy["name"].as_str().unwrap_or(&id).to_string();
            copy["id"] = json!(new_id);
            copy["name"] = json!(format!("{name} copy"));
            raw["pages"]
                .as_array_mut()
                .expect("pages array")
                .insert(index + 1, copy);
            added.push(new_id.clone());
            let copied = crate::stories::duplicate_page_frames(raw, &new_id)?;
            stories_reflowed = copied["duplicated_frames"].as_array().map_or(0, Vec::len);
            result = json!({"operation": "duplicate", "page": id, "new_id": new_id});
            result["frames"] = copied;
        }
        PageAction::Move { id, index } => {
            ensure!(
                index < pages(raw).len(),
                "[invalid-input] page index out of range"
            );
            let old = page_index(raw, &id)?;
            let list = raw["pages"].as_array_mut().expect("pages array");
            let page = list.remove(old);
            list.insert(index, page);
            // The moved page leaves its spread and forms its own.
            keys.remove(&id);
            result = json!({"operation": "move", "page": id, "index": index});
        }
        PageAction::Remove { id, drop_story } => {
            ensure!(
                pages(raw).len() > 1,
                "[invalid-input] cannot remove the last page"
            );
            let index = page_index(raw, &id)?;
            let frames_report = crate::stories::detach_page_frames(raw, &id, drop_story)?;
            stories_reflowed = frames_report["rethreaded"].as_array().map_or(0, Vec::len)
                + frames_report["stories_dropped"]
                    .as_array()
                    .map_or(0, Vec::len);
            let successor = pages(raw).get(index + 1).map(|p| page_id(p).to_string());
            raw["pages"]
                .as_array_mut()
                .expect("pages array")
                .remove(index);
            removed.push(id.clone());
            keys.remove(&id);
            // A section that started on the removed page moves to the page
            // that now follows, unless that page already starts one.
            let starts: HashSet<String> = raw["layout"]["sections"]
                .as_array()
                .map(|s| {
                    s.iter()
                        .filter_map(|x| x["start_page"].as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let sections = array_mut(layout_mut(raw), "sections");
            let mut drop = Vec::new();
            for (i, section) in sections.iter_mut().enumerate() {
                if section["start_page"] == id.as_str() {
                    let section_id = section["id"].as_str().unwrap_or_default().to_string();
                    match &successor {
                        Some(next) if !starts.contains(next) => {
                            section["start_page"] = json!(next);
                        }
                        _ => drop.push(i),
                    }
                    sections_changed.push(section_id);
                }
            }
            for i in drop.into_iter().rev() {
                sections.remove(i);
            }
            let active = pages(raw)[index.min(pages(raw).len() - 1)]["id"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            result = json!({"operation": "remove", "page": id, "active_page": active});
            result["frames"] = frames_report;
        }
    }

    regroup(raw, &keys);
    sort_sections(raw);
    crate::scene::validate(raw)?;

    let after_labels = label_map(raw);
    let renumbered = after_labels
        .iter()
        .filter(|(page, label)| before_labels.get(*page).is_some_and(|b| b != *label))
        .count();
    if let Some(layout) = raw.get("layout") {
        result["spreads"] = json!(list(layout, "spreads").len());
    }
    let (reflowed, overset) = crate::stories::reflow_counts(raw, stories_reflowed);
    result["reflow"] = json!({
        "pages_added": added,
        "pages_removed": removed,
        "pages_renumbered": renumbered,
        "sections_changed": sections_changed,
        "stories_reflowed": reflowed,
        "frames_overset": overset,
    });
    Ok(result)
}

fn sort_sections(raw: &mut Value) {
    let order: HashMap<String, usize> = pages(raw)
        .iter()
        .enumerate()
        .map(|(i, p)| (page_id(p).to_string(), i))
        .collect();
    if let Some(sections) = raw
        .get_mut("layout")
        .and_then(|l| l.get_mut("sections"))
        .and_then(Value::as_array_mut)
    {
        sections.sort_by_key(|s| {
            s["start_page"]
                .as_str()
                .and_then(|p| order.get(p).copied())
                .unwrap_or(usize::MAX)
        });
    }
}

// -------------------------------------------------------------- spread ops

/// Group the listed adjacent pages into one spread.
pub fn spread_merge(raw: &mut Value, members: &[String], facing: Option<bool>) -> Result<Value> {
    require_v9(raw)?;
    ensure_layout(raw);
    ensure!(
        (2..=MAX_PAGES_PER_SPREAD).contains(&members.len()),
        "[invalid-input] a spread takes 2-{MAX_PAGES_PER_SPREAD} pages; got {}",
        members.len()
    );
    let first = page_index(raw, &members[0])?;
    for (offset, member) in members.iter().enumerate() {
        ensure!(
            page_index(raw, member)? == first + offset,
            "[invalid-input] spread pages must be adjacent and in page order; {member} is out of place"
        );
    }
    let mut keys = key_map(raw);
    let key = fresh_id("s", &spread_ids(raw));
    for member in members {
        keys.insert(member.clone(), key.clone());
    }
    regroup(raw, &keys);
    if let Some(facing) = facing {
        set_facing(raw, &key, facing)?;
    }
    crate::scene::validate(raw)?;
    Ok(json!({"operation": "spread-merge", "spread": key, "pages": members}))
}

/// Make every page of a spread its own single-page spread.
pub fn spread_split(raw: &mut Value, spread: &str) -> Result<Value> {
    require_v9(raw)?;
    ensure_layout(raw);
    let members: Vec<String> = raw["layout"]["spreads"]
        .as_array()
        .and_then(|s| s.iter().find(|s| s["id"] == spread))
        .map(|s| {
            list(s, "pages")
                .iter()
                .filter_map(|p| p.as_str().map(str::to_string))
                .collect()
        })
        .with_context(|| format!("[missing-resource] spread not found: {spread}"))?;
    let mut keys = key_map(raw);
    for member in members.iter().skip(1) {
        keys.remove(member);
    }
    regroup(raw, &keys);
    crate::scene::validate(raw)?;
    Ok(json!({"operation": "spread-split", "spread": spread, "pages": members}))
}

fn set_facing(raw: &mut Value, spread: &str, facing: bool) -> Result<()> {
    let spreads = raw["layout"]["spreads"]
        .as_array_mut()
        .context("document has no spreads")?;
    let entry = spreads
        .iter_mut()
        .find(|s| s["id"] == spread)
        .with_context(|| format!("[missing-resource] spread not found: {spread}"))?;
    entry["facing"] = json!(facing);
    Ok(())
}

pub fn spread_set_facing(raw: &mut Value, spread: &str, facing: bool) -> Result<Value> {
    require_v9(raw)?;
    ensure_layout(raw);
    set_facing(raw, spread, facing)?;
    crate::scene::validate(raw)?;
    Ok(json!({"operation": "spread-facing", "spread": spread, "facing": facing}))
}

/// Standard book arrangement: the first page alone, then facing pairs.
/// Existing spread IDs are not preserved; this is an explicit rebuild.
pub fn spread_arrange(raw: &mut Value, facing: bool) -> Result<Value> {
    require_v9(raw)?;
    ensure_layout(raw);
    let ids: Vec<String> = pages(raw).iter().map(|p| page_id(p).to_string()).collect();
    let spreads: Vec<Value> = ids
        .iter()
        .enumerate()
        .scan(0usize, |_, (i, _)| Some(i))
        .filter(|i| *i == 0 || i % 2 == 1)
        .enumerate()
        .map(|(n, start)| {
            let end = if start == 0 {
                1
            } else {
                (start + 2).min(ids.len())
            };
            json!({"id": format!("s{}", n + 1), "pages": ids[start..end], "facing": facing})
        })
        .collect();
    layout_mut(raw).insert("spreads".into(), Value::Array(spreads.clone()));
    crate::scene::validate(raw)?;
    Ok(json!({"operation": "spread-arrange", "facing": facing, "spreads": spreads.len()}))
}

// ------------------------------------------------------------ parent ops

pub struct ParentSettings {
    pub name: Option<String>,
    pub margins: Option<[f64; 4]>,
    pub columns: Option<u64>,
    pub gutter: Option<f64>,
    pub pages: Option<Vec<String>>,
}

fn apply_settings(parent: &mut Value, settings: ParentSettings) {
    if let Some(name) = settings.name {
        parent["name"] = json!(name);
    }
    if let Some(margins) = settings.margins {
        parent["margins"] = json!(margins);
    }
    if let Some(columns) = settings.columns {
        parent["columns"] = json!(columns);
    }
    if let Some(gutter) = settings.gutter {
        parent["gutter"] = json!(gutter);
    }
    if let Some(pages) = settings.pages {
        parent["pages"] = json!(pages);
    }
}

pub fn parent_create(
    raw: &mut Value,
    id: &str,
    based_on: Option<&str>,
    settings: ParentSettings,
) -> Result<Value> {
    require_v9(raw)?;
    check_id("parent", id)?;
    ensure!(
        find_parent(raw, id).is_none(),
        "[invalid-input] parent ID already exists: {id}"
    );
    if let Some(base) = based_on {
        ensure!(
            find_parent(raw, base).is_some(),
            "[missing-resource] parent not found: {base}"
        );
    }
    let mut parent = json!({"id": id, "based_on": based_on, "pages": []});
    apply_settings(&mut parent, settings);
    array_mut(layout_mut(raw), "parents").push(parent);
    crate::scene::validate(raw)?;
    Ok(json!({"operation": "parent-create", "parent": id, "based_on": based_on}))
}

pub fn parent_set(
    raw: &mut Value,
    id: &str,
    based_on: Option<Option<&str>>,
    settings: ParentSettings,
) -> Result<Value> {
    require_v9(raw)?;
    ensure!(
        find_parent(raw, id).is_some(),
        "[missing-resource] parent not found: {id}"
    );
    let parents = array_mut(layout_mut(raw), "parents");
    let parent = parents
        .iter_mut()
        .find(|p| p["id"] == id)
        .expect("parent exists");
    if let Some(base) = based_on {
        parent["based_on"] = json!(base);
    }
    apply_settings(parent, settings);
    crate::scene::validate(raw)?;
    Ok(json!({"operation": "parent-set", "parent": id}))
}

fn pages_using(raw: &Value, parent: &str) -> Vec<String> {
    pages(raw)
        .iter()
        .filter(|p| p["layout"]["parent"] == parent)
        .map(|p| page_id(p).to_string())
        .collect()
}

pub fn parent_remove(raw: &mut Value, id: &str) -> Result<Value> {
    require_v9(raw)?;
    ensure!(
        find_parent(raw, id).is_some(),
        "[missing-resource] parent not found: {id}"
    );
    let users = pages_using(raw, id);
    ensure!(
        users.is_empty(),
        "[in-use] parent {id} is used by pages {}; assign them another parent first (`page parent <page> --parent <id>` or `--none`)",
        users.join(", ")
    );
    let derived: Vec<&str> = list(&raw["layout"], "parents")
        .iter()
        .filter(|p| p["based_on"] == id)
        .filter_map(|p| p["id"].as_str())
        .collect();
    ensure!(
        derived.is_empty(),
        "[in-use] parent {id} is the base of {}; re-base or remove them first",
        derived.join(", ")
    );
    array_mut(layout_mut(raw), "parents").retain(|p| p["id"] != id);
    crate::scene::validate(raw)?;
    Ok(json!({"operation": "parent-remove", "parent": id}))
}

pub fn parent_list(raw: &Value) -> Value {
    let parents = raw.get("layout").map_or(&[][..], |l| list(l, "parents"));
    json!({"parents": parents.iter().map(|p| {
        let id = p["id"].as_str().unwrap_or_default();
        json!({
            "id": id,
            "based_on": p["based_on"],
            "name": p["name"],
            "pages": p["pages"],
            "items": list(p, "layers").iter().map(|l| list(l, "nodes").len()).sum::<usize>(),
            "used_by": pages_using(raw, id),
        })
    }).collect::<Vec<_>>()})
}

/// Add an ordinary scene node to a parent's layer (created when absent).
pub fn parent_item_add(
    raw: &mut Value,
    parent: &str,
    node: Value,
    layer: Option<&str>,
    parent_page: Option<&str>,
) -> Result<Value> {
    require_v9(raw)?;
    let item = node
        .get("id")
        .and_then(Value::as_str)
        .filter(|i| !i.is_empty())
        .context("[invalid-input] the parent item needs an \"id\"")?
        .to_string();
    ensure!(
        !item.contains('/'),
        "[invalid-input] parent item IDs cannot contain '/'"
    );
    let existing = find_parent(raw, parent)
        .with_context(|| format!("[missing-resource] parent not found: {parent}"))?;
    ensure!(
        !list(existing, "layers")
            .iter()
            .flat_map(|l| list(l, "nodes"))
            .any(|n| n["id"] == item.as_str()),
        "[invalid-input] parent {parent} already has an item {item}"
    );
    if let Some(page) = parent_page {
        ensure!(
            list(existing, "pages").iter().any(|p| p == page),
            "[missing-resource] parent {parent} has no page {page}"
        );
    }
    let mut node = node;
    if let Some(page) = parent_page {
        node["parent_page"] = json!(page);
    }
    let parents = array_mut(layout_mut(raw), "parents");
    let entry = parents
        .iter_mut()
        .find(|p| p["id"] == parent)
        .expect("parent exists");
    let layers = entry
        .as_object_mut()
        .expect("parent is an object")
        .entry("layers")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .context("[malformed-resource] parent layers must be an array")?;
    let layer_id = layer.unwrap_or("layer-1");
    if !layers.iter().any(|l| l["id"] == layer_id) {
        layers.push(json!({"id": layer_id, "name": layer_id, "visible": true, "locked": false, "nodes": []}));
    }
    let target = layers
        .iter_mut()
        .find(|l| l["id"] == layer_id)
        .expect("layer exists");
    target["nodes"]
        .as_array_mut()
        .context("[malformed-resource] parent layer nodes must be an array")?
        .push(node);
    crate::scene::validate(raw)?;
    Ok(json!({"operation": "parent-item-add", "parent": parent, "item": item, "layer": layer_id}))
}

pub fn parent_item_remove(raw: &mut Value, parent: &str, item: &str) -> Result<Value> {
    require_v9(raw)?;
    ensure!(
        find_parent(raw, parent).is_some(),
        "[missing-resource] parent not found: {parent}"
    );
    let parents = array_mut(layout_mut(raw), "parents");
    let entry = parents
        .iter_mut()
        .find(|p| p["id"] == parent)
        .expect("parent exists");
    let mut removed = false;
    if let Some(layers) = entry.get_mut("layers").and_then(Value::as_array_mut) {
        for layer in layers {
            if let Some(nodes) = layer.get_mut("nodes").and_then(Value::as_array_mut) {
                let before = nodes.len();
                nodes.retain(|n| n["id"] != item);
                removed |= nodes.len() != before;
            }
        }
    }
    ensure!(
        removed,
        "[missing-resource] parent {parent} has no item {item}"
    );
    crate::scene::validate(raw)?;
    let orphaned: Vec<String> = pages(raw)
        .iter()
        .filter(|p| {
            list(&p["layout"], "overrides")
                .iter()
                .any(|o| o["item"] == item)
        })
        .map(|p| page_id(p).to_string())
        .collect();
    Ok(json!({
        "operation": "parent-item-remove",
        "parent": parent,
        "item": item,
        "orphaned_overrides_on": orphaned,
    }))
}

pub fn page_set_parent(raw: &mut Value, page: &str, parent: Option<&str>) -> Result<Value> {
    require_v9(raw)?;
    ensure_layout(raw);
    page_index(raw, page)?;
    if let Some(parent) = parent {
        ensure!(
            find_parent(raw, parent).is_some(),
            "[missing-resource] parent not found: {parent}"
        );
    }
    page_layout_mut(raw, page)?.insert("parent".into(), json!(parent));
    crate::scene::validate(raw)?;
    let resolved = resolve_page(raw, page)?;
    Ok(json!({
        "operation": "page-parent",
        "page": page,
        "parent": parent,
        "orphaned_overrides": resolved["orphaned_overrides"],
    }))
}

// --------------------------------------------------- override and detach

fn page_layout_mut<'a>(raw: &'a mut Value, page: &str) -> Result<&'a mut Map<String, Value>> {
    let index = page_index(raw, page)?;
    let entry = &mut raw["pages"][index];
    if !entry["layout"].is_object() {
        entry["layout"] = json!({});
    }
    Ok(entry["layout"].as_object_mut().expect("layout object"))
}

fn effective_item(raw: &Value, page: &str, item: &str) -> Result<Value> {
    let resolved = resolve_page(raw, page)?;
    list(&resolved, "items")
        .iter()
        .find(|i| i["source"] == item)
        .cloned()
        .with_context(|| {
            let available: Vec<&str> = list(&resolved, "items")
                .iter()
                .filter_map(|i| i["source"].as_str())
                .collect();
            format!(
                "[missing-resource] page {page} shows no parent item {item}; available: [{}]",
                available.join(", ")
            )
        })
}

pub fn override_set(
    raw: &mut Value,
    page: &str,
    item: &str,
    properties: Map<String, Value>,
) -> Result<Value> {
    require_v9(raw)?;
    ensure!(
        !properties.is_empty(),
        "[invalid-input] an override needs at least one property"
    );
    effective_item(raw, page, item)?;
    let layout = page_layout_mut(raw, page)?;
    let overrides = array_mut(layout, "overrides");
    let changed: Vec<String> = properties.keys().cloned().collect();
    match overrides.iter_mut().find(|o| o["item"] == item) {
        Some(entry) => {
            let existing = entry["properties"]
                .as_object_mut()
                .context("[malformed-resource] override properties must be an object")?;
            for (key, value) in properties {
                existing.insert(key, value);
            }
        }
        None => overrides.push(json!({"item": item, "properties": properties})),
    }
    crate::scene::validate(raw)?;
    Ok(json!({"operation": "override-set", "page": page, "item": item, "properties": changed}))
}

pub fn override_remove(
    raw: &mut Value,
    page: &str,
    item: &str,
    property: Option<&str>,
) -> Result<Value> {
    require_v9(raw)?;
    let layout = page_layout_mut(raw, page)?;
    let overrides = array_mut(layout, "overrides");
    let position = overrides
        .iter()
        .position(|o| o["item"] == item)
        .with_context(|| {
            format!("[missing-resource] page {page} has no override for item {item}")
        })?;
    match property {
        None => {
            overrides.remove(position);
        }
        Some(property) => {
            let properties = overrides[position]["properties"]
                .as_object_mut()
                .context("[malformed-resource] override properties must be an object")?;
            ensure!(
                properties.remove(property).is_some(),
                "[missing-resource] item {item} on page {page} does not override {property}"
            );
            if properties.is_empty() {
                overrides.remove(position);
            }
        }
    }
    if array_mut(page_layout_mut(raw, page)?, "overrides").is_empty() {
        page_layout_mut(raw, page)?.remove("overrides");
    }
    crate::scene::validate(raw)?;
    Ok(json!({"operation": "override-remove", "page": page, "item": item, "property": property}))
}

/// Copy a parent item (with its overrides applied) onto the page as an
/// ordinary node and stop following the parent for that item.
pub fn detach(raw: &mut Value, page: &str, item: &str) -> Result<Value> {
    require_v9(raw)?;
    let effective = effective_item(raw, page, item)?;
    let layer_id = effective["layer"].as_str().unwrap_or("layer-1").to_string();
    let mut node = effective["node"].clone();
    let index = page_index(raw, page)?;

    let mut taken = HashSet::new();
    fn collect(nodes: &[Value], taken: &mut HashSet<String>) {
        for node in nodes {
            if let Some(id) = node["id"].as_str() {
                taken.insert(id.to_string());
            }
            collect(list(node, "children"), taken);
        }
    }
    for layer in list(&raw["pages"][index], "layers") {
        collect(list(layer, "nodes"), &mut taken);
    }
    let new_id = if taken.contains(item) {
        (2..)
            .map(|n| format!("{item}-{n}"))
            .find(|candidate| !taken.contains(candidate))
            .expect("an unused identifier exists")
    } else {
        item.to_string()
    };
    node["id"] = json!(new_id);
    if let Some(object) = node.as_object_mut() {
        object.remove("parent_page");
    }

    let layers = raw["pages"][index]["layers"]
        .as_array_mut()
        .context("page has no layers")?;
    let position = layers
        .iter()
        .position(|l| l["id"] == layer_id.as_str())
        .unwrap_or(0);
    layers[position]["nodes"]
        .as_array_mut()
        .context("page layer has no nodes")?
        .push(node);

    let layout = page_layout_mut(raw, page)?;
    array_mut(layout, "overrides").retain(|o| o["item"] != item);
    if array_mut(layout, "overrides").is_empty() {
        layout.remove("overrides");
    }
    let detached = array_mut(layout, "detached");
    detached.push(json!(item));
    crate::scene::validate(raw)?;
    Ok(
        json!({"operation": "detach", "page": page, "item": item, "node": new_id, "layer": layers_id(raw, index, position)}),
    )
}

fn layers_id(raw: &Value, page: usize, layer: usize) -> Value {
    raw["pages"][page]["layers"][layer]["id"].clone()
}

// ---------------------------------------------------------------- sections

pub struct SectionSettings {
    pub start_number: Option<u64>,
    pub style: Option<String>,
    pub prefix: Option<String>,
    pub continue_numbering: Option<bool>,
}

pub fn section_add(
    raw: &mut Value,
    at: &str,
    id: Option<&str>,
    settings: SectionSettings,
) -> Result<Value> {
    require_v9(raw)?;
    page_index(raw, at)?;
    ensure_layout(raw);
    let existing: HashSet<String> = list(&raw["layout"], "sections")
        .iter()
        .filter_map(|s| s["id"].as_str().map(str::to_string))
        .collect();
    ensure!(
        !list(&raw["layout"], "sections")
            .iter()
            .any(|s| s["start_page"] == at),
        "[invalid-input] a section already starts at page {at}; use `section set`"
    );
    let id = match id {
        Some(id) => {
            check_id("section", id)?;
            ensure!(
                !existing.contains(id),
                "[invalid-input] section ID already exists: {id}"
            );
            id.to_string()
        }
        None => fresh_id("sec-", &existing),
    };
    let mut section = json!({"id": id, "start_page": at});
    fill_section(&mut section, settings);
    array_mut(layout_mut(raw), "sections").push(section);
    sort_sections(raw);
    crate::scene::validate(raw)?;
    Ok(
        json!({"operation": "section-add", "section": id, "start_page": at, "label": label_of(raw, at)}),
    )
}

fn fill_section(section: &mut Value, settings: SectionSettings) {
    if let Some(n) = settings.start_number {
        section["start_number"] = json!(n);
    }
    if let Some(style) = settings.style {
        section["style"] = json!(style);
    }
    if let Some(prefix) = settings.prefix {
        section["prefix"] = json!(prefix);
    }
    if let Some(cont) = settings.continue_numbering {
        section["continue"] = json!(cont);
    }
}

fn label_of(raw: &Value, page: &str) -> Value {
    page_labels(raw)
        .into_iter()
        .find(|e| e["page"] == page)
        .map_or(Value::Null, |e| e["label"].clone())
}

pub fn section_set(raw: &mut Value, id: &str, settings: SectionSettings) -> Result<Value> {
    require_v9(raw)?;
    let sections = array_mut(layout_mut(raw), "sections");
    let section = sections
        .iter_mut()
        .find(|s| s["id"] == id)
        .with_context(|| format!("[missing-resource] section not found: {id}"))?;
    fill_section(section, settings);
    crate::scene::validate(raw)?;
    Ok(json!({"operation": "section-set", "section": id}))
}

pub fn section_remove(raw: &mut Value, id: &str) -> Result<Value> {
    require_v9(raw)?;
    let sections = array_mut(layout_mut(raw), "sections");
    let before = sections.len();
    sections.retain(|s| s["id"] != id);
    ensure!(
        sections.len() != before,
        "[missing-resource] section not found: {id}"
    );
    crate::scene::validate(raw)?;
    Ok(json!({"operation": "section-remove", "section": id}))
}

/// Sections plus the computed numbering of every page, paginated.
pub fn section_list(raw: &Value, offset: usize, limit: usize) -> Value {
    let sections = raw.get("layout").map_or(&[][..], |l| list(l, "sections"));
    let labels = page_labels(raw);
    let matches = labels.len();
    let page: Vec<&Value> = labels.iter().skip(offset).take(limit).collect();
    json!({
        "sections": sections,
        "matches": matches,
        "returned": page.len(),
        "offset": offset,
        "limit": limit,
        "has_more": offset + page.len() < matches,
        "pages": page,
    })
}

/// Spread and numbering fields appended to `page list` rows of a v9 document.
pub fn page_info(raw: &Value, page: &str) -> Value {
    let (spread, facing) = spread_of(raw, page).map_or((Value::Null, false), |(s, _)| {
        (s["id"].clone(), s["facing"].as_bool().unwrap_or(false))
    });
    json!({
        "spread": spread,
        "facing": facing,
        "parent": raw["pages"]
            .as_array()
            .and_then(|p| p.iter().find(|p| page_id(p) == page))
            .map_or(Value::Null, |p| p["layout"]["parent"].clone()),
        "label": label_of(raw, page),
    })
}

// ------------------------------------------------------ migrate / downgrade

fn default_layout(raw: &Value) -> Value {
    let spreads: Vec<Value> = pages(raw)
        .iter()
        .enumerate()
        .map(|(i, p)| json!({"id": format!("s{}", i + 1), "pages": [page_id(p)], "facing": false}))
        .collect();
    json!({"engine": ENGINE, "spreads": spreads})
}

/// `migrate --target 9`: the v8 migration, then version 9 with one single-page
/// spread per page. Lossless; rendering is unchanged.
pub fn migrate(raw: Value) -> Result<Value> {
    if is_v9(&raw) {
        crate::scene::validate(&raw)?;
        return Ok(raw);
    }
    ensure!(
        raw.get("layout").is_none(),
        "[unsupported-capability] an existing \"layout\" extension conflicts with version 9; rename the extension before migration"
    );
    let mut raw = crate::color::catalog::migrate(raw)?;
    ensure!(
        pages(&raw).len() <= MAX_PAGES,
        "[limit-exceeded] pages per document: {} exceeds the limit of {MAX_PAGES}",
        pages(&raw).len()
    );
    raw["layout"] = default_layout(&raw);
    raw["version"] = json!(VERSION);
    crate::scene::validate(&raw)?;
    Ok(raw)
}

/// `migrate --target 8`: only a document whose layout is representable in v8
/// (one non-facing page per spread, nothing else) can go back. Anything else
/// fails naming the first construct, and nothing is written.
pub fn downgrade(mut raw: Value) -> Result<Value> {
    crate::scene::validate(&raw)?;
    ensure!(
        is_v9(&raw),
        "[invalid-input] layout downgrade requires version 9"
    );
    let refuse = |what: String| -> Result<Value> {
        bail!("[unsupported-capability] version 8 cannot represent {what}; remove it first (flattening composed layout arrives with the paragraph composer)")
    };
    if let Some(id) = crate::wrap::first_wrapped(&raw) {
        return refuse(format!("text wrap on object {id}"));
    }
    if let Some(layout) = raw.get("layout") {
        if let Some(spread) = list(layout, "spreads")
            .iter()
            .find(|s| list(s, "pages").len() > 1 || s["facing"].as_bool().unwrap_or(false))
        {
            return refuse(format!(
                "spread {} (facing or multi-page)",
                spread["id"].as_str().unwrap_or_default()
            ));
        }
        if let Some(parent) = list(layout, "parents").first() {
            return refuse(format!(
                "parent {}",
                parent["id"].as_str().unwrap_or_default()
            ));
        }
        if let Some(section) = list(layout, "sections").first() {
            return refuse(format!(
                "section {}",
                section["id"].as_str().unwrap_or_default()
            ));
        }
        if let Some(story) = list(layout, "stories").first() {
            return refuse(format!(
                "story {}",
                story["id"].as_str().unwrap_or_default()
            ));
        }
        let known = ["engine", "spreads", "parents", "sections", "stories"];
        if let Some(key) = layout
            .as_object()
            .and_then(|o| {
                o.iter()
                    .find(|(k, v)| !known.contains(&k.as_str()) && !is_empty(v))
            })
            .map(|(k, _)| k.clone())
        {
            return refuse(format!("layout.{key}"));
        }
    }
    for page in pages(&raw) {
        if let Some(layout) = page.get("layout").filter(|l| !is_empty(l)) {
            let key = layout
                .as_object()
                .and_then(|o| o.keys().next().cloned())
                .unwrap_or_default();
            return refuse(format!("page {} layout.{key}", page_id(page)));
        }
    }
    let object = raw.as_object_mut().context("document must be an object")?;
    object.remove("layout");
    for page in object
        .get_mut("pages")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        if let Some(page) = page.as_object_mut() {
            page.remove("layout");
        }
    }
    raw["version"] = json!(crate::color::VERSION);
    crate::scene::validate(&raw)?;
    Ok(raw)
}

fn is_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        _ => false,
    }
}

/// Bring a v9 document down to v8 for older migration paths, leaving other
/// versions untouched.
pub fn to_v8(raw: Value) -> Result<Value> {
    if is_v9(&raw) {
        downgrade(raw)
    } else {
        Ok(raw)
    }
}

/// Compact listing of spreads with page positions and labels.
pub fn spread_list(raw: &Value) -> Value {
    let spreads = raw.get("layout").map_or(&[][..], |l| list(l, "spreads"));
    json!({"spreads": spreads.iter().map(|s| {
        let members = list(s, "pages");
        json!({
            "id": s["id"],
            "facing": s["facing"].as_bool().unwrap_or(false),
            "pages": members,
            "labels": members.iter().filter_map(Value::as_str).map(|p| label_of(raw, p)).collect::<Vec<_>>(),
        })
    }).collect::<Vec<_>>()})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_formats_follow_the_published_vectors() {
        assert_eq!(format_number("arabic", 12), "12");
        assert_eq!(format_number("lower-roman", 1994), "mcmxciv");
        assert_eq!(format_number("upper-roman", 4), "IV");
        assert_eq!(format_number("lower-alpha", 26), "z");
        assert_eq!(format_number("lower-alpha", 27), "aa");
        assert_eq!(format_number("upper-alpha", 53), "AAA");
    }
}
