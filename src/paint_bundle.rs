//! `pentool paint-bundle` and `pentool paint-diff` (v0.13.0 task 12).
//!
//! A bundle is data only: brush presets, palettes, composition guides and plan
//! templates. Importing one validates every entry with the same validators that
//! guard individual commands, edits only the document-level maps (never scene
//! nodes, so component instances, overrides and masks are untouched), and commits
//! through the shared transaction so history, dry run and revision guards apply.
//! Plan templates live in `paint_plans`; `plan-show` writes one out for
//! `paint-plan`.
use crate::{paint_guide, paint_palette, paint_plan, raster, transaction};
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

pub const MAX_PLANS: usize = 256;
const MAX_BUNDLE_BYTES: u64 = 16 << 20;
const KIND: &str = "pentool-paint-bundle";
const SECTIONS: &[(&str, &str)] = &[
    ("brushes", "brush_presets"),
    ("palettes", "paint_palettes"),
    ("guides", "paint_guides"),
    ("plans", "paint_plans"),
];

/// Human-facing singular of a bundle section name. `trim_end_matches('s')`
/// mangles `brushes` into `brushe`, so the mapping is explicit.
fn singular(name: &str) -> &str {
    match name {
        "brushes" => "brush",
        "palettes" => "palette",
        "guides" => "guide",
        "plans" => "plan",
        _ => name,
    }
}

#[derive(clap::Subcommand, Debug)]
pub enum BundleAction {
    /// Write brushes, palettes, guides and plan templates to a portable bundle file.
    Export {
        /// Bundle file to write.
        #[arg(long)]
        out: PathBuf,
        /// Comma-separated names to include per section (default: everything).
        #[arg(long, value_delimiter = ',')]
        brushes: Option<Vec<String>>,
        #[arg(long, value_delimiter = ',')]
        palettes: Option<Vec<String>>,
        #[arg(long, value_delimiter = ',')]
        guides: Option<Vec<String>>,
        #[arg(long, value_delimiter = ',')]
        plans: Option<Vec<String>>,
    },
    /// Validate and merge a bundle file into the document's paint data.
    Import {
        file: PathBuf,
        /// Overwrite entries that already exist instead of failing.
        #[arg(long)]
        replace: bool,
    },
    /// Store a validated plan template.
    PlanAdd {
        id: String,
        file: PathBuf,
        #[arg(long)]
        replace: bool,
    },
    /// Remove a stored plan template.
    PlanRemove { id: String },
    /// List stored plan templates.
    PlanList,
    /// Write a stored plan template to a file runnable by `paint-plan`.
    PlanShow {
        id: String,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// List the paint assets a package or bundle would carry, with hashes.
    Assets,
}

#[derive(clap::Args, Debug)]
pub struct DiffArgs {
    /// Earlier document, or plan file with `--plans`.
    pub before: PathBuf,
    /// Later document, or plan file with `--plans`.
    pub after: PathBuf,
    /// Compare two plan JSON files instead of two documents.
    #[arg(long)]
    pub plans: bool,
}

fn read_json(path: &Path, what: &str) -> Result<Value> {
    ensure!(
        std::fs::metadata(path)
            .with_context(|| format!("[missing-resource] cannot read {}", path.display()))?
            .len()
            <= MAX_BUNDLE_BYTES,
        "[limit-exceeded] {what} {} is larger than 16 MiB",
        path.display()
    );
    serde_json::from_slice(&std::fs::read(path)?).with_context(|| {
        format!(
            "[malformed-resource] {what} {} must be JSON",
            path.display()
        )
    })
}

fn section<'a>(raw: &'a Value, key: &str) -> Result<Option<&'a Map<String, Value>>> {
    match raw.get(key) {
        None => Ok(None),
        Some(v) => v
            .as_object()
            .map(Some)
            .with_context(|| format!("[malformed-resource] {key} must be an object")),
    }
}

fn plan_record(plan: &Value) -> Result<Value> {
    let hash = paint_plan::validate_template(plan)?;
    Ok(json!({"plan": plan, "plan_hash": hash}))
}

