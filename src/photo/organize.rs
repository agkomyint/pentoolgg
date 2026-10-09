//! Local organization (`docs/photography-v1.md`, "Stacks, collections and
//! search"): ratings, picks, labels, keywords, stacks, collections, search,
//! contact sheets and compare. Everything except the sheets reads document JSON
//! only and never decodes sources.
use super::catalog::is_id;
use anyhow::{bail, Context, Result};
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::path::Path;

pub const DEFAULT_LIMIT: usize = 50;
pub const MAX_LIMIT: usize = 1000;
const MAX_QUERY_CHARS: usize = 4096;
const MAX_TERMS: usize = 32;
/// Smart collections may refer to other collections this deep.
const MAX_COLLECTION_DEPTH: usize = 8;
const MAX_KEYWORDS: usize = 64;
const PICKS: [&str; 3] = ["none", "pick", "reject"];
const LABELS: [&str; 6] = ["none", "red", "yellow", "green", "blue", "purple"];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Cmp {
    Eq,
    Ge,
    Le,
}

impl Cmp {
    fn test<T: PartialOrd>(self, value: T, wanted: T) -> bool {
        match self {
            Cmp::Eq => value == wanted,
            Cmp::Ge => value >= wanted,
            Cmp::Le => value <= wanted,
        }
    }
}

#[derive(Debug)]
enum Term {
    Rating(Cmp, u64),
    Pick(String),
    Label(String),
    Keyword(String),
    Camera(String),
    Lens(String),
    Iso(Cmp, f64),
    Focal(Cmp, f64),
    Captured(Cmp, String),
    StackTop,
    HasVariants,
    HasLocal,
    Id(String),
    Collection(String),
}

/// A parsed search query: terms combined with AND.
#[derive(Debug)]
pub struct Query(Vec<Term>);

/// Split on whitespace, keeping double-quoted values together.
fn tokens(query: &str) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in query.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if quoted {
        bail!("[invalid-input] query has an unterminated quote")
    }
    if !current.is_empty() {
        out.push(current);
    }
    Ok(out)
}

/// `key>=value`, `key<=value`, `key=value` or `key:value`.
fn split_term(token: &str) -> Option<(&str, Cmp, &str)> {
    let at = token.find([':', '=', '<', '>'])?;
    let (key, rest) = token.split_at(at);
    let (cmp, value) = if let Some(v) = rest.strip_prefix(">=") {
        (Cmp::Ge, v)
    } else if let Some(v) = rest.strip_prefix("<=") {
        (Cmp::Le, v)
    } else {
        let v = rest.strip_prefix('=').or_else(|| rest.strip_prefix(':'))?;
        (Cmp::Eq, v)
    };
    Some((key, cmp, value))
}

