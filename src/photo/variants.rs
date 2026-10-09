//! Variants, snapshots and synchronized settings (`docs/photography-v1.md`,
//! "Photo entries, variants and snapshots" and "Synchronized settings").
//!
//! Every operation edits only `photography.photos[*]` develop objects: sources are
//! never copied, and brush-mask tiles stay shared by digest. The caller commits
//! the changed document as one transaction.
use super::catalog::{
    apply_upright, is_id, photo_nodes, profile_loader, stored_bytes, AIRLIGHT_INPUTS,
    MAX_SNAPSHOTS, MAX_VARIANTS,
};
use super::develop::GROUPS;
use super::dng::Dng;
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::Path;

/// At most this many targets in one `photo settings sync`.
pub const MAX_SYNC_TARGETS: usize = 10_000;

fn catalog(raw: &Value) -> Result<&Value> {
    raw.get("photography").context(
        "[missing-resource] the document has no photography catalog; add a photo with `raw add` first",
    )
}

fn photo_index(raw: &Value, photo_id: &str) -> Result<usize> {
    catalog(raw)?["photos"]
        .as_array()
        .into_iter()
        .flatten()
        .position(|photo| photo["id"] == photo_id)
        .with_context(|| format!("[missing-resource] photo {photo_id} is not in the catalog"))
}

fn variant_index(photo: &Value, variant_id: &str) -> Result<usize> {
    let photo_id = photo["id"].as_str().unwrap_or_default();
    photo["variants"]
        .as_array()
        .into_iter()
        .flatten()
        .position(|variant| variant["id"] == variant_id)
        .with_context(|| {
            format!("[missing-resource] variant {photo_id}/{variant_id} does not exist")
        })
}

fn snapshot_index(photo: &Value, snapshot_id: &str) -> Result<usize> {
    let photo_id = photo["id"].as_str().unwrap_or_default();
    photo
        .get("snapshots")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .position(|snapshot| snapshot["id"] == snapshot_id)
        .with_context(|| {
            format!("[missing-resource] snapshot {photo_id}/{snapshot_id} does not exist")
        })
}

fn new_id(value: &str, what: &str) -> Result<()> {
    if !is_id(value) {
        bail!("[invalid-input] {what} {value:?} must match ^[A-Za-z0-9][A-Za-z0-9._-]{{0,63}}$")
    }
    Ok(())
}

fn name(value: Option<&str>, what: &str) -> Result<Option<Value>> {
    match value {
        Some(name) if name.chars().count() > 256 => {
            bail!("[invalid-input] {what} name must be at most 256 characters")
        }
        Some(name) => Ok(Some(json!(name))),
        None => Ok(None),
    }
}

/// Split `photo[/variant]`; the variant defaults to `master`.
pub fn split_target(value: &str) -> Result<(String, String)> {
    let (photo, variant) = value.split_once('/').unwrap_or((value, "master"));
    if !is_id(photo) || !is_id(variant) {
        bail!("[invalid-input] {value:?} must be photo or photo/variant, each matching ^[A-Za-z0-9][A-Za-z0-9._-]{{0,63}}$")
    }
    Ok((photo.to_string(), variant.to_string()))
}

/// Validate the changed document, then replace the original with it.
fn finish(raw: &mut Value, changed: Value) -> Result<()> {
    crate::scene::validate(&changed)?;
    *raw = changed;
    Ok(())
}