fn check_plan_record(name: &str, rec: &Value) -> Result<()> {
    ensure!(
        paint_guide::valid_id(name),
        "[invalid-request] plan id {name:?} must be 1-128 characters of letters, digits, '.', '_' or '-'"
    );
    let obj = rec
        .as_object()
        .with_context(|| format!("[malformed-resource] plan template {name} must be an object"))?;
    ensure!(
        obj.keys().all(|k| k == "plan" || k == "plan_hash"),
        "[malformed-resource] plan template {name} has unknown properties"
    );
    let hash = paint_plan::validate_template(obj.get("plan").unwrap_or(&Value::Null))
        .with_context(|| format!("plan template {name}"))?;
    if let Some(stored) = obj.get("plan_hash").and_then(Value::as_str) {
        ensure!(
            stored == hash,
            "[conflict] plan template {name} plan_hash does not match its plan"
        );
    }
    Ok(())
}

/// Validate the stored plan templates of a document.
pub fn validate_plans(raw: &Value) -> Result<()> {
    let Some(map) = section(raw, "paint_plans")? else {
        return Ok(());
    };
    ensure!(
        map.len() <= MAX_PLANS,
        "[limit-exceeded] at most {MAX_PLANS} plan templates"
    );
    for (name, rec) in map {
        check_plan_record(name, rec)?;
    }
    Ok(())
}

fn check_guide_record(id: &str, rec: &Value) -> Result<()> {
    ensure!(
        paint_guide::valid_id(id),
        "[invalid-request] guide id {id:?} must be 1-128 characters of letters, digits, '.', '_' or '-'"
    );
    let kind = rec["kind"].as_str().unwrap_or_default();
    paint_guide::geometry(kind, &rec["params"]).with_context(|| format!("guide {id}"))?;
    if let Some(h) = rec.get("guide_hash").and_then(Value::as_str) {
        ensure!(
            h == paint_guide::guide_hash(kind, &rec["params"])?,
            "[conflict] guide {id} guide_hash does not match its parameters"
        );
    }
    Ok(())
}

fn bundle_hash(bundle: &Value) -> Result<String> {
    let mut body = bundle.clone();
    if let Some(m) = body.as_object_mut() {
        m.remove("bundle_hash");
    }
    Ok(crate::resource::sha256(&serde_json::to_vec(&body)?))
}

fn pick(
    raw: &Value,
    key: &str,
    names: &Option<Vec<String>>,
    label: &str,
) -> Result<Map<String, Value>> {
    let source = section(raw, key)?;
    let mut out = Map::new();
    match names {
        None => {
            if let Some(map) = source {
                out = map.clone();
            }
        }
        Some(list) => {
            for name in list {
                let value = source.and_then(|m| m.get(name)).with_context(|| {
                    format!("[not-found] {label} {name:?} does not exist in this document")
                })?;
                out.insert(name.clone(), value.clone());
            }
        }
    }
    Ok(out)
}

/// Build a bundle value from selected document data.
fn export(raw: &Value, selection: [&Option<Vec<String>>; 4]) -> Result<Value> {
    let mut bundle = json!({"schema": 1, "kind": KIND, "engine": raster::ENGINE});
    for ((name, key), names) in SECTIONS.iter().zip(selection) {
        let mut map = pick(raw, key, names, singular(name))?;
        if *name == "plans" {
            for rec in map.values_mut() {
                *rec = rec["plan"].clone();
            }
        }
        bundle[*name] = Value::Object(map);
    }
    bundle["bundle_hash"] = json!(bundle_hash(&bundle)?);
    Ok(bundle)
}