fn is_date(value: &str) -> bool {
    let b = value.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

const TERMS: &str = "rating>=N, pick:pick|reject|none, label:COLOR, keyword:TEXT, camera:TEXT, lens:TEXT, iso>=N, focal>=N, captured>=YYYY-MM-DD, stack:top, has:variants, has:local, id:GLOB, collection:NAME";

/// Parse a search query. An empty query matches every photo.
pub fn parse(query: &str) -> Result<Query> {
    if query.chars().count() > MAX_QUERY_CHARS {
        bail!("[invalid-input] query is longer than {MAX_QUERY_CHARS} characters")
    }
    let tokens = tokens(query)?;
    if tokens.len() > MAX_TERMS {
        bail!("[invalid-input] query has more than {MAX_TERMS} terms")
    }
    let mut terms = Vec::with_capacity(tokens.len());
    for token in &tokens {
        let Some((key, cmp, value)) = split_term(token) else {
            bail!("[invalid-input] query term {token:?} is not KEY:VALUE; expected one of {TERMS}")
        };
        let key = key.to_lowercase();
        let ordered = matches!(key.as_str(), "rating" | "iso" | "focal" | "captured");
        if cmp != Cmp::Eq && !ordered {
            bail!("[invalid-input] query term {token:?}: {key} takes only ':'")
        }
        if value.is_empty() {
            bail!("[invalid-input] query term {token:?} has no value")
        }
        let lower = value.to_lowercase();
        let number = || -> Result<f64> {
            value
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite())
                .with_context(|| format!("[invalid-input] query term {token:?} needs a number"))
        };
        terms.push(match key.as_str() {
            "rating" => match value.parse::<u64>() {
                Ok(n) if n <= 5 => Term::Rating(cmp, n),
                _ => bail!("[invalid-input] query term {token:?}: rating is 0–5"),
            },
            "pick" if PICKS.contains(&lower.as_str()) => Term::Pick(lower),
            "label" if LABELS.contains(&lower.as_str()) => Term::Label(lower),
            "pick" => bail!("[invalid-input] query term {token:?}: pick is pick, reject or none"),
            "label" => bail!(
                "[invalid-input] query term {token:?}: label is one of {}",
                LABELS.join(", ")
            ),
            "keyword" => Term::Keyword(lower),
            "camera" => Term::Camera(lower),
            "lens" => Term::Lens(lower),
            "iso" => Term::Iso(cmp, number()?),
            "focal" => Term::Focal(cmp, number()?),
            "captured" if is_date(value) => Term::Captured(cmp, value.to_string()),
            "captured" => {
                bail!("[invalid-input] query term {token:?}: captured needs YYYY-MM-DD")
            }
            "stack" if lower == "top" => Term::StackTop,
            "has" if lower == "variants" => Term::HasVariants,
            "has" if lower == "local" => Term::HasLocal,
            "stack" | "has" => bail!(
                "[invalid-input] query term {token:?}: use stack:top, has:variants or has:local"
            ),
            "id" => Term::Id(lower),
            "collection" => Term::Collection(value.to_string()),
            _ => bail!("[invalid-input] unknown query term {token:?}; expected one of {TERMS}"),
        });
    }
    Ok(Query(terms))
}

/// `*` matches any run of characters and `?` one character.
fn glob(pattern: &[char], text: &[char]) -> bool {
    let (mut p, mut t) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            mark = t;
            p += 1;
        } else if let Some(s) = star {
            p = s + 1;
            mark += 1;
            t = mark;
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|c| *c == '*')
}

fn catalog(raw: &Value) -> Result<&Value> {
    raw.get("photography").context(
        "[missing-resource] the document has no photography catalog; add a photo with `raw add` first",
    )
}

fn photos(catalog: &Value) -> &[Value] {
    catalog["photos"].as_array().map_or(&[], Vec::as_slice)
}

/// Collection members, resolving smart collections recursively.
fn members(catalog: &Value, name: &str, stack: &mut Vec<String>) -> Result<HashSet<String>> {
    let collection = catalog["collections"].get(name).with_context(|| {
        format!("[missing-resource] collection {name} does not exist; add it with `photo collection add`")
    })?;
    if stack.iter().any(|n| n == name) {
        bail!(
            "[invalid-input] smart collection {name} refers to itself through {}",
            stack.join(" -> ")
        )
    }
    if stack.len() >= MAX_COLLECTION_DEPTH {
        bail!("[limit-exceeded] smart collections nest deeper than {MAX_COLLECTION_DEPTH}")
    }
    match collection["kind"].as_str() {
        Some("smart") => {
            stack.push(name.to_string());
            let query = parse(collection["query"].as_str().unwrap_or_default())
                .with_context(|| format!("collection {name}"))?;
            let ids = select_with(catalog, &query, stack)?;
            stack.pop();
            Ok(ids.into_iter().collect())
        }
        _ => Ok(collection["photos"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|id| id.as_str().map(str::to_string))
            .collect()),
    }
}