/// Where a new variant's settings come from.
pub enum VariantSource<'a> {
    Variant(&'a str),
    Snapshot(&'a str),
}

/// `photo variant add`: a virtual copy holding a complete develop object copied
/// from a variant or a snapshot of the same photo.
pub fn add_variant(
    raw: &mut Value,
    photo_id: &str,
    variant_id: &str,
    from: VariantSource,
    variant_name: Option<&str>,
) -> Result<Value> {
    crate::scene::validate(raw)?;
    new_id(variant_id, "variant ID")?;
    let index = photo_index(raw, photo_id)?;
    let photo = &raw["photography"]["photos"][index];
    let variants = photo["variants"].as_array().unwrap();
    if variants.iter().any(|variant| variant["id"] == variant_id) {
        bail!("[conflict] variant {photo_id}/{variant_id} already exists; choose another ID or remove it first")
    }
    if variants.len() >= MAX_VARIANTS {
        bail!("[limit-exceeded] photo {photo_id} already has {MAX_VARIANTS} variants; remove one first")
    }
    let (develop, from) = match from {
        VariantSource::Variant(source) => {
            let i = variant_index(photo, source)?;
            (variants[i]["develop"].clone(), json!({"variant": source}))
        }
        VariantSource::Snapshot(source) => {
            let i = snapshot_index(photo, source)?;
            (
                photo["snapshots"][i]["develop"].clone(),
                json!({"snapshot": source}),
            )
        }
    };
    let mut variant = json!({"id": variant_id});
    if let Some(name) = name(variant_name, &format!("variant {photo_id}/{variant_id}"))? {
        variant["name"] = name;
    }
    variant["develop"] = develop;
    let mut changed = raw.clone();
    changed["photography"]["photos"][index]["variants"]
        .as_array_mut()
        .unwrap()
        .push(variant);
    finish(raw, changed)?;
    Ok(json!({"photo": photo_id, "variant": variant_id, "from": from}))
}

/// `photo variant remove`: `master`, a variant a snapshot belongs to, and a
/// variant a photo node shows are refused.
pub fn remove_variant(raw: &mut Value, photo_id: &str, variant_id: &str) -> Result<Value> {
    crate::scene::validate(raw)?;
    if variant_id == "master" {
        bail!("[invalid-input] variant {photo_id}/master cannot be removed; every photo keeps its master")
    }
    let index = photo_index(raw, photo_id)?;
    let photo = &raw["photography"]["photos"][index];
    let position = variant_index(photo, variant_id)?;
    let snapshots: Vec<&str> = photo
        .get("snapshots")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|snapshot| snapshot["variant"] == variant_id)
        .filter_map(|snapshot| snapshot["id"].as_str())
        .collect();
    if !snapshots.is_empty() {
        bail!(
            "[conflict] variant {photo_id}/{variant_id} has snapshots {}; remove them with `photo snapshot remove` first",
            snapshots.join(", ")
        )
    }
    let nodes: Vec<&str> = photo_nodes(raw)
        .into_iter()
        .filter(|node| node.get("photo") == Some(&json!(photo_id)))
        .filter(|node| node.get("variant") == Some(&json!(variant_id)))
        .map(|node| node.get("id").and_then(Value::as_str).unwrap_or("unknown"))
        .collect();
    if !nodes.is_empty() {
        bail!(
            "[conflict] variant {photo_id}/{variant_id} is shown by photo nodes {}; point them at another variant or delete them first",
            nodes.join(", ")
        )
    }
    let mut changed = raw.clone();
    changed["photography"]["photos"][index]["variants"]
        .as_array_mut()
        .unwrap()
        .remove(position);
    finish(raw, changed)?;
    Ok(json!({"photo": photo_id, "removed": {"variant": variant_id}}))
}

/// `photo variant rename`: set or clear a variant's display name.
pub fn rename_variant(
    raw: &mut Value,
    photo_id: &str,
    variant_id: &str,
    variant_name: Option<&str>,
) -> Result<Value> {
    crate::scene::validate(raw)?;
    let index = photo_index(raw, photo_id)?;
    let position = variant_index(&raw["photography"]["photos"][index], variant_id)?;
    let mut changed = raw.clone();
    let variant = changed["photography"]["photos"][index]["variants"][position]
        .as_object_mut()
        .unwrap();
    match name(variant_name, &format!("variant {photo_id}/{variant_id}"))? {
        Some(name) => {
            variant.insert("name".into(), name);
        }
        None => {
            variant.remove("name");
        }
    }
    finish(raw, changed)?;
    Ok(json!({"photo": photo_id, "variant": variant_id, "name": variant_name}))
}

