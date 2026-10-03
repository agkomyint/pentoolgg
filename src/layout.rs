//! Bounds-based alignment and distribution for v4 nodes and groups.
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum Operation {
    Left,
    Hcenter,
    Right,
    Top,
    Vcenter,
    Bottom,
    DistributeH,
    DistributeV,
    GapH,
    GapV,
    Grid,
}

#[derive(Clone)]
struct Box2 {
    id: String,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}
pub fn apply(
    raw: &mut Value,
    page: Option<&str>,
    operation: Operation,
    ids: &[String],
    gap: Option<f64>,
    columns: Option<usize>,
    key: Option<&str>,
) -> Result<Value> {
    if ids.len() < 2 {
        bail!("layout requires at least two node IDs")
    }
    let mut boxes = ids
        .iter()
        .map(|id| {
            let b = crate::scene::node_bounds(raw, page, id)?;
            Ok(Box2 {
                id: id.clone(),
                x: b["x"].as_f64().unwrap(),
                y: b["y"].as_f64().unwrap(),
                w: b["width"].as_f64().unwrap(),
                h: b["height"].as_f64().unwrap(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let selection_min_x = boxes.iter().map(|b| b.x).fold(f64::INFINITY, f64::min);
    let selection_max_r = boxes
        .iter()
        .map(|b| b.x + b.w)
        .fold(f64::NEG_INFINITY, f64::max);
    let selection_min_y = boxes.iter().map(|b| b.y).fold(f64::INFINITY, f64::min);
    let selection_max_b = boxes
        .iter()
        .map(|b| b.y + b.h)
        .fold(f64::NEG_INFINITY, f64::max);
    let key_box = key
        .map(|id| {
            boxes
                .iter()
                .find(|item| item.id == id)
                .with_context(|| format!("key object is not selected: {id}"))
        })
        .transpose()?;
    let (min_x, max_r, min_y, max_b) = key_box.map_or(
        (
            selection_min_x,
            selection_max_r,
            selection_min_y,
            selection_max_b,
        ),
        |item| (item.x, item.x + item.w, item.y, item.y + item.h),
    );
    let mut deltas = Vec::new();
    match operation {
        Operation::Left
        | Operation::Hcenter
        | Operation::Right
        | Operation::Top
        | Operation::Vcenter
        | Operation::Bottom => {
            for b in &boxes {
                let (dx, dy) = match operation {
                    Operation::Left => (min_x - b.x, 0.0),
                    Operation::Hcenter => ((min_x + max_r - b.w) / 2.0 - b.x, 0.0),
                    Operation::Right => (max_r - b.w - b.x, 0.0),
                    Operation::Top => (0.0, min_y - b.y),
                    Operation::Vcenter => (0.0, (min_y + max_b - b.h) / 2.0 - b.y),
                    Operation::Bottom => (0.0, max_b - b.h - b.y),
                    _ => unreachable!(),
                };
                move_one(raw, page, b, dx, dy, &mut deltas)?
            }
        }
        Operation::DistributeH | Operation::GapH => {
            boxes.sort_by(|a, b| a.x.total_cmp(&b.x));
            let total: f64 = boxes.iter().map(|b| b.w).sum();
            let actual = if matches!(operation, Operation::GapH) {
                gap.context("--gap is required")?
            } else {
                (selection_max_r - selection_min_x - total) / (boxes.len() - 1) as f64
            };
            let mut cursor = selection_min_x;
            for b in &boxes {
                move_one(raw, page, b, cursor - b.x, 0.0, &mut deltas)?;
                cursor += b.w + actual
            }
        }
        Operation::DistributeV | Operation::GapV => {
            boxes.sort_by(|a, b| a.y.total_cmp(&b.y));
            let total: f64 = boxes.iter().map(|b| b.h).sum();
            let actual = if matches!(operation, Operation::GapV) {
                gap.context("--gap is required")?
            } else {
                (selection_max_b - selection_min_y - total) / (boxes.len() - 1) as f64
            };
            let mut cursor = selection_min_y;
            for b in &boxes {
                move_one(raw, page, b, 0.0, cursor - b.y, &mut deltas)?;
                cursor += b.h + actual
            }
        }
        Operation::Grid => {
            let columns = columns.context("--columns is required")?;
            if columns == 0 {
                bail!("--columns must be positive")
            }
            let gap = gap.unwrap_or(0.0);
            let cell_w = boxes.iter().map(|b| b.w).fold(0.0, f64::max);
            let cell_h = boxes.iter().map(|b| b.h).fold(0.0, f64::max);
            for (index, b) in boxes.iter().enumerate() {
                let x = selection_min_x + (index % columns) as f64 * (cell_w + gap);
                let y = selection_min_y + (index / columns) as f64 * (cell_h + gap);
                move_one(raw, page, b, x - b.x, y - b.y, &mut deltas)?
            }
        }
    }
    crate::scene::validate(raw)?;
    Ok(
        json!({"selection_bounds":{"x":selection_min_x,"y":selection_min_y,"right":selection_max_r,"bottom":selection_max_b},"key":key,"deltas":deltas}),
    )
}
fn move_one(
    raw: &mut Value,
    page: Option<&str>,
    b: &Box2,
    dx: f64,
    dy: f64,
    out: &mut Vec<Value>,
) -> Result<()> {
    crate::scene::translate_node(raw, page, &b.id, dx, dy)?;
    out.push(
        json!({"id":b.id,"dx":dx,"dy":dy,"bounds":{"x":b.x,"y":b.y,"width":b.w,"height":b.h}}),
    );
    Ok(())
}
