//! Corruption detection and recovery for raster layers.
//!
//! The materialized tile map is authoritative. A checkpoint pins a full tile map,
//! and `journal[journal_offset..]` replays from it to the live tiles. `verify` checks
//! every stored tile against its digest and optionally proves that replay still
//! reaches the live tile map; `repair` rebuilds damaged tiles by replay or, when
//! that is impossible, drops them to transparency and records what was lost.
use super::*;

/// Rebuild a surface from a checkpoint tile map (or an empty layer) plus the journal.
pub fn replay(raw: &Value, node: &Value) -> Result<Surface> {
    let id = node["id"].as_str().unwrap_or("?");
    let (width, height) = dimensions(node)?;
    let journal = node["journal"].as_array().cloned().unwrap_or_default();
    let (mut surface, offset) = match node.get("checkpoint") {
        None => (
            Surface {
                width,
                height,
                tiles: BTreeMap::new(),
            },
            0,
        ),
        Some(checkpoint) => {
            let engine = checkpoint["engine"].as_u64().unwrap_or(1);
            if engine > ENGINE {
                bail!("[unsupported-capability] raster {id} checkpoint needs raster engine {engine}; this build implements {ENGINE}")
            }
            let Some(tiles) = checkpoint.get("tiles") else {
                bail!("[not-replayable] raster {id} checkpoint has no pinned tiles, so its journal cannot be replayed")
            };
            (
                Surface::load_map(raw, width, height, tiles)?,
                checkpoint["journal_offset"].as_u64().unwrap_or(0) as usize,
            )
        }
    };
    for (index, entry) in journal.iter().enumerate().skip(offset) {
        let engine = entry["engine"].as_u64().unwrap_or(1);
        if engine > ENGINE {
            bail!("[unsupported-capability] raster {id} journal entry {index} needs raster engine {engine}; this build implements {ENGINE}")
        }
        replay_entry(raw, &mut surface, entry)
            .with_context(|| format!("raster {id} journal entry {index} could not be replayed"))?;
        if let Some(expected) = entry["tile_map_sha256"].as_str() {
            let actual = surface.tile_map_hash();
            if actual != expected {
                bail!("[replay-mismatch] raster {id} journal entry {index} replays to {actual}, but recorded {expected}")
            }
        }
    }
    Ok(surface)
}

/// Re-apply one journal entry. Every replayable operation is dispatched here.
pub(crate) fn replay_entry(raw: &Value, surface: &mut Surface, entry: &Value) -> Result<()> {
    match entry["op"].as_str().unwrap_or("stroke") {
        "stroke" => {
            let mut brush = Brush::parse(&entry["brush"])?;
            if brush.tip.as_deref().is_some_and(|tip| !is_digest(tip)) {
                bail!("[malformed-raster] journal brush tips must be recorded as sha256: digests")
            }
            let tip = resolve_tip(raw, &mut brush)?;
            let stroke = Stroke {
                brush,
                tip,
                samples: parse_samples(&entry["samples"])?,
                color: parse_color(entry["color"].as_str().unwrap_or_default())?,
                blend: Blend::parse(entry["blend"].as_str().unwrap_or("normal"))?,
                seed: entry["seed"].as_u64().unwrap_or(0),
            };
            apply_stroke(surface, &stroke)?;
            Ok(())
        }
        other => bail!("[not-replayable] journal operation {other:?} is not replayable"),
    }
}

#[derive(Default)]
struct TileCheck {
    good: usize,
    missing: Vec<String>,
    corrupt: Vec<String>,
}

fn check_tiles(raw: &Value, tiles: &Value) -> TileCheck {
    let store = raw.get("raster_tiles").and_then(Value::as_object);
    let mut check = TileCheck::default();
    for (key, digest) in tiles.as_object().into_iter().flatten() {
        let digest = digest.as_str().unwrap_or_default();
        match store.and_then(|s| s.get(digest)) {
            None => check.missing.push(key.clone()),
            Some(entry) => match decode_tile(entry, digest) {
                Ok(_) => check.good += 1,
                Err(_) => check.corrupt.push(key.clone()),
            },
        }
    }
    check
}