fn select_with(catalog: &Value, query: &Query, stack: &mut Vec<String>) -> Result<Vec<String>> {
    let tops: HashSet<&str> = catalog["stacks"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(_, stack)| stack["photos"][0].as_str())
        .collect();
    let mut sets = Vec::new();
    for term in &query.0 {
        if let Term::Collection(name) = term {
            sets.push(members(catalog, name, stack)?);
        }
    }
    let mut out = Vec::new();
    for photo in photos(catalog) {
        super::check_cancelled()?;
        let id = photo["id"].as_str().unwrap_or_default();
        let asset = &catalog["assets"][photo["source"].as_str().unwrap_or_default()];
        let capture = &asset["capture"];
        let text = |value: &Value| value.as_str().unwrap_or_default().to_lowercase();
        let mut sets = sets.iter();
        let matched = query.0.iter().all(|term| match term {
            Term::Rating(cmp, n) => cmp.test(photo["rating"].as_u64().unwrap_or(0), *n),
            Term::Pick(v) => photo["pick"].as_str().unwrap_or("none") == v,
            Term::Label(v) => photo["label"].as_str().unwrap_or("none") == v,
            Term::Keyword(v) => photo["keywords"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|k| text(k) == *v),
            Term::Camera(v) => format!(
                "{} {}\n{}",
                text(&capture["make"]),
                text(&capture["model"]),
                text(&asset["raw"]["unique_camera_model"])
            )
            .contains(v.as_str()),
            Term::Lens(v) => text(&capture["lens"]).contains(v.as_str()),
            Term::Iso(cmp, n) => capture["iso"].as_f64().is_some_and(|iso| cmp.test(iso, *n)),
            Term::Focal(cmp, n) => capture["focal_length"]
                .as_f64()
                .is_some_and(|f| cmp.test(f, *n)),
            Term::Captured(cmp, date) => capture["captured"]
                .as_str()
                .and_then(|c| c.get(..10))
                .is_some_and(|c| cmp.test(c, date.as_str())),
            Term::StackTop => tops.contains(id),
            Term::HasVariants => photo["variants"].as_array().is_some_and(|v| v.len() > 1),
            Term::HasLocal => photo["variants"].as_array().into_iter().flatten().any(|v| {
                v["develop"]["local"]
                    .as_array()
                    .is_some_and(|l| !l.is_empty())
            }),
            Term::Id(pattern) => {
                let pattern: Vec<char> = pattern.chars().collect();
                let text: Vec<char> = id.to_lowercase().chars().collect();
                glob(&pattern, &text)
            }
            Term::Collection(_) => sets.next().is_some_and(|set| set.contains(id)),
        });
        if matched {
            out.push(id.to_string());
        }
    }
    Ok(out)
}

/// The IDs of the photos matching `query`, in catalog order.
pub fn select(raw: &Value, query: &str) -> Result<Vec<String>> {
    let query = parse(query)?;
    select_with(catalog(raw)?, &query, &mut Vec::new())
}

/// Check that a smart collection query parses and resolves without cycles.
pub fn check_query(raw: &Value, query: &str) -> Result<()> {
    select(raw, query).map(|_| ())
}

/// `photo search`: compact rows in catalog order, paginated.
pub fn search(raw: &Value, query: &str, limit: usize, offset: usize) -> Result<Value> {
    crate::scene::validate(raw)?;
    if !(1..=MAX_LIMIT).contains(&limit) {
        bail!("[invalid-input] --limit must be 1–{MAX_LIMIT}")
    }
    let catalog = catalog(raw)?;
    let ids = select(raw, query)?;
    let stacks: Vec<(&String, &Value)> = catalog["stacks"]
        .as_object()
        .into_iter()
        .flatten()
        .collect();
    let rows: Vec<Value> = ids
        .iter()
        .skip(offset)
        .take(limit)
        .map(|id| {
            let photo = photos(catalog).iter().find(|p| p["id"] == **id).unwrap();
            let asset = &catalog["assets"][photo["source"].as_str().unwrap_or_default()];
            let swap = asset["orientation"].as_u64().is_some_and(|o| o >= 5);
            let (w, h) = (&asset["pixel_width"], &asset["pixel_height"]);
            let mut row = json!({
                "id": id,
                "kind": asset["kind"],
                "width": if swap { h } else { w },
                "height": if swap { w } else { h },
                "rating": photo["rating"].as_u64().unwrap_or(0),
                "pick": photo["pick"].as_str().unwrap_or("none"),
                "label": photo["label"].as_str().unwrap_or("none"),
                "variants": photo["variants"].as_array().map_or(0, Vec::len),
            });
            if let Some((stack, _)) = stacks.iter().find(|(_, s)| {
                s["photos"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|p| p == id)
            }) {
                row["stack"] = json!(stack);
            }
            row
        })
        .collect();
    Ok(json!({
        "query": query,
        "matches": ids.len(),
        "returned": rows.len(),
        "offset": offset,
        "limit": limit,
        "has_more": offset.saturating_add(rows.len()) < ids.len(),
        "photos": rows,
    }))
}

