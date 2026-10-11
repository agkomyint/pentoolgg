//! `pentool paint-plan`: declarative, atomic, multi-pass paint plans
//! (v0.13.0 task 9).
//!
//! A plan is an ordered list of passes. Each pass names a raster layer and one of
//! the shared raster actions (`stroke`, `stroke-path`, `stroke-field`, ...) with the
//! same `args` the CLI and `POST /api/raster` use, so a pass can never do more than
//! the individual command could. The planner runs every pass on a clone of the
//! document, measures what each pass changed, enforces per-pass and per-plan limits,
//! and only then hands the final document to the shared transaction path. Dry run
//! and commit use the same planner, so the report is identical.
use crate::raster;
use crate::transaction;
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Most passes in one plan (protocol hard limit).
pub const MAX_PASSES: usize = 64;
/// Hard plan limits from the protocol; plan `limits` may only lower them.
pub const MAX_DABS: u64 = 4_000_000;
pub const MAX_CHANGED_PIXELS: u64 = 67_108_864;
pub const MAX_TILES: u64 = 4_096;
const MAX_PLAN_BYTES: u64 = 8 << 20;

const ACTIONS: &[&str] = &[
    "stroke",
    "stroke-path",
    "stroke-field",
    "fill",
    "clear",
    "clone",
    "heal",
];

#[derive(clap::Args, Debug)]
pub struct PlanArgs {
    /// `.pen` document to paint into.
    pub input: PathBuf,
    /// Plan JSON file (or `-` for stdin is not supported; use a file).
    pub plan: PathBuf,
    /// Plan and report without writing.
    #[arg(long)]
    pub dry_run: bool,
    /// Refuse unless the document is still at this revision.
    #[arg(long)]
    pub if_revision: Option<String>,
    /// Refuse unless the canonical plan hashes to this value.
    #[arg(long)]
    pub expect_plan_hash: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct Limits {
    passes: u64,
    dabs: u64,
    changed_pixels: u64,
    tiles: u64,
}

impl Limits {
    fn parse(value: &Value) -> Result<Self> {
        let mut limits = Self {
            passes: MAX_PASSES as u64,
            dabs: MAX_DABS,
            changed_pixels: MAX_CHANGED_PIXELS,
            tiles: MAX_TILES,
        };
        if value.is_null() {
            return Ok(limits);
        }
        let object = value
            .as_object()
            .context("[invalid-request] plan.limits must be an object")?;
        for (key, v) in object {
            let (slot, hard) = match key.as_str() {
                "max_passes" => (&mut limits.passes, MAX_PASSES as u64),
                "max_dabs" => (&mut limits.dabs, MAX_DABS),
                "max_changed_pixels" => (&mut limits.changed_pixels, MAX_CHANGED_PIXELS),
                "max_tiles" => (&mut limits.tiles, MAX_TILES),
                other => bail!("[invalid-request] unknown plan limit {other:?}; use max_passes, max_dabs, max_changed_pixels or max_tiles"),
            };
            let n = v
                .as_u64()
                .filter(|n| (1..=hard).contains(n))
                .with_context(|| {
                    format!("[invalid-request] plan.limits.{key} must be an integer 1-{hard}")
                })?;
            *slot = n;
        }
        Ok(limits)
    }