/// `photo snapshot add`: an immutable copy of a variant's current settings.
pub fn add_snapshot(
    raw: &mut Value,
    photo_id: &str,
    variant_id: &str,
    snapshot_id: &str,
    snapshot_name: Option<&str>,
) -> Result<Value> {
    crate::scene::validate(raw)?;
    new_id(snapshot_id, "snapshot ID")?;
    let index = photo_index(raw, photo_id)?;
    let photo = &raw["photography"]["photos"][index];
    let position = variant_index(photo, variant_id)?;
    let snapshots = photo.get("snapshots").and_then(Value::as_array);
    if snapshots
        .into_iter()
        .flatten()
        .any(|snapshot| snapshot["id"] == snapshot_id)
    {
        bail!("[conflict] snapshot {photo_id}/{snapshot_id} already exists; choose another ID or remove it first")
    }
    if snapshots.map_or(0, Vec::len) >= MAX_SNAPSHOTS {
        bail!("[limit-exceeded] photo {photo_id} already has {MAX_SNAPSHOTS} snapshots; remove one first")
    }
    let mut snapshot = json!({"id": snapshot_id});
    if let Some(name) = name(snapshot_name, &format!("snapshot {photo_id}/{snapshot_id}"))? {
        snapshot["name"] = name;
    }
    snapshot["variant"] = json!(variant_id);
    snapshot["develop"] = photo["variants"][position]["develop"].clone();
    let mut changed = raw.clone();
    let photo = &mut changed["photography"]["photos"][index];
    if !photo.get("snapshots").is_some_and(Value::is_array) {
        photo["snapshots"] = json!([]);
    }
    photo["snapshots"].as_array_mut().unwrap().push(snapshot);
    finish(raw, changed)?;
    Ok(json!({"photo": photo_id, "variant": variant_id, "snapshot": snapshot_id}))
}

/// `photo snapshot restore`: copy a snapshot's settings back into its variant.
/// The snapshot itself is unchanged.
pub fn restore_snapshot(raw: &mut Value, photo_id: &str, snapshot_id: &str) -> Result<Value> {
    crate::scene::validate(raw)?;
    let index = photo_index(raw, photo_id)?;
    let photo = &raw["photography"]["photos"][index];
    let snapshot = &photo["snapshots"][snapshot_index(photo, snapshot_id)?];
    let variant_id = snapshot["variant"].as_str().unwrap_or_default().to_string();
    let position = variant_index(photo, &variant_id)?;
    let develop = snapshot["develop"].clone();
    let unchanged = photo["variants"][position]["develop"] == develop;
    let mut changed = raw.clone();
    changed["photography"]["photos"][index]["variants"][position]["develop"] = develop;
    finish(raw, changed)?;
    Ok(json!({
        "photo": photo_id,
        "snapshot": snapshot_id,
        "variant": variant_id,
        "changed": !unchanged,
    }))
}

/// `photo snapshot remove`.
pub fn remove_snapshot(raw: &mut Value, photo_id: &str, snapshot_id: &str) -> Result<Value> {
    crate::scene::validate(raw)?;
    let index = photo_index(raw, photo_id)?;
    let position = snapshot_index(&raw["photography"]["photos"][index], snapshot_id)?;
    let mut changed = raw.clone();
    let photo = changed["photography"]["photos"][index]
        .as_object_mut()
        .unwrap();
    let snapshots = photo["snapshots"].as_array_mut().unwrap();
    snapshots.remove(position);
    if snapshots.is_empty() {
        photo.remove("snapshots");
    }
    finish(raw, changed)?;
    Ok(json!({"photo": photo_id, "removed": {"snapshot": snapshot_id}}))
}

/// What `photo settings sync` copies.
pub struct SyncRequest<'a> {
    pub source: (&'a str, &'a str),
    pub targets: &'a [(String, String)],
    /// The groups to copy; empty means every group except `process`.
    pub groups: &'a [String],
    pub except: &'a [String],
    /// Run auto white balance, upright and auto tone again for each target.
    pub auto_per_photo: bool,
}