/// Which photos an organization command applies to.
pub enum Selection {
    Ids(Vec<String>),
    Query(String),
}

impl Selection {
    /// A comma-separated ID list, or a query when the text holds a query term.
    pub fn parse(text: &str) -> Selection {
        if text.contains([':', '<', '>', '=', ' ', '*', '?']) || text.trim().is_empty() {
            Selection::Query(text.to_string())
        } else {
            Selection::Ids(text.split(',').map(|s| s.trim().to_string()).collect())
        }
    }

    /// Resolve to photo IDs in the order given (IDs) or catalog order (queries).
    pub fn resolve(&self, raw: &Value) -> Result<Vec<String>> {
        let catalog = catalog(raw)?;
        let ids = match self {
            Selection::Query(query) => select(raw, query)?,
            Selection::Ids(ids) => {
                let mut seen = HashSet::new();
                for id in ids {
                    if !is_id(id) {
                        bail!("[invalid-input] photo ID {id:?} must match ^[A-Za-z0-9][A-Za-z0-9._-]{{0,63}}$")
                    }
                    if !seen.insert(id) {
                        bail!("[invalid-input] photo {id} is listed more than once")
                    }
                    if !photos(catalog).iter().any(|p| p["id"] == *id) {
                        bail!("[missing-resource] photo {id} is not in the catalog")
                    }
                }
                ids.clone()
            }
        };
        if ids.is_empty() {
            bail!("[invalid-input] the selection matches no photos")
        }
        Ok(ids)
    }
}

fn photo_mut<'a>(raw: &'a mut Value, id: &str) -> &'a mut Map<String, Value> {
    raw["photography"]["photos"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|p| p["id"] == id)
        .unwrap()
        .as_object_mut()
        .unwrap()
}

fn finish(raw: &mut Value, changed: Value) -> Result<()> {
    crate::scene::validate(&changed)?;
    *raw = changed;
    Ok(())
}

/// What `photo rate` sets.
#[derive(Default)]
pub struct Rating {
    pub rating: Option<u64>,
    pub pick: Option<String>,
    pub label: Option<String>,
}

/// `photo rate`: set rating, pick and label. Default values are stored as absence.
pub fn rate(raw: &mut Value, selection: &Selection, rating: &Rating) -> Result<Value> {
    crate::scene::validate(raw)?;
    if rating.rating.is_none() && rating.pick.is_none() && rating.label.is_none() {
        bail!("[invalid-input] photo rate needs --rating, --pick or --label")
    }
    if rating.rating.is_some_and(|r| r > 5) {
        bail!("[invalid-input] --rating must be 0–5")
    }
    if let Some(pick) = &rating.pick {
        if !PICKS.contains(&pick.as_str()) {
            bail!("[invalid-input] --pick must be pick, reject or none")
        }
    }
    if let Some(label) = &rating.label {
        if !LABELS.contains(&label.as_str()) {
            bail!(
                "[invalid-input] --label must be one of {}",
                LABELS.join(", ")
            )
        }
    }
    let ids = selection.resolve(raw)?;
    let mut changed = raw.clone();
    let mut count = 0;
    for id in &ids {
        let photo = photo_mut(&mut changed, id);
        let before = photo.clone();
        let mut put = |key: &str, value: Value, default: Value| {
            if value == default {
                photo.remove(key);
            } else {
                photo.insert(key.into(), value);
            }
        };
        if let Some(r) = rating.rating {
            put("rating", json!(r), json!(0));
        }
        if let Some(p) = &rating.pick {
            put("pick", json!(p), json!("none"));
        }
        if let Some(l) = &rating.label {
            put("label", json!(l), json!("none"));
        }
        count += usize::from(*photo != before);
    }
    finish(raw, changed)?;
    Ok(json!({"photos": ids, "changed": count}))
}

fn keyword(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 64 || value.chars().any(char::is_control) {
        bail!(
            "[invalid-input] keyword {value:?} must be 1–64 characters without control characters"
        )
    }
    Ok(value.to_string())
}