    fn to_json(self) -> Value {
        json!({
            "max_passes": self.passes,
            "max_dabs": self.dabs,
            "max_changed_pixels": self.changed_pixels,
            "max_tiles": self.tiles,
        })
    }
}

struct Pass {
    id: String,
    target: String,
    action: String,
    args: Value,
    description: Option<String>,
    /// Optional measured-diagnostics inputs: region, protect, palette.
    diagnostics: Map<String, Value>,
}

struct Plan {
    id: String,
    page: Option<String>,
    description: Option<String>,
    passes: Vec<Pass>,
    limits: Limits,
}

fn valid_id(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 128
        && text
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn reject_unknown(object: &Map<String, Value>, allowed: &[&str], what: &str) -> Result<()> {
    if let Some(key) = object.keys().find(|k| !allowed.contains(&k.as_str())) {
        bail!(
            "[invalid-request] unknown {what} field {key:?}; use {}",
            allowed.join(", ")
        )
    }
    Ok(())
}

fn id_field(object: &Map<String, Value>, key: &str, what: &str) -> Result<String> {
    let text = object
        .get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("[invalid-request] {what} needs a string {key:?}"))?;
    ensure!(
        valid_id(text),
        "[invalid-request] {what} {key} {text:?} must be 1-128 characters of letters, digits, '.', '_' or '-'"
    );
    Ok(text.to_owned())
}

impl Plan {
    fn parse(value: &Value) -> Result<Self> {
        let object = value
            .as_object()
            .context("[invalid-request] a paint plan must be a JSON object")?;
        reject_unknown(
            object,
            &[
                "schema",
                "engine",
                "id",
                "page",
                "description",
                "passes",
                "limits",
            ],
            "plan",
        )?;
        ensure!(
            object.get("schema").and_then(Value::as_u64) == Some(1),
            "[unsupported-capability] plan.schema must be 1"
        );
        ensure!(
            object.get("engine").and_then(Value::as_u64) == Some(raster::ENGINE),
            "[unsupported-capability] plan.engine must be {}",
            raster::ENGINE
        );
        let id = id_field(object, "id", "plan")?;
        let page = match object.get("page") {
            None | Some(Value::Null) => None,
            Some(_) => Some(id_field(object, "page", "plan")?),
        };
        let description = match object.get("description") {
            None | Some(Value::Null) => None,
            Some(Value::String(text)) if text.len() <= 4096 => Some(text.clone()),
            Some(_) => {
                bail!("[invalid-request] plan.description must be a string of at most 4096 bytes")
            }
        };
        let limits = Limits::parse(object.get("limits").unwrap_or(&Value::Null))?;
        let passes = object
            .get("passes")
            .and_then(Value::as_array)
            .context("[invalid-request] plan.passes must be an array")?;
        ensure!(
            !passes.is_empty() && passes.len() as u64 <= limits.passes,
            "[limit-exceeded] a plan needs 1-{} passes, got {}",
            limits.passes,
            passes.len()
        );
        let mut seen = BTreeSet::new();
        let mut parsed = Vec::with_capacity(passes.len());
        for (index, pass) in passes.iter().enumerate() {
            let what = format!("pass {index}");
            let object = pass
                .as_object()
                .with_context(|| format!("[invalid-request] {what} must be an object"))?;
            reject_unknown(
                object,
                &[
                    "id",
                    "target",
                    "action",
                    "args",
                    "description",
                    "region",
                    "protect",
                    "palette",
                ],
                &what,
            )?;
            let pass_id = id_field(object, "id", &what)?;
            ensure!(
                seen.insert(pass_id.clone()),
                "[invalid-request] duplicate pass id {pass_id:?}; pass ids must be unique"
            );
            let target = id_field(object, "target", &what)?;
            let action = object
                .get("action")
                .and_then(Value::as_str)
                .with_context(|| format!("[invalid-request] pass {pass_id} needs an \"action\""))?;
            ensure!(
                ACTIONS.contains(&action),
                "[invalid-request] pass {pass_id}: action {action:?} is not allowed in a plan; use {}",
                ACTIONS.join(", ")
            );
            let args = object.get("args").cloned().unwrap_or(Value::Null);
            ensure!(
                args.is_null() || args.is_object(),
                "[invalid-request] pass {pass_id}: args must be an object"
            );
            let description = match object.get("description") {
                None | Some(Value::Null) => None,
                Some(Value::String(text)) if text.len() <= 1024 => Some(text.clone()),
                Some(_) => bail!("[invalid-request] pass {pass_id}: description must be a string of at most 1024 bytes"),
            };
            let mut diagnostics = Map::new();
            for key in ["region", "protect", "palette"] {
                if let Some(v) = object.get(key).filter(|v| !v.is_null()) {
                    diagnostics.insert(key.to_owned(), v.clone());
                }
            }
            crate::paint_score::parse_options(&diagnostics)
                .with_context(|| format!("pass {pass_id}"))?;
            parsed.push(Pass {
                id: pass_id,
                target,
                action: action.to_owned(),
                args,
                description,
                diagnostics,
            });
        }
        Ok(Self {
            id,
            page,
            description,
            passes: parsed,
            limits,
        })
    }

    /// The canonical resolved form that `plan_hash` covers.
    fn canonical(&self) -> Value {
        json!({
            "schema": 1,
            "engine": raster::ENGINE,
            "id": self.id,
            "page": self.page,
            "description": self.description,
            "limits": self.limits.to_json(),
            "passes": self.passes.iter().map(|p| json!({
                "id": p.id,
                "target": p.target,
                "action": p.action,
                "args": p.args,
                "description": p.description,
                "diagnostics": p.diagnostics,
            })).collect::<Vec<_>>(),
        })
    }