fn verify_node(raw: &Value, node: &Value, with_replay: bool) -> Value {
    let live = check_tiles(raw, &node["tiles"]);
    let engine = node_engine(node).unwrap_or(0);
    let checkpoint = node.get("checkpoint").map(|checkpoint| {
        let pinned = checkpoint.get("tiles").map(|tiles| check_tiles(raw, tiles));
        json!({
            "replay_base": pinned.is_some(),
            "missing": pinned.as_ref().map_or(0, |c| c.missing.len()),
            "corrupt": pinned.as_ref().map_or(0, |c| c.corrupt.len()),
        })
    });
    let structure = validate_node(node).err().map(|e| e.to_string());
    let damaged = !live.missing.is_empty() || !live.corrupt.is_empty();
    let mut report = json!({
        "id": node["id"],
        "engine": engine,
        "editable": engine >= 1 && engine <= ENGINE,
        "tiles": live.good + live.missing.len() + live.corrupt.len(),
        "missing": live.missing,
        "corrupt": live.corrupt,
        "journal_entries": node["journal"].as_array().map_or(0, Vec::len),
        "checkpoint": checkpoint,
        "structure_error": structure,
    });
    let mut ok = !damaged && report["structure_error"].is_null();
    if with_replay {
        let outcome = match replay(raw, node) {
            Ok(surface) => match Surface::load(raw, node) {
                Ok(current) if current.tile_map_hash() == surface.tile_map_hash() => {
                    json!({"status":"match","tile_map_sha256":surface.tile_map_hash()})
                }
                Ok(current) => {
                    ok = false;
                    json!({"status":"mismatch","replayed":surface.tile_map_hash(),"current":current.tile_map_hash()})
                }
                // The live tiles are damaged but replay works: repairable.
                Err(_) => json!({"status":"rebuildable","tile_map_sha256":surface.tile_map_hash()}),
            },
            Err(error) => {
                let message = error.to_string();
                if !message.contains("[not-replayable]") {
                    ok = false;
                }
                json!({"status":"failed","error":message})
            }
        };
        report["replay"] = outcome;
    }
    report["ok"] = json!(ok);
    report
}

/// Inspect raster layers without modifying anything. With `id`, only that layer;
/// otherwise every raster on the selected page (or all pages).
pub fn verify(
    raw: &Value,
    page: Option<&str>,
    id: Option<&str>,
    with_replay: bool,
) -> Result<Value> {
    let mut layers = Vec::new();
    for (page_index, p) in raw["pages"].as_array().into_iter().flatten().enumerate() {
        if page.is_some_and(|wanted| p["id"] != wanted) {
            continue;
        }
        let mut nodes = Vec::new();
        for layer in p["layers"].as_array().into_iter().flatten() {
            if let Some(list) = layer["nodes"].as_array() {
                walk_rasters(list, &mut nodes);
            }
        }
        for node in nodes {
            if id.is_some_and(|wanted| node["id"] != wanted) {
                continue;
            }
            let mut report = verify_node(raw, node, with_replay);
            report["page"] = p.get("id").cloned().unwrap_or(json!(page_index));
            layers.push(report);
        }
    }
    if let Some(wanted) = id {
        if layers.is_empty() {
            return Err(bad(
                "not-found",
                format!("raster layer {wanted} was not found; list nodes with `pentool tree DOCUMENT --kind raster`"),
            ));
        }
    }
    let retained = retained_digests(raw);
    let orphans = raw
        .get("raster_tiles")
        .and_then(Value::as_object)
        .map_or(0, |store| {
            store.keys().filter(|d| !retained.contains(*d)).count()
        });
    let store = raw.get("raster_tiles").and_then(Value::as_object);
    let mut tips_checked = 0usize;
    let mut damaged_tips = Vec::new();
    for (name, digest) in raw
        .get("brush_tips")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        tips_checked += 1;
        let digest = digest.as_str().unwrap_or_default();
        let problem = match store.and_then(|s| s.get(digest)) {
            None => Some("missing"),
            Some(entry) => decode_tile(entry, digest).err().map(|_| "corrupt"),
        };
        if let Some(problem) = problem {
            damaged_tips.push(json!({"name": name, "tip": digest, "problem": problem}));
        }
    }
    let ok = layers.iter().all(|layer| layer["ok"] == true) && damaged_tips.is_empty();
    Ok(json!({
        "ok": ok,
        "layers": layers,
        "tips": {"checked": tips_checked, "damaged": damaged_tips},
        "orphan_tiles": orphans,
        "fix": if ok { Value::Null } else {
            json!("run `pentool raster DOCUMENT repair ID --strategy replay`, or --strategy transparent to drop unrecoverable tiles; re-add a damaged brush tip with `tip-remove` then `tip-add`")
        },
    }))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepairStrategy {
    Replay,
    Transparent,
}