fn merge(raw: &mut Value, bundle: &Value, replace: bool) -> Result<Value> {
    let obj = bundle
        .as_object()
        .context("[malformed-resource] a bundle must be a JSON object")?;
    ensure!(
        obj.get("kind").and_then(Value::as_str) == Some(KIND),
        "[invalid-request] not a Pentool paint bundle (kind must be {KIND:?})"
    );
    ensure!(
        obj.get("schema").and_then(Value::as_u64) == Some(1),
        "[unsupported-capability] bundle schema must be 1"
    );
    let engine = obj.get("engine").and_then(Value::as_u64).unwrap_or(0);
    ensure!(
        (1..=raster::ENGINE).contains(&engine),
        "[unsupported-capability] bundle needs raster engine {engine}; this build implements engine {}",
        raster::ENGINE
    );
    if let Some(key) = obj.keys().find(|k| {
        !["schema", "kind", "engine", "bundle_hash"].contains(&k.as_str())
            && !SECTIONS.iter().any(|(n, _)| n == k)
    }) {
        bail!("[invalid-request] unknown bundle field {key:?}; bundles carry data only")
    }
    if let Some(h) = obj.get("bundle_hash").and_then(Value::as_str) {
        ensure!(
            h == bundle_hash(bundle)?,
            "[conflict] bundle_hash does not match the bundle contents"
        );
    }
    let mut next = raw.clone();
    let mut report = Map::new();
    for (name, key) in SECTIONS {
        let Some(entries) = obj.get(*name) else {
            continue;
        };
        let entries = entries
            .as_object()
            .with_context(|| format!("[invalid-request] bundle {name} must be an object"))?;
        let mut added = Vec::new();
        let mut replaced = Vec::new();
        for (id, value) in entries {
            let exists = section(&next, key)?.is_some_and(|m| m.contains_key(id));
            let entry_name = match *name {
                "brushes" => "brush preset",
                "palettes" => "palette",
                "guides" => "guide",
                "plans" => "plan",
                _ => *name,
            };
            ensure!(
                replace || !exists,
                "[conflict] {} {id:?} already exists; pass --replace to overwrite it. Nothing was written",
                entry_name
            );
            let stored = match *name {
                "plans" => plan_record(value).with_context(|| format!("plan {id}"))?,
                "guides" => {
                    let mut rec = value.clone();
                    check_guide_record(id, &rec)?;
                    let kind = rec["kind"].as_str().unwrap_or_default().to_owned();
                    rec["guide_hash"] = json!(paint_guide::guide_hash(&kind, &rec["params"])?);
                    rec
                }
                _ => value.clone(),
            };
            next.as_object_mut()
                .context("[invalid-input] document must be an object")?
                .entry(*key)
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .with_context(|| format!("[malformed-resource] {key} must be an object"))?
                .insert(id.clone(), stored);
            if exists {
                replaced.push(id.clone());
            } else {
                added.push(id.clone());
            }
        }
        report.insert(
            (*name).into(),
            json!({"added": added, "replaced": replaced}),
        );
    }
    raster::validate_document(&next)?;
    paint_palette::validate(&next)?;
    if let Some(map) = section(&next, "paint_guides")? {
        ensure!(
            map.len() <= paint_guide::MAX_GUIDES,
            "[limit-exceeded] a document holds at most {} guides",
            paint_guide::MAX_GUIDES
        );
        for (id, rec) in map {
            check_guide_record(id, rec)?;
        }
    }
    validate_plans(&next)?;
    *raw = next;
    Ok(Value::Object(report))
}

fn assets(raw: &Value) -> Result<Value> {
    let mut out = Map::new();
    for (name, key) in SECTIONS {
        let mut list = Vec::new();
        if let Some(map) = section(raw, key)? {
            for (id, rec) in map {
                list.push(json!({
                    "id": id,
                    "hash": crate::resource::sha256(&serde_json::to_vec(rec)?),
                }));
            }
        }
        out.insert((*name).into(), Value::Array(list));
    }
    Ok(Value::Object(out))
}

/// Flat, validated table of a document's paint data for package manifests.
pub fn package_table(raw: &Value) -> Result<Vec<Value>> {
    if let Some(map) = section(raw, "paint_guides")? {
        for (id, rec) in map {
            check_guide_record(id, rec)?;
        }
    }
    validate_plans(raw)?;
    paint_palette::validate(raw)?;
    let mut table = Vec::new();
    for (name, key) in SECTIONS {
        if let Some(map) = section(raw, key)? {
            for (id, rec) in map {
                table.push(json!({
                    "section": name,
                    "id": id,
                    "hash": crate::resource::sha256(&serde_json::to_vec(rec)?),
                }));
            }
        }
    }
    Ok(table)
}