    fn hash(&self) -> Result<String> {
        Ok(crate::resource::sha256(&serde_json::to_vec(
            &self.canonical(),
        )?))
    }
}

/// Pixel difference between two surfaces of one layer: changed pixels, tiles and the
/// bounding box `[x0, y0, x1, y1)` of every changed pixel.
fn diff(before: &raster::Surface, after: &raster::Surface) -> (u64, u64, Option<[u32; 4]>) {
    let keys: BTreeSet<(u32, u32)> = before
        .tiles
        .keys()
        .chain(after.tiles.keys())
        .copied()
        .collect();
    let blank = vec![0_u8; raster::TILE_BYTES];
    let (mut pixels, mut tiles) = (0_u64, 0_u64);
    let mut bounds: Option<[u32; 4]> = None;
    for key in keys {
        let a = before.tiles.get(&key).map_or(&blank[..], |t| &t[..]);
        let b = after.tiles.get(&key).map_or(&blank[..], |t| &t[..]);
        if a == b {
            continue;
        }
        let mut changed = false;
        for (index, (pa, pb)) in a.chunks_exact(4).zip(b.chunks_exact(4)).enumerate() {
            if pa == pb {
                continue;
            }
            changed = true;
            pixels += 1;
            let x = key.0 * raster::TILE as u32 + (index % raster::TILE) as u32;
            let y = key.1 * raster::TILE as u32 + (index / raster::TILE) as u32;
            let e = bounds.get_or_insert([x, y, x + 1, y + 1]);
            e[0] = e[0].min(x);
            e[1] = e[1].min(y);
            e[2] = e[2].max(x + 1);
            e[3] = e[3].max(y + 1);
        }
        if changed {
            tiles += 1;
        }
    }
    (pixels, tiles, bounds)
}

/// Cooperative cancellation point. The error names the last completed stage and
/// states that nothing was written (`[cancelled]`, protocol "Cancellation").
fn checkpoint(stage: &str) -> Result<()> {
    crate::composite::check_cancelled()
        .map_err(|e| anyhow::anyhow!("{e}; last completed stage: {stage}; nothing was written"))
}

/// Run the plan on a clone of `raw`. On success `raw` is replaced and the report is
/// returned; on any failure `raw` is untouched and the error names the pass.
pub fn execute(raw: &mut Value, page: Option<&str>, plan_json: &Value) -> Result<Value> {
    checkpoint("start")?;
    let plan = Plan::parse(plan_json)?;
    let plan_hash = plan.hash()?;
    checkpoint("parse")?;
    let page = plan.page.as_deref().or(page);
    let mut next = raw.clone();
    let (mut dabs, mut pixels, mut tiles, mut strokes) = (0_u64, 0_u64, 0_u64, 0_u64);
    let mut touched: BTreeSet<(String, u32, u32)> = BTreeSet::new();
    let mut reports = Vec::with_capacity(plan.passes.len());
    for (index, pass) in plan.passes.iter().enumerate() {
        let fail = |what: &str| {
            format!(
                "paint plan {} pass {} ({index}: {} on {}) {what}; nothing was written",
                plan.id, pass.id, pass.action, pass.target
            )
        };
        checkpoint(&format!("pass {index} planned"))?;
        let before = raster::layer_surface(&next, &pass.target)
            .with_context(|| fail("has no such raster layer"))?;
        let result = raster::apply(&mut next, page, &pass.target, &pass.action, &pass.args)
            .with_context(|| fail("failed"))?;
        let after = raster::layer_surface(&next, &pass.target)?;
        let (changed, changed_tiles, bounds) = diff(&before, &after);
        let diagnostics = crate::paint_score::diagnose(
            &before,
            &after,
            &crate::paint_score::parse_options(&pass.diagnostics)?,
        );
        // stroke-path nests dabs under "generated" and lists "strokes"; stroke-field
        // reports "dabs" and "stroke_ids"; a plain stroke reports "dabs" and "stroke".
        let pass_dabs = result["dabs"]
            .as_u64()
            .or_else(|| result["generated"]["dabs"].as_u64())
            .unwrap_or(0);
        let pass_strokes = ["stroke_ids", "strokes"]
            .iter()
            .find_map(|key| result[*key].as_array().map(|ids| ids.len() as u64))
            .unwrap_or_else(|| u64::from(result["stroke"].is_string()));
        dabs += pass_dabs;
        pixels += changed;
        strokes += pass_strokes;
        for (key, a) in &after.tiles {
            if before.tiles.get(key) != Some(a) {
                touched.insert((pass.target.clone(), key.0, key.1));
            }
        }
        tiles = touched.len() as u64;
        for (what, value, limit) in [
            ("dabs", dabs, plan.limits.dabs),
            ("changed pixels", pixels, plan.limits.changed_pixels),
            ("changed tiles", tiles, plan.limits.tiles),
        ] {
            if value > limit {
                bail!(
                    "[limit-exceeded] {}: the plan has reached {value} {what} (limit {limit}); shrink the pass or raise plan.limits",
                    fail("exceeded a plan limit")
                )
            }
        }
        checkpoint(&format!("pass {index} applied"))?;
        reports.push(json!({
            "index": index,
            "id": pass.id,
            "target": pass.target,
            "action": pass.action,
            "description": pass.description,
            "strokes": pass_strokes,
            "dabs": pass_dabs,
            "changed_pixels": changed,
            "changed_tiles": changed_tiles,
            "changed_bounds": bounds,
            "tile_map_sha256": after.tile_map_hash(),
            "diagnostics": diagnostics,
            "result": result,
        }));
    }
    transaction::validate_value(&next)?;
    checkpoint("validate")?;
    let mut targets: Vec<&str> = plan.passes.iter().map(|p| p.target.as_str()).collect();
    targets.sort_unstable();
    targets.dedup();
    let mut final_maps = Map::new();
    for target in targets {
        final_maps.insert(
            target.to_owned(),
            json!(raster::layer_surface(&next, target)?.tile_map_hash()),
        );
    }
    *raw = next;
    Ok(json!({
        "plan": plan.id,
        "plan_hash": plan_hash,
        "engine": raster::ENGINE,
        "limits": plan.limits.to_json(),
        "passes": reports,
        "totals": {
            "passes": plan.passes.len(),
            "strokes": strokes,
            "dabs": dabs,
            "changed_pixels": pixels,
            "changed_tiles": tiles,
        },
        "tile_maps": final_maps,
    }))
}

/// Validate a plan document without running it and return its canonical hash.
pub fn validate_template(plan_json: &Value) -> Result<String> {
    Plan::parse(plan_json)?.hash()
}

pub fn run(args: PlanArgs, selected_page: Option<&str>) -> Result<()> {
    let PlanArgs {
        input,
        plan,
        dry_run,
        if_revision,
        expect_plan_hash,
    } = args;
    ensure!(
        std::fs::metadata(&plan)
            .with_context(|| format!("[missing-resource] cannot read {}", plan.display()))?
            .len()
            <= MAX_PLAN_BYTES,
        "[limit-exceeded] {} is larger than 8 MiB",
        plan.display()
    );
    let plan_json: Value = serde_json::from_slice(&std::fs::read(&plan)?)
        .context("[invalid-request] the plan file must be JSON")?;
    if let Some(expected) = &expect_plan_hash {
        let actual = Plan::parse(&plan_json)?.hash()?;
        ensure!(
            &actual == expected,
            "[plan-changed] the plan hashes to {actual}, not the expected {expected}; review the plan again. The document was not changed"
        );
    }
    let mut raw: Value = serde_json::from_slice(
        &std::fs::read(&input)
            .with_context(|| format!("[missing-resource] cannot read {}", input.display()))?,
    )
    .context("[malformed-resource] invalid .pen document")?;
    transaction::validate_value(&raw)?;
    let result = execute(&mut raw, selected_page, &plan_json)?;
    let change: transaction::ChangeSummary = transaction::commit_value(
        Path::new(&input),
        "paint-plan",
        dry_run,
        if_revision.as_deref(),
        &raw,
    )?;
    // A dry run's `commit` block is what to pass back to commit this exact plan.
    let commit = json!({
        "if_revision": change.revision_before,
        "plan_hash": result["plan_hash"],
    });
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"result": result, "commit": commit, "dry_run": dry_run, "change": change})
        )?
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (Value, Value) {
        let mut raw = crate::scene::new_document(64, 64);
        raster::add(&mut raw, None, None, "p", 0.0, 0.0, 64, 64).unwrap();
        let passes: Vec<Value> = (0..3)
            .map(|i| {
                json!({"id": format!("s{i}"), "target": "p", "action": "stroke-path",
                       "args": {"path": format!("M2 {} L60 {}", 8 + i * 8, 8 + i * 8),
                                "brush": {"size": 4, "hardness": 1}, "color": "#336699"}})
            })
            .collect();
        (
            raw,
            json!({"schema": 1, "engine": 1, "id": "c", "passes": passes}),
        )
    }

    #[test]
    fn cancellation_at_every_checkpoint_writes_nothing_and_names_the_stage() {
        let (raw, plan) = fixture();
        let mut checkpoints = 0;
        for n in 1..=64 {
            let mut working = raw.clone();
            let (result, reached) =
                crate::composite::cancel_at_check(n, || execute(&mut working, None, &plan));
            if !reached {
                // Past the last checkpoint the plan simply completes.
                assert!(result.is_ok());
                break;
            }
            checkpoints = n;
            let error = format!("{:#}", result.unwrap_err());
            assert!(error.contains("[cancelled]"), "{n}: {error}");
            assert!(error.contains("last completed stage"), "{n}: {error}");
            assert_eq!(
                working, raw,
                "cancelled at checkpoint {n} mutated the document"
            );
        }
        // start, parse, 3 x (planned, applied), validate.
        assert_eq!(checkpoints, 9);
    }
}