/// `photo keyword`: add or remove keywords; comparison ignores case, and an
/// added keyword that already exists in another case keeps the stored spelling.
pub fn keywords(
    raw: &mut Value,
    selection: &Selection,
    add: &[String],
    remove: &[String],
    clear: bool,
) -> Result<Value> {
    crate::scene::validate(raw)?;
    if add.is_empty() && remove.is_empty() && !clear {
        bail!("[invalid-input] photo keyword needs --add, --remove or --clear")
    }
    let add = add.iter().map(|k| keyword(k)).collect::<Result<Vec<_>>>()?;
    let remove: HashSet<String> = remove
        .iter()
        .map(|k| keyword(k).map(|k| k.to_lowercase()))
        .collect::<Result<_>>()?;
    let ids = selection.resolve(raw)?;
    let mut changed = raw.clone();
    let mut count = 0;
    for id in &ids {
        let photo = photo_mut(&mut changed, id);
        let before: Vec<Value> = photo
            .get("keywords")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut list: Vec<String> = if clear {
            Vec::new()
        } else {
            before
                .iter()
                .filter_map(Value::as_str)
                .filter(|k| !remove.contains(&k.to_lowercase()))
                .map(str::to_string)
                .collect()
        };
        for k in &add {
            if !list.iter().any(|e| e.to_lowercase() == k.to_lowercase()) {
                list.push(k.clone());
            }
        }
        if list.len() > MAX_KEYWORDS {
            bail!("[limit-exceeded] photo {id} would have more than {MAX_KEYWORDS} keywords")
        }
        let after: Vec<Value> = list.into_iter().map(Value::String).collect();
        count += usize::from(after != before);
        if after.is_empty() {
            photo.remove("keywords");
        } else {
            photo.insert("keywords".into(), Value::Array(after));
        }
    }
    finish(raw, changed)?;
    Ok(json!({"photos": ids, "changed": count}))
}

fn new_id(value: &str, what: &str) -> Result<()> {
    if !is_id(value) {
        bail!("[invalid-input] {what} {value:?} must match ^[A-Za-z0-9][A-Za-z0-9._-]{{0,63}}$")
    }
    Ok(())
}

fn ensure_object<'a>(catalog: &'a mut Value, key: &str) -> &'a mut Map<String, Value> {
    if !catalog.get(key).is_some_and(Value::is_object) {
        catalog[key] = json!({});
    }
    catalog[key].as_object_mut().unwrap()
}

fn drop_if_empty(catalog: &mut Value, key: &str) {
    if catalog[key].as_object().is_some_and(Map::is_empty) {
        catalog.as_object_mut().unwrap().remove(key);
    }
}

/// `photo stack add`: group 2 or more photos; the first is the top.
pub fn add_stack(raw: &mut Value, stack: &str, selection: &Selection) -> Result<Value> {
    crate::scene::validate(raw)?;
    new_id(stack, "stack ID")?;
    let ids = selection.resolve(raw)?;
    if ids.len() < 2 {
        bail!("[invalid-input] a stack needs at least two photos")
    }
    let catalog = catalog(raw)?;
    if catalog["stacks"].get(stack).is_some() {
        bail!("[conflict] stack {stack} already exists; remove it first")
    }
    for (name, other) in catalog["stacks"].as_object().into_iter().flatten() {
        if let Some(id) = ids
            .iter()
            .find(|id| other["photos"].as_array().unwrap().iter().any(|p| p == *id))
        {
            bail!("[conflict] photo {id} is already in stack {name}; a photo belongs to at most one stack")
        }
    }
    let mut changed = raw.clone();
    ensure_object(&mut changed["photography"], "stacks")
        .insert(stack.into(), json!({"photos": ids}));
    finish(raw, changed)?;
    Ok(json!({"stack": stack, "photos": ids, "top": ids[0]}))
}

/// `photo stack top`: move a member to the front.
pub fn stack_top(raw: &mut Value, stack: &str, photo: &str) -> Result<Value> {
    crate::scene::validate(raw)?;
    let mut changed = raw.clone();
    let list = changed["photography"]["stacks"]
        .get_mut(stack)
        .and_then(|s| s["photos"].as_array_mut())
        .with_context(|| format!("[missing-resource] stack {stack} does not exist"))?;
    let at = list
        .iter()
        .position(|p| p == photo)
        .with_context(|| format!("[missing-resource] photo {photo} is not in stack {stack}"))?;
    let member = list.remove(at);
    list.insert(0, member);
    let photos = list.clone();
    finish(raw, changed)?;
    Ok(json!({"stack": stack, "photos": photos, "top": photo}))
}