impl RepairStrategy {
    pub fn parse(text: &str) -> Result<Self> {
        match text {
            "replay" => Ok(Self::Replay),
            "transparent" => Ok(Self::Transparent),
            other => bail!("[invalid-input] repair strategy {other:?} is not supported; use replay or transparent"),
        }
    }
}

/// Repair one raster layer in memory. `replay` rebuilds the exact pixels from the
/// checkpoint and journal; `transparent` keeps intact tiles, drops damaged ones and
/// rolls a new checkpoint because the old journal no longer describes the pixels.
pub fn repair(
    raw: &mut Value,
    page: Option<&str>,
    id: &str,
    strategy: RepairStrategy,
) -> Result<Value> {
    ensure_node_unlocked(raw, page, id)?;
    let mut next = raw.clone();
    let node = locate(&mut next, page, id)?.clone();
    let (width, height) = dimensions(&node)?;
    let damaged = check_tiles(&next, &node["tiles"]);
    let mut node_copy = node.clone();
    let (surface, lost) = match strategy {
        RepairStrategy::Replay => {
            let surface = replay(&next, &node).map_err(|error| {
                anyhow::anyhow!("{error}; try `--strategy transparent` to keep the intact tiles")
            })?;
            (surface, Vec::new())
        }
        RepairStrategy::Transparent => {
            let mut kept = Map::new();
            for (key, digest) in node["tiles"].as_object().into_iter().flatten() {
                if !damaged.missing.contains(key) && !damaged.corrupt.contains(key) {
                    kept.insert(key.clone(), digest.clone());
                }
            }
            let surface = Surface::load_map(&next, width, height, &Value::Object(kept))?;
            let mut lost = damaged.missing.clone();
            lost.extend(damaged.corrupt.iter().cloned());
            lost.sort();
            (surface, lost)
        }
    };
    // Damaged store entries under a digest the rebuilt surface still uses must be
    // rewritten; everything else unreferenced is collected below.
    surface.store_with(&mut next, &mut node_copy, true)?;
    let hash = surface.tile_map_hash();
    let rolled = strategy == RepairStrategy::Transparent;
    if rolled {
        roll_checkpoint(&mut node_copy, &hash);
        node_copy["checkpoint"]["repaired_lost_tiles"] = json!(lost);
    }
    *locate(&mut next, page, id)? = node_copy;
    // Damaged entries this layer no longer references are collected here unless
    // another layer still pins them; that layer then reports its own damage.
    let released = collect_garbage(&mut next);
    crate::scene::validate(&next)?;
    *raw = next;
    Ok(json!({
        "id": id,
        "strategy": match strategy { RepairStrategy::Replay => "replay", RepairStrategy::Transparent => "transparent" },
        "tiles_missing": damaged.missing.len(),
        "tiles_corrupt": damaged.corrupt.len(),
        "lost_tiles": lost,
        "checkpoint_rolled": rolled,
        "tile_map_sha256": hash,
        "tiles_released": released,
    }))
}