fn sync_groups(groups: &[String], except: &[String]) -> Result<Vec<&'static str>> {
    for group in groups.iter().chain(except) {
        if group == "process" {
            bail!("[invalid-input] group process is fixed by the engine and is never synchronized")
        }
        if !GROUPS.contains(&group.as_str()) {
            bail!(
                "[invalid-input] unknown settings group {group:?}; expected one of {}",
                GROUPS[1..].join(", ")
            )
        }
    }
    let chosen: Vec<&'static str> = GROUPS[1..]
        .iter()
        .copied()
        .filter(|group| groups.is_empty() || groups.iter().any(|g| g == group))
        .filter(|group| !except.iter().any(|g| g == group))
        .collect();
    if chosen.is_empty() {
        bail!("[invalid-input] --groups and --except leave no settings group to synchronize")
    }
    Ok(chosen)
}

/// The source facts that decide whether `raw` and `white_balance` apply.
struct Kind<'a> {
    kind: &'a str,
    camera_model: Option<&'a str>,
}

fn kind_of<'a>(catalog: &'a Value, photo: &'a Value) -> Kind<'a> {
    let asset = &catalog["assets"][photo["source"].as_str().unwrap_or_default()];
    Kind {
        kind: asset["kind"].as_str().unwrap_or("rendered"),
        camera_model: asset["raw"]["unique_camera_model"].as_str(),
    }
}

/// Why `group` does not apply to the target, if it does not.
fn skip_reason(
    group: &str,
    develop: &Value,
    from: &Kind,
    to: &Kind,
    profiles: &Value,
) -> Option<String> {
    if !matches!(group, "raw" | "white_balance") {
        return None;
    }
    if from.kind != to.kind {
        return Some(format!(
            "the source is a {} photo and the target is a {} photo",
            from.kind, to.kind
        ));
    }
    if group == "raw" {
        if let Some(digest) = develop["raw"]["camera_profile"]["profile"].as_str() {
            let profile = &profiles[digest];
            let model = profile["unique_camera_model"].as_str();
            if profile["force_model"] != true && model != to.camera_model {
                return Some(format!(
                    "camera profile {digest} is for camera {:?} but the target is {:?}",
                    model.unwrap_or_default(),
                    to.camera_model.unwrap_or_default()
                ));
            }
        }
    }
    None
}