/// `photo stack remove`: unstack; the photos stay in the catalog.
pub fn remove_stack(raw: &mut Value, stack: &str) -> Result<Value> {
    crate::scene::validate(raw)?;
    let mut changed = raw.clone();
    changed["photography"]["stacks"]
        .as_object_mut()
        .and_then(|s| s.remove(stack))
        .with_context(|| format!("[missing-resource] stack {stack} does not exist"))?;
    drop_if_empty(&mut changed["photography"], "stacks");
    finish(raw, changed)?;
    Ok(json!({"removed": {"stack": stack}}))
}

/// The contents of a collection.
pub enum CollectionSpec {
    Manual(Selection),
    Smart(String),
}

/// `photo collection add`: a manual collection freezes the selection into IDs;
/// a smart one stores its query.
pub fn add_collection(
    raw: &mut Value,
    id: &str,
    name: Option<&str>,
    spec: CollectionSpec,
) -> Result<Value> {
    crate::scene::validate(raw)?;
    new_id(id, "collection ID")?;
    if catalog(raw)?["collections"].get(id).is_some() {
        bail!("[conflict] collection {id} already exists; remove it first")
    }
    let mut collection = match &spec {
        CollectionSpec::Manual(selection) => {
            json!({"kind": "manual", "photos": selection.resolve(raw)?})
        }
        CollectionSpec::Smart(query) => json!({"kind": "smart", "query": query}),
    };
    if let Some(name) = name {
        collection["name"] = json!(name);
    }
    let mut changed = raw.clone();
    ensure_object(&mut changed["photography"], "collections").insert(id.into(), collection.clone());
    if let CollectionSpec::Smart(query) = &spec {
        check_query(&changed, query).with_context(|| format!("collection {id}"))?;
    }
    finish(raw, changed)?;
    let members = members(catalog(raw)?, id, &mut Vec::new())?.len();
    Ok(json!({"collection": id, "kind": collection["kind"], "photos": members}))
}

/// `photo collection update`: add or remove photos of a manual collection.
pub fn update_collection(
    raw: &mut Value,
    id: &str,
    add: Option<&Selection>,
    remove: Option<&Selection>,
) -> Result<Value> {
    crate::scene::validate(raw)?;
    let collection = catalog(raw)?["collections"]
        .get(id)
        .with_context(|| format!("[missing-resource] collection {id} does not exist"))?;
    if collection["kind"] != "manual" {
        bail!("[invalid-input] collection {id} is smart; its members come from its query")
    }
    if add.is_none() && remove.is_none() {
        bail!("[invalid-input] photo collection update needs --add or --remove")
    }
    let add = add.map(|s| s.resolve(raw)).transpose()?.unwrap_or_default();
    let remove: HashSet<String> = remove
        .map(|s| s.resolve(raw))
        .transpose()?
        .unwrap_or_default()
        .into_iter()
        .collect();
    let mut list: Vec<String> = collection["photos"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| p.as_str())
        .filter(|p| !remove.contains(*p))
        .map(str::to_string)
        .collect();
    for photo in add {
        if !list.contains(&photo) {
            list.push(photo);
        }
    }
    let mut changed = raw.clone();
    changed["photography"]["collections"][id]["photos"] = json!(list);
    finish(raw, changed)?;
    Ok(json!({"collection": id, "photos": list.len()}))
}

/// `photo collection remove`. A smart collection that refers to it is refused.
pub fn remove_collection(raw: &mut Value, id: &str) -> Result<Value> {
    crate::scene::validate(raw)?;
    let catalog = catalog(raw)?;
    if catalog["collections"].get(id).is_none() {
        bail!("[missing-resource] collection {id} does not exist")
    }
    for (name, other) in catalog["collections"].as_object().into_iter().flatten() {
        if name != id {
            if let Some(query) = other["query"].as_str() {
                if parse(query)?
                    .0
                    .iter()
                    .any(|t| matches!(t, Term::Collection(c) if c == id))
                {
                    bail!("[conflict] smart collection {name} refers to collection {id}; change or remove it first")
                }
            }
        }
    }
    let mut changed = raw.clone();
    changed["photography"]["collections"]
        .as_object_mut()
        .unwrap()
        .remove(id);
    drop_if_empty(&mut changed["photography"], "collections");
    finish(raw, changed)?;
    Ok(json!({"removed": {"collection": id}}))
}