fn print(value: &Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

pub fn run(
    input: &Path,
    dry_run: bool,
    if_revision: Option<&str>,
    action: BundleAction,
) -> Result<()> {
    let mut raw: Value = serde_json::from_slice(
        &std::fs::read(input)
            .with_context(|| format!("[missing-resource] cannot read {}", input.display()))?,
    )
    .context("[malformed-resource] invalid .pen document")?;
    transaction::validate_value(&raw)?;
    let (mut result, operation) = match action {
        BundleAction::Export {
            out,
            brushes,
            palettes,
            guides,
            plans,
        } => {
            let bundle = export(&raw, [&brushes, &palettes, &guides, &plans])?;
            if let Some(parent) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&out, serde_json::to_vec_pretty(&bundle)?)?;
            let counts: Map<String, Value> = SECTIONS
                .iter()
                .map(|(n, _)| {
                    (
                        (*n).into(),
                        json!(bundle[*n].as_object().map_or(0, Map::len)),
                    )
                })
                .collect();
            return print(&json!({
                "exported": counts, "bundle_hash": bundle["bundle_hash"], "out": out,
            }));
        }
        BundleAction::PlanList => {
            let list: Vec<Value> = section(&raw, "paint_plans")?
                .map(|m| {
                    m.iter()
                        .map(|(id, rec)| {
                            json!({
                                "id": id,
                                "plan_hash": rec["plan_hash"],
                                "passes": rec["plan"]["passes"].as_array().map_or(0, Vec::len),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            return print(&json!({
                "matches": list.len(), "returned": list.len(), "offset": 0,
                "limit": MAX_PLANS, "has_more": false, "plans": list,
            }));
        }
        BundleAction::PlanShow { id, out } => {
            let rec = section(&raw, "paint_plans")?
                .and_then(|m| m.get(&id))
                .with_context(|| {
                    format!("[not-found] plan template {id:?} does not exist; list with `paint-bundle DOCUMENT plan-list`")
                })?;
            let plan = rec["plan"].clone();
            return match out {
                Some(path) => {
                    std::fs::write(&path, serde_json::to_vec_pretty(&plan)?)?;
                    print(&json!({"id": id, "plan_hash": rec["plan_hash"], "out": path}))
                }
                None => print(&plan),
            };
        }
        BundleAction::Assets => return print(&assets(&raw)?),
        BundleAction::Import { file, replace } => {
            let bundle = read_json(&file, "bundle")?;
            (
                json!({"imported": merge(&mut raw, &bundle, replace)?}),
                "paint-bundle-import",
            )
        }
        BundleAction::PlanAdd { id, file, replace } => {
            let plan = read_json(&file, "plan")?;
            let bundle = json!({"schema": 1, "kind": KIND, "engine": raster::ENGINE,
                                "plans": {id: plan}});
            (
                json!({"imported": merge(&mut raw, &bundle, replace)?}),
                "paint-bundle-plan-add",
            )
        }
        BundleAction::PlanRemove { id } => {
            let map = raw
                .get_mut("paint_plans")
                .and_then(Value::as_object_mut)
                .filter(|m| m.contains_key(&id))
                .with_context(|| format!("[not-found] plan template {id:?} does not exist"))?;
            map.remove(&id);
            if map.is_empty() {
                if let Some(root) = raw.as_object_mut() {
                    root.remove("paint_plans");
                }
            }
            (json!({"removed": id}), "paint-bundle-plan-remove")
        }
    };
    let change = transaction::commit_value(input, operation, dry_run, if_revision, &raw)?;
    result["dry_run"] = json!(dry_run);
    result["change"] = serde_json::to_value(&change)?;
    result["pixels_changed"] = json!(false);
    print(&result)
}

// ---------------------------------------------------------------------- diff

fn diff_maps(before: Option<&Map<String, Value>>, after: Option<&Map<String, Value>>) -> Value {
    let empty = Map::new();
    let (b, a) = (before.unwrap_or(&empty), after.unwrap_or(&empty));
    let added: Vec<&String> = a.keys().filter(|k| !b.contains_key(*k)).collect();
    let removed: Vec<&String> = b.keys().filter(|k| !a.contains_key(*k)).collect();
    let changed: Vec<&String> = b
        .iter()
        .filter(|(k, v)| a.get(*k).is_some_and(|w| w != *v))
        .map(|(k, _)| k)
        .collect();
    json!({"added": added, "removed": removed, "changed": changed})
}

fn keys_changed(before: &Value, after: &Value) -> Vec<String> {
    let mut keys: Vec<&String> = Vec::new();
    for v in [before, after] {
        if let Some(m) = v.as_object() {
            keys.extend(m.keys());
        }
    }
    keys.sort();
    keys.dedup();
    keys.into_iter()
        .filter(|k| before.get(k.as_str()) != after.get(k.as_str()))
        .cloned()
        .collect()
}

/// Per-pass and constraint differences between two plan JSON values.
pub fn diff_plans(before: &Value, after: &Value) -> Result<Value> {
    let (bh, ah) = (
        paint_plan::validate_template(before)?,
        paint_plan::validate_template(after)?,
    );
    let pass_list = |plan: &Value| -> Vec<(String, Value)> {
        plan["passes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| p["id"].as_str().map(|id| (id.to_owned(), p.clone())))
            .collect()
    };
    let (bp, ap) = (pass_list(before), pass_list(after));
    let find = |list: &[(String, Value)], id: &str| list.iter().position(|(k, _)| k == id);
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut changed = Vec::new();
    for (id, pass) in &ap {
        match find(&bp, id) {
            None => added.push(json!(id)),
            Some(i) => {
                let fields = keys_changed(&bp[i].1, pass);
                if !fields.is_empty() {
                    changed.push(json!({"id": id, "fields": fields}));
                }
            }
        }
    }
    for (id, _) in &bp {
        if find(&ap, id).is_none() {
            removed.push(json!(id));
        }
    }
    let shared = |l: &[(String, Value)], other: &[(String, Value)]| -> Vec<String> {
        l.iter()
            .filter(|(k, _)| find(other, k).is_some())
            .map(|(k, _)| k.clone())
            .collect()
    };
    Ok(json!({
        "identical": bh == ah,
        "before_hash": bh,
        "after_hash": ah,
        "passes": {
            "added": added, "removed": removed, "changed": changed,
            "reordered": shared(&bp, &ap) != shared(&ap, &bp),
        },
        "limits_changed": before.get("limits") != after.get("limits"),
        "header_changed": keys_changed(
            &json!({"engine": before["engine"], "page": before["page"]}),
            &json!({"engine": after["engine"], "page": after["page"]}),
        ),
    }))
}

fn layer_impact(before: &Value, after: &Value) -> Result<Vec<Value>> {
    let ids_b = raster::raster_ids(before);
    let ids_a = raster::raster_ids(after);
    let blank = vec![0u8; raster::TILE_BYTES];
    let mut out = Vec::new();
    for id in ids_b.iter().filter(|i| ids_a.contains(i)) {
        let (sb, sa) = (
            raster::layer_surface(before, id)?,
            raster::layer_surface(after, id)?,
        );
        let mut keys: Vec<(u32, u32)> = sb.tiles.keys().chain(sa.tiles.keys()).copied().collect();
        keys.sort_unstable();
        keys.dedup();
        let mut changed = 0u64;
        let mut bounds: Option<[u32; 4]> = None;
        for k in keys {
            let lhs = sb.tiles.get(&k).map_or(&blank[..], |t| &t[..]);
            let rhs = sa.tiles.get(&k).map_or(&blank[..], |t| &t[..]);
            if lhs != rhs {
                changed += 1;
                let t = raster::TILE as u32;
                let (x0, y0, x1, y1) = (k.0 * t, k.1 * t, (k.0 + 1) * t, (k.1 + 1) * t);
                bounds = Some(match bounds {
                    None => [x0, y0, x1, y1],
                    Some(b) => [b[0].min(x0), b[1].min(y0), b[2].max(x1), b[3].max(y1)],
                });
            }
        }
        let resized = sb.width != sa.width || sb.height != sa.height;
        if changed > 0 || resized {
            out.push(json!({
                "layer": id,
                "changed_tiles": changed,
                "tile_bounds": bounds,
                "size_changed": resized,
                "before_tile_map": sb.tile_map_hash(),
                "after_tile_map": sa.tile_map_hash(),
            }));
        }
    }
    Ok(out)
}

pub fn run_diff(args: DiffArgs) -> Result<()> {
    if args.plans {
        let b = read_json(&args.before, "plan")?;
        let a = read_json(&args.after, "plan")?;
        return print(&diff_plans(&b, &a)?);
    }
    let b = read_json(&args.before, "document")?;
    let a = read_json(&args.after, "document")?;
    transaction::validate_value(&b)?;
    transaction::validate_value(&a)?;
    let mut sections = Map::new();
    for (name, key) in SECTIONS {
        sections.insert(
            (*name).into(),
            diff_maps(section(&b, key)?, section(&a, key)?),
        );
    }
    let mut plan_changes = Vec::new();
    if let (Some(pb), Some(pa)) = (section(&b, "paint_plans")?, section(&a, "paint_plans")?) {
        for (id, rec) in pb {
            if let Some(other) = pa.get(id).filter(|o| *o != rec) {
                plan_changes
                    .push(json!({"id": id, "diff": diff_plans(&rec["plan"], &other["plan"])?}));
            }
        }
    }
    print(&json!({
        "kind": "document",
        "sections": sections,
        "plan_changes": plan_changes,
        "layers": layer_impact(&b, &a)?,
    }))
}