/// `photo settings sync`: copy whole settings groups from one variant to the
/// target variants. Values copy as values and modes as modes; with
/// `auto_per_photo` each target's auto white balance, upright and auto tone are
/// analysed again. A stored dehaze airlight is a measurement of the image, so it
/// is always measured again on a raw target whose airlight inputs changed.
pub fn sync_settings(raw: &mut Value, document: &Path, request: SyncRequest) -> Result<Value> {
    crate::scene::validate(raw)?;
    let SyncRequest {
        source: (source_photo, source_variant),
        targets,
        groups,
        except,
        auto_per_photo,
    } = request;
    let groups = sync_groups(groups, except)?;
    if targets.is_empty() {
        bail!("[invalid-input] --to names no target variants")
    }
    if targets.len() > MAX_SYNC_TARGETS {
        bail!(
            "[limit-exceeded] {} targets exceed the limit of {MAX_SYNC_TARGETS} per sync",
            targets.len()
        )
    }
    let mut seen = HashSet::with_capacity(targets.len());
    for (photo, variant) in targets {
        if !seen.insert((photo.as_str(), variant.as_str())) {
            bail!("[invalid-input] target {photo}/{variant} is listed more than once")
        }
        if (photo.as_str(), variant.as_str()) == (source_photo, source_variant) {
            bail!("[invalid-input] target {photo}/{variant} is the sync source")
        }
    }
    let catalog = catalog(raw)?;
    let source_index = photo_index(raw, source_photo)?;
    let source = &catalog["photos"][source_index];
    let from = kind_of(catalog, source);
    let source_develop = &source["variants"][variant_index(source, source_variant)?]["develop"];
    // Resolve every target before any analysis runs.
    let mut located = Vec::with_capacity(targets.len());
    for (photo_id, variant_id) in targets {
        let index = photo_index(raw, photo_id)?;
        let position = variant_index(&catalog["photos"][index], variant_id)?;
        located.push((photo_id.as_str(), variant_id.as_str(), index, position));
    }
    let loader = profile_loader(document, &catalog["profiles"]);
    let load = |digest: &str| loader(digest).map(|(bytes, _)| bytes);
    let mut changed_document = raw.clone();
    let mut report = Vec::with_capacity(located.len());
    for (photo_id, variant_id, index, position) in located {
        super::check_cancelled()?;
        let photo = &catalog["photos"][index];
        let to = kind_of(catalog, photo);
        let before = photo["variants"][position]["develop"].clone();
        let mut develop = before.clone();
        let mut skipped = Vec::new();
        let mut copied = Vec::new();
        for group in &groups {
            if let Some(reason) =
                skip_reason(group, source_develop, &from, &to, &catalog["profiles"])
            {
                skipped.push(json!({"group": group, "reason": reason}));
                continue;
            }
            match source_develop.get(*group) {
                Some(value) => develop[*group] = value.clone(),
                None => {
                    develop.as_object_mut().unwrap().remove(*group);
                }
            }
            copied.push(*group);
        }
        let what = format!("target {photo_id}/{variant_id}");
        let develop_at = |document: &mut Value, develop: &Value| {
            document["photography"]["photos"][index]["variants"][position]["develop"] =
                develop.clone();
        };
        develop_at(&mut changed_document, &develop);
        crate::scene::validate(&changed_document).with_context(|| what.clone())?;
        let mut resolved = json!({});
        let dehaze = develop["presence"]["dehaze"].as_f64().unwrap_or(0.0) != 0.0
            || super::local::uses_dehaze(&develop);
        let [auto_white, auto_geometry, auto_tone] =
            ["white_balance", "geometry", "tone"].map(|group| {
                auto_per_photo && copied.contains(&group) && develop[group].get("auto").is_some()
            });
        let analyses = auto_white || auto_geometry || auto_tone;
        let airlight = dehaze
            && (develop["presence"].get("dehaze_airlight").is_none()
                || AIRLIGHT_INPUTS
                    .iter()
                    .chain(&["presence", "local"])
                    .any(|group| before.get(*group) != develop.get(*group)));
        if to.kind == "rendered" {
            if analyses {
                skipped.push(json!({"group": "auto", "reason": "a rendered photo cannot be analysed; the source's resolved values were copied"}));
            }
            if airlight {
                skipped.push(json!({"group": "presence.dehaze_airlight", "reason": "a rendered photo cannot be measured; the source's airlight was copied"}));
            }
        } else if analyses || airlight {
            let asset = &catalog["assets"][photo["source"].as_str().unwrap_or_default()];
            let bytes = stored_bytes(
                document,
                &asset["storage"],
                photo["source"].as_str().unwrap(),
            )
            .with_context(|| format!("photo {photo_id} source"))?;
            let dng = Dng::inspect(&bytes)?;
            if auto_white && develop["white_balance"]["mode"] == "temperature" {
                let spec =
                    super::profile::select(develop["raw"].get("camera_profile"), &dng, &load)?;
                if !spec.is_monochrome() {
                    let n = super::profile::suggest_neutral(&dng, &develop)?;
                    develop["white_balance"] = super::profile::suggested(&spec, n)?;
                    resolved["white_balance"] = develop["white_balance"].clone();
                }
            }
            if auto_geometry {
                if let Some(mode) = develop["geometry"]["upright"].as_str().map(str::to_string) {
                    let solved = super::pipeline::upright(
                        &dng,
                        &develop,
                        &mode,
                        develop["geometry"].get("guides"),
                        &loader,
                    )
                    .with_context(|| what.clone())?;
                    apply_upright(&mut develop, &mode, &solved);
                    resolved["geometry"] = develop["geometry"].clone();
                }
            }
            if auto_tone {
                develop["tone"] = super::pipeline::auto_tone(&dng, &develop, &loader)?;
                resolved["tone"] = develop["tone"].clone();
            }
            let measure = airlight
                || (dehaze
                    && ["white_balance", "geometry", "tone"]
                        .iter()
                        .any(|group| resolved.get(*group).is_some()));
            if measure {
                if let Some(presence) = develop.get_mut("presence").and_then(Value::as_object_mut) {
                    presence.remove("dehaze_airlight");
                }
                develop_at(&mut changed_document, &develop);
                crate::scene::validate(&changed_document).with_context(|| what.clone())?;
                let value = super::pipeline::resolve_airlight(&dng, &develop, &loader)?;
                if !develop.get("presence").is_some_and(Value::is_object) {
                    develop["presence"] = json!({});
                }
                develop["presence"]["dehaze_airlight"] = value.clone();
                resolved["dehaze_airlight"] = value;
            }
        }
        develop_at(&mut changed_document, &develop);
        let changed: Vec<&str> = GROUPS[1..]
            .iter()
            .copied()
            .filter(|group| before.get(*group) != develop.get(*group))
            .collect();
        let mut row = json!({
            "photo": photo_id,
            "variant": variant_id,
            "changed": changed,
            "skipped": skipped,
        });
        if resolved.as_object().is_some_and(|r| !r.is_empty()) {
            row["resolved"] = resolved;
        }
        report.push(row);
    }
    drop(loader);
    crate::scene::validate(&changed_document)?;
    *raw = changed_document;
    Ok(json!({
        "source": {"photo": source_photo, "variant": source_variant},
        "groups": groups,
        "auto_per_photo": auto_per_photo,
        "targets": report,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_default_to_everything_but_process() {
        let all = sync_groups(&[], &[]).unwrap();
        assert_eq!(all, GROUPS[1..].to_vec());
        let some = sync_groups(&["tone".into(), "crop".into()], &["crop".into()]).unwrap();
        assert_eq!(some, vec!["tone"]);
        assert!(sync_groups(&["process".into()], &[]).is_err());
        assert!(sync_groups(&[], &["nope".into()]).is_err());
    }

    #[test]
    fn raw_and_white_balance_skip_other_source_kinds_and_cameras() {
        let raw = Kind {
            kind: "raw",
            camera_model: Some("A"),
        };
        let other = Kind {
            kind: "raw",
            camera_model: Some("B"),
        };
        let rendered = Kind {
            kind: "rendered",
            camera_model: None,
        };
        let develop = json!({"raw": {"camera_profile": {"profile": "sha256:p"}}});
        let profiles = json!({"sha256:p": {"kind": "camera", "unique_camera_model": "A"}});
        for group in ["raw", "white_balance"] {
            assert!(skip_reason(group, &develop, &raw, &rendered, &profiles).is_some());
        }
        assert!(skip_reason("tone", &develop, &raw, &rendered, &profiles).is_none());
        assert!(skip_reason("raw", &develop, &raw, &raw, &profiles).is_none());
        let reason = skip_reason("raw", &develop, &raw, &other, &profiles).unwrap();
        assert!(reason.contains("camera profile"), "{reason}");
        assert!(skip_reason("white_balance", &develop, &raw, &other, &profiles).is_none());
        let forced = json!({"sha256:p": {"unique_camera_model": "A", "force_model": true}});
        assert!(skip_reason("raw", &develop, &raw, &other, &forced).is_none());
    }

    #[test]
    fn targets_default_to_master() {
        assert_eq!(
            split_target("hero").unwrap(),
            ("hero".into(), "master".into())
        );
        assert_eq!(split_target("hero/alt").unwrap().1, "alt");
        assert!(split_target("hero/").is_err());
        assert!(split_target("a/b/c").is_err());
    }
}