/// Contact sheet and compare layout.
pub struct Sheet {
    pub columns: u32,
    pub cell: u32,
    /// A caption template with `{id}`, `{name}`, `{rating}` and `{variant}`.
    pub caption: String,
}

pub const MAX_SHEET_PHOTOS: usize = 256;
const GAP: u32 = 16;
const CAPTION_SIZE: u32 = 14;

fn caption(template: &str, photo: &Value, variant: &str) -> String {
    let text = template
        .replace("{id}", photo["id"].as_str().unwrap_or_default())
        .replace("{name}", photo["name"].as_str().unwrap_or_default())
        .replace(
            "{rating}",
            &photo["rating"].as_u64().unwrap_or(0).to_string(),
        )
        .replace("{variant}", variant);
    text.chars().take(96).collect()
}

/// An 8-bit sRGB thumbnail of a photo variant that fits in `cell` x `cell`.
fn thumbnail(
    raw: &Value,
    document: &Path,
    photo: &Value,
    variant: &str,
    cell: u32,
) -> Result<Vec<u8>> {
    let id = photo["id"].as_str().unwrap_or_default();
    let catalog = &raw["photography"];
    let source = photo["source"].as_str().unwrap_or_default();
    let asset = &catalog["assets"][source];
    let decoded = if asset["kind"] == "rendered" {
        let bytes = super::catalog::stored_bytes(document, &asset["storage"], source)?;
        image::load_from_memory(&bytes).with_context(|| {
            format!("[unsupported-capability] photo {id} source cannot be decoded for a thumbnail")
        })?
    } else {
        let developed = super::catalog::render_photo(raw, document, id, variant)?;
        let output = super::output::Output::new(None, Some(8), "perceptual", None, None)?;
        let encoded =
            super::output::encode_png(&developed.image, &developed.rendering, true, &output, 1)?;
        image::load_from_memory(&encoded.bytes)?
    };
    let thumb = decoded
        .resize(cell, cell, image::imageops::FilterType::Triangle)
        .to_rgba8();
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(thumb).write_to(&mut bytes, image::ImageFormat::Png)?;
    Ok(bytes.into_inner())
}

/// Build a contact sheet scene: one cell per `(photo, variant)` with a caption.
/// Photos whose thumbnail cannot be made get a gray cell and are reported.
pub fn sheet(
    raw: &Value,
    document: &Path,
    items: &[(String, String)],
    layout: &Sheet,
) -> Result<(Value, Value)> {
    crate::scene::validate(raw)?;
    if items.is_empty() || items.len() > MAX_SHEET_PHOTOS {
        bail!("[limit-exceeded] a sheet holds 1–{MAX_SHEET_PHOTOS} photos; narrow the selection")
    }
    if !(1..=16).contains(&layout.columns) {
        bail!("[invalid-input] --columns must be 1–16")
    }
    if !(32..=1024).contains(&layout.cell) {
        bail!("[invalid-input] --cell must be 32–1024 pixels")
    }
    let catalog = catalog(raw)?;
    let columns = layout.columns.min(items.len() as u32);
    let rows = (items.len() as u32).div_ceil(columns);
    let pitch_x = layout.cell + GAP;
    let pitch_y = layout.cell + GAP + CAPTION_SIZE + 8;
    let width = columns * pitch_x + GAP;
    let height = rows * pitch_y + GAP;
    crate::image::validate_surface(u64::from(width), u64::from(height))?;
    let mut scene = crate::composite::migrate(crate::scene::new_document(width, height))?;
    scene["name"] = json!("Contact sheet");
    let mut cells = Vec::with_capacity(items.len());
    let mut texts = Vec::with_capacity(items.len());
    for (index, (photo_id, variant)) in items.iter().enumerate() {
        super::check_cancelled()?;
        let photo = photos(catalog)
            .iter()
            .find(|p| p["id"] == *photo_id)
            .with_context(|| {
                format!("[missing-resource] photo {photo_id} is not in the catalog")
            })?;
        if !photo["variants"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|v| v["id"] == *variant)
        {
            bail!("[missing-resource] variant {photo_id}/{variant} does not exist")
        }
        let (col, row) = (index as u32 % columns, index as u32 / columns);
        let (x, y) = (
            f64::from(GAP + col * pitch_x),
            f64::from(GAP + row * pitch_y),
        );
        let cell = f64::from(layout.cell);
        let node = format!("cell-{}", index + 1);
        match thumbnail(raw, document, photo, variant, layout.cell) {
            Ok(bytes) => {
                let storage = crate::image::embedded_storage(&bytes);
                crate::image::add(
                    &mut scene,
                    None,
                    "layer-1",
                    &node,
                    &bytes,
                    storage,
                    x,
                    y,
                    cell,
                    cell,
                    crate::image::Fit::Contain,
                )?;
                cells.push(json!({"photo": photo_id, "variant": variant, "node": node}));
            }
            Err(error) => {
                scene["pages"][0]["layers"][0]["nodes"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"kind": "rect", "id": node, "x": x, "y": y, "width": cell, "height": cell, "style": {"fill": {"fallback": "#d1d5db"}}}));
                cells.push(json!({"photo": photo_id, "variant": variant, "node": node, "unavailable": format!("{error:#}")}));
            }
        }
        let content = caption(&layout.caption, photo, variant);
        let baseline = y + cell + f64::from(CAPTION_SIZE + 4);
        texts.push(content.clone());
        scene["pages"][0]["layers"][0]["nodes"]
            .as_array_mut()
            .unwrap()
            .push(json!({"kind": "text", "id": format!("caption-{}", index + 1), "content": content, "x": x, "y": baseline, "font_family": crate::fonts::DEFAULT_FAMILY, "font_size": CAPTION_SIZE, "style": {"fill": {"fallback": "#111827"}}}));
    }
    crate::scene::validate(&scene)?;
    Ok((
        scene,
        json!({"width": width, "height": height, "columns": columns, "rows": rows, "cells": cells}),
    ))
}

/// Write a sheet scene to `out` (`.png` or `.pdf`) through a temporary file.
pub fn write_sheet(scene: &Value, document: &Path, out: &Path) -> Result<()> {
    let extension = out
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    let temporary = out.with_extension("sheet.tmp");
    match extension.as_deref() {
        Some("png") => {
            let bytes = crate::composite::png(scene, document, None, 1.0)?;
            std::fs::write(&temporary, bytes)
                .with_context(|| format!("write {}", temporary.display()))?;
        }
        Some("pdf") => {
            let bytes = crate::composite::png(scene, document, None, 1.0)?;
            let lines = crate::composite::text_lines(scene, document, None)?;
            crate::pdf::write_png_pages(&[(bytes, lines)], &temporary)?;
        }
        _ => bail!("[invalid-input] --out must end in .png or .pdf"),
    }
    std::fs::rename(&temporary, out).with_context(|| format!("write {}", out.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_matches_stars_and_single_characters() {
        let g = |p: &str, t: &str| {
            glob(
                &p.chars().collect::<Vec<_>>(),
                &t.chars().collect::<Vec<_>>(),
            )
        };
        assert!(g("img-*", "img-0001"));
        assert!(g("*-0?", "a-01"));
        assert!(g("*", ""));
        assert!(!g("img-?", "img-10"));
        assert!(!g("a*b", "acbc"));
    }

    #[test]
    fn queries_split_on_spaces_outside_quotes() {
        assert_eq!(
            tokens(r#"keyword:"blue hour"  rating>=4"#).unwrap(),
            ["keyword:blue hour", "rating>=4"]
        );
        assert!(parse("").unwrap().0.is_empty());
        assert!(matches!(
            parse("rating>=4").unwrap().0[..],
            [Term::Rating(Cmp::Ge, 4)]
        ));
        assert!(parse(&"id:a ".repeat(MAX_TERMS + 1)).is_err());
        assert!(parse(&"x".repeat(MAX_QUERY_CHARS + 1)).is_err());
    }

    #[test]
    fn selections_are_ids_unless_they_hold_a_query() {
        assert!(matches!(Selection::parse("a,b"), Selection::Ids(ids) if ids == ["a", "b"]));
        assert!(matches!(Selection::parse("rating>=3"), Selection::Query(_)));
        assert!(matches!(Selection::parse(""), Selection::Query(_)));
    }
}
