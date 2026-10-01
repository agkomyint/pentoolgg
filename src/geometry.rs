//! One geometry implementation used by CLI and HTTP visual editing.
use crate::document::Document;
use anyhow::{bail, Context, Result};
use clap::Subcommand;
use kurbo::{Affine, BezPath, ParamCurveNearest, PathEl, Point, Shape};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, Deserialize, Subcommand)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Operation {
    Bounds,
    Nodes,
    Hit {
        x: f64,
        y: f64,
        #[arg(long, default_value_t = 4.0)]
        #[serde(default = "tolerance")]
        tolerance: f64,
    },
    /// Affine matrix [a,b,c,d,e,f]: x'=ax+cy+e, y'=bx+dy+f.
    Transform {
        a: f64,
        b: f64,
        c: f64,
        d: f64,
        e: f64,
        f: f64,
    },
    Translate {
        dx: f64,
        dy: f64,
    },
    Rotate {
        degrees: f64,
        #[arg(long, default_value_t = 0.0)]
        #[serde(default)]
        cx: f64,
        #[arg(long, default_value_t = 0.0)]
        #[serde(default)]
        cy: f64,
    },
    Scale {
        sx: f64,
        sy: f64,
        #[arg(long, default_value_t = 0.0)]
        #[serde(default)]
        cx: f64,
        #[arg(long, default_value_t = 0.0)]
        #[serde(default)]
        cy: f64,
    },
    /// Set a zero-based anchor position; attached handles follow it.
    MoveAnchor {
        index: usize,
        x: f64,
        y: f64,
    },
    /// Set a control point on a normalized element. Handle is 1 or 2.
    SetHandle {
        element: usize,
        handle: usize,
        x: f64,
        y: f64,
    },
    /// Split a drawable segment at 0<t<1, preserving its shape.
    Split {
        segment: usize,
        t: f64,
    },
}
fn tolerance() -> f64 {
    4.0
}

pub fn execute_layer(doc: &mut Document, id: &str, op: &Operation) -> Result<Value> {
    if !matches!(
        op,
        Operation::Transform { .. }
            | Operation::Translate { .. }
            | Operation::Rotate { .. }
            | Operation::Scale { .. }
    ) {
        bail!("layer operations support translate, rotate, scale, and transform");
    }
    let mut updated = doc.clone();
    let layer = updated
        .layers
        .iter()
        .find(|l| l.id == id)
        .context("layer not found")?;
    if layer.locked {
        bail!("layer is locked");
    }
    let ids: Vec<String> = layer.paths.iter().map(|p| p.id.clone()).collect();
    let text_ids: Vec<String> = layer.texts.iter().map(|t| t.id.clone()).collect();
    for path in &ids {
        execute(&mut updated, id, path, op)?;
    }
    for text in &text_ids {
        execute(&mut updated, id, text, op)?;
    }
    *doc = updated;
    Ok(json!({"ok":true,"paths_changed":ids.len(),"texts_changed":text_ids.len()}))
}
impl Operation {
    pub fn is_query(&self) -> bool {
        matches!(self, Self::Bounds | Self::Nodes | Self::Hit { .. })
    }
}
fn endpoint(el: &PathEl) -> Option<Point> {
    match el {
        PathEl::MoveTo(p) | PathEl::LineTo(p) => Some(*p),
        PathEl::QuadTo(_, p) | PathEl::CurveTo(_, _, p) => Some(*p),
        PathEl::ClosePath => None,
    }
}

fn move_endpoint(els: &mut [PathEl], i: usize, delta: kurbo::Vec2) {
    let p = endpoint(&els[i]).unwrap() + delta;
    els[i] = match els[i] {
        PathEl::MoveTo(_) => PathEl::MoveTo(p),
        PathEl::LineTo(_) => PathEl::LineTo(p),
        PathEl::QuadTo(a, _) => PathEl::QuadTo(a + delta, p),
        PathEl::CurveTo(a, b, _) => PathEl::CurveTo(a, b + delta, p),
        _ => unreachable!(),
    };
    if let Some(next) = els.get_mut(i + 1) {
        *next = match *next {
            PathEl::CurveTo(a, b, p) => PathEl::CurveTo(a + delta, b, p),
            PathEl::QuadTo(a, p) => PathEl::QuadTo(a + delta, p),
            other => other,
        };
    }
}

pub fn execute(doc: &mut Document, layer_id: &str, id: &str, op: &Operation) -> Result<Value> {
    if doc
        .layers
        .iter()
        .find(|l| l.id == layer_id)
        .is_some_and(|l| l.texts.iter().any(|t| t.id == id))
    {
        return execute_text(doc, layer_id, id, op);
    }
    let layer = doc
        .layers
        .iter_mut()
        .find(|l| l.id == layer_id)
        .context("layer not found")?;
    if !op.is_query() && layer.locked {
        bail!("layer is locked");
    }
    let path = layer
        .paths
        .iter_mut()
        .find(|p| p.id == id)
        .context("path not found")?;
    let mut bez = BezPath::from_svg(&path.d).context("invalid path geometry")?;
    if bez.elements().is_empty() {
        bail!("empty path");
    }
    if !bez.is_finite() {
        bail!("path coordinates must be finite");
    }
    // Reject nonfinite numbers before they can enter saved geometry.
    fn check(v: &Value) -> bool {
        match v {
            Value::Number(n) => n.as_f64().is_some_and(f64::is_finite),
            Value::Object(m) => m.values().all(check),
            Value::Null => false,
            _ => true,
        }
    }
    if !check(&serde_json::to_value(op)?) {
        bail!("coordinates must be finite");
    }
    let result = match *op {
        Operation::Bounds => {
            let r = bez.bounding_box();
            json!({"x":r.x0,"y":r.y0,"width":r.width(),"height":r.height(),"includes_stroke":false})
        }
        Operation::Nodes => {
            let mut index = 0;
            let nodes:Vec<Value>=bez.elements().iter().enumerate().filter_map(|(element,el)|endpoint(el).map(|p|{let n=json!({"index":index,"element":element,"x":p.x,"y":p.y,"handles":match el{PathEl::QuadTo(a,_)=>vec![json!({"handle":1,"x":a.x,"y":a.y})],PathEl::CurveTo(a,b,_)=>vec![json!({"handle":1,"x":a.x,"y":a.y}),json!({"handle":2,"x":b.x,"y":b.y})],_=>vec![]}});index+=1;n})).collect();
            json!({"nodes":nodes,"d":bez.to_svg()})
        }
        Operation::Hit { x, y, tolerance } => {
            if tolerance < 0.0 {
                bail!("tolerance must be nonnegative");
            }
            let p = Point::new(x, y);
            let distance = bez
                .segments()
                .map(|s| s.nearest(p, 1e-6).distance_sq)
                .fold(f64::INFINITY, f64::min)
                .sqrt();
            let fill = path.fill != "none" && bez.winding(p) != 0;
            let stroke =
                path.stroke != "none" && distance <= f64::from(path.stroke_width) / 2.0 + tolerance;
            json!({"hit":layer.visible&&(fill||stroke),"fill":fill,"stroke":stroke,"distance":distance})
        }
        Operation::Transform { a, b, c, d, e, f } => {
            bez.apply_affine(Affine::new([a, b, c, d, e, f]));
            json!({})
        }
        Operation::Translate { dx, dy } => {
            bez.apply_affine(Affine::translate((dx, dy)));
            json!({})
        }
        Operation::Rotate { degrees, cx, cy } => {
            bez.apply_affine(
                Affine::translate((cx, cy))
                    * Affine::rotate(degrees.to_radians())
                    * Affine::translate((-cx, -cy)),
            );
            json!({})
        }
        Operation::Scale { sx, sy, cx, cy } => {
            bez.apply_affine(
                Affine::translate((cx, cy))
                    * Affine::scale_non_uniform(sx, sy)
                    * Affine::translate((-cx, -cy)),
            );
            json!({})
        }
        Operation::MoveAnchor { index, x, y } => {
            let mut els = bez.elements().to_vec();
            let i = els
                .iter()
                .enumerate()
                .filter(|(_, e)| endpoint(e).is_some())
                .nth(index)
                .map(|(i, _)| i)
                .context("anchor index out of range")?;
            let delta = Point::new(x, y) - endpoint(&els[i]).unwrap();
            let start = (0..=i)
                .rev()
                .find(|&j| matches!(els[j], PathEl::MoveTo(_)))
                .unwrap();
            let end = (start + 1..els.len())
                .find(|&j| matches!(els[j], PathEl::MoveTo(_)))
                .unwrap_or(els.len());
            let linked = if end > start + 2
                && matches!(els[end - 1], PathEl::ClosePath)
                && endpoint(&els[start]) == endpoint(&els[end - 2])
            {
                if i == start {
                    Some(end - 2)
                } else if i == end - 2 {
                    Some(start)
                } else {
                    None
                }
            } else {
                None
            };
            move_endpoint(&mut els, i, delta);
            if let Some(j) = linked {
                move_endpoint(&mut els, j, delta);
            }
            bez = BezPath::from_vec(els);
            json!({})
        }
        Operation::SetHandle {
            element,
            handle,
            x,
            y,
        } => {
            let mut els = bez.elements().to_vec();
            let el = els.get_mut(element).context("element index out of range")?;
            let p = Point::new(x, y);
            *el = match (*el, handle) {
                (PathEl::CurveTo(_, b, end), 1) => PathEl::CurveTo(p, b, end),
                (PathEl::CurveTo(a, _, end), 2) => PathEl::CurveTo(a, p, end),
                (PathEl::QuadTo(_, end), 1) => PathEl::QuadTo(p, end),
                _ => bail!("handle does not exist on this element"),
            };
            bez = BezPath::from_vec(els);
            json!({})
        }
        Operation::Split { segment, t } => {
            use kurbo::ParamCurve;
            if !(0.0 < t && t < 1.0) {
                bail!("split t must be between 0 and 1");
            }
            let target = bez
                .segments()
                .nth(segment)
                .context("segment index out of range")?;
            let mut count = 0;
            let mut els = Vec::new();
            let mut start = Point::ZERO;
            let mut last = Point::ZERO;
            for el in bez.elements() {
                let drawable = match el {
                    PathEl::MoveTo(p) => {
                        start = *p;
                        last = *p;
                        false
                    }
                    PathEl::ClosePath => {
                        let drawn = last != start;
                        last = start;
                        drawn
                    }
                    _ => {
                        last = endpoint(el).unwrap();
                        true
                    }
                };
                if drawable {
                    if count == segment {
                        for s in [target.subsegment(0.0..t), target.subsegment(t..1.0)] {
                            els.push(match s {
                                kurbo::PathSeg::Line(l) => PathEl::LineTo(l.p1),
                                kurbo::PathSeg::Quad(q) => PathEl::QuadTo(q.p1, q.p2),
                                kurbo::PathSeg::Cubic(c) => PathEl::CurveTo(c.p1, c.p2, c.p3),
                            });
                        }
                        if matches!(el, PathEl::ClosePath) {
                            els.push(PathEl::ClosePath);
                        }
                        count += 1;
                        continue;
                    }
                    count += 1;
                }
                els.push(*el);
            }
            bez = BezPath::from_vec(els);
            json!({})
        }
    };
    if !op.is_query() {
        if !bez.is_finite() {
            bail!("operation produced nonfinite coordinates");
        }
        path.d = bez.to_svg();
        return Ok(json!({"ok":true,"d":path.d}));
    }
    Ok(result)
}

fn execute_text(doc: &mut Document, layer_id: &str, id: &str, op: &Operation) -> Result<Value> {
    let li = doc
        .layers
        .iter()
        .position(|l| l.id == layer_id)
        .context("layer not found")?;
    if !op.is_query() && doc.layers[li].locked {
        bail!("layer is locked");
    }
    let ti = doc.layers[li]
        .texts
        .iter()
        .position(|t| t.id == id)
        .context("text not found")?;
    let text = &doc.layers[li].texts[ti];
    match *op {
        Operation::Bounds => {
            let r = crate::render::text_bounds(doc, text)?;
            Ok(
                json!({"x":r.x0,"y":r.y0,"width":r.width(),"height":r.height(),"kind":"text","includes_stroke":false}),
            )
        }
        Operation::Hit { x, y, tolerance } => {
            if ![x, y, tolerance].iter().all(|v| v.is_finite()) || tolerance < 0.0 {
                bail!("hit coordinates and tolerance must be finite and tolerance nonnegative");
            }
            let r = crate::render::text_bounds(doc, text)?.inset(tolerance);
            Ok(
                json!({"hit":doc.layers[li].visible&&text.fill!="none"&&r.contains((x,y)),"kind":"text","method":"text-layout-bounds"}),
            )
        }
        Operation::Nodes
        | Operation::MoveAnchor { .. }
        | Operation::SetHandle { .. }
        | Operation::Split { .. } => {
            bail!("text is editable typography, not a path; use text set or export --outline-text")
        }
        _ => {
            let affine = match *op {
                Operation::Transform { a, b, c, d, e, f } => Affine::new([a, b, c, d, e, f]),
                Operation::Translate { dx, dy } => Affine::translate((dx, dy)),
                Operation::Rotate { degrees, cx, cy } => {
                    Affine::translate((cx, cy))
                        * Affine::rotate(degrees.to_radians())
                        * Affine::translate((-cx, -cy))
                }
                Operation::Scale { sx, sy, cx, cy } => {
                    Affine::translate((cx, cy))
                        * Affine::scale_non_uniform(sx, sy)
                        * Affine::translate((-cx, -cy))
                }
                _ => unreachable!(),
            };
            let transform = (affine * Affine::new(text.transform)).as_coeffs();
            if !transform.iter().all(|v| v.is_finite()) {
                bail!("transform must be finite");
            }
            doc.layers[li].texts[ti].transform = transform;
            Ok(json!({"ok":true,"kind":"text","transform":transform}))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Path;
    #[test]
    fn layer_transform_moves_fills_together_and_keeps_other_layers() {
        let mut d = doc();
        let mut second = d.layers[0].paths[0].clone();
        second.id = "q".into();
        second.fill = "red".into();
        d.layers[0].paths.push(second);
        let mut other = d.layers[0].clone();
        other.id = "other".into();
        d.layers.push(other.clone());
        execute_layer(
            &mut d,
            "layer-1",
            &Operation::Translate { dx: 20.0, dy: 30.0 },
        )
        .unwrap();
        assert_eq!(d.layers[0].paths[1].fill, "red");
        assert_eq!(d.layers[1].paths[0].d, other.paths[0].d);
        for p in &d.layers[0].paths {
            assert_eq!(
                BezPath::from_svg(&p.d).unwrap().elements()[0],
                PathEl::MoveTo((20.0, 30.0).into())
            );
        }
    }
    #[test]
    fn layer_transform_is_atomic_on_invalid_geometry() {
        let mut d = doc();
        let mut bad = d.layers[0].paths[0].clone();
        bad.id = "bad".into();
        bad.d = "broken".into();
        d.layers[0].paths.push(bad);
        let before = serde_json::to_string(&d).unwrap();
        assert!(execute_layer(
            &mut d,
            "layer-1",
            &Operation::Translate { dx: 2.0, dy: 3.0 }
        )
        .is_err());
        assert_eq!(serde_json::to_string(&d).unwrap(), before);
    }
    #[test]
    fn nonfinite_and_locked_operations_rejected() {
        let mut d = doc();
        assert!(execute(
            &mut d,
            "layer-1",
            "p",
            &Operation::Translate {
                dx: f64::NAN,
                dy: 0.0
            }
        )
        .is_err());
        d.layers[0].locked = true;
        assert!(execute_layer(
            &mut d,
            "layer-1",
            &Operation::Translate { dx: 2.0, dy: 0.0 }
        )
        .is_err());
    }
    #[test]
    fn closed_curve_seam_moves_both_handles() {
        let mut d = doc();
        d.layers[0].paths[0].d = "M 0 0 C 10 0 10 10 0 0 Z".into();
        execute(
            &mut d,
            "layer-1",
            "p",
            &Operation::MoveAnchor {
                index: 0,
                x: 20.0,
                y: 30.0,
            },
        )
        .unwrap();
        let bez = BezPath::from_svg(&d.layers[0].paths[0].d).unwrap();
        assert_eq!(
            bez.elements()[1],
            PathEl::CurveTo(
                (30.0, 30.0).into(),
                (30.0, 40.0).into(),
                (20.0, 30.0).into()
            )
        );
    }
    #[test]
    fn split_indexes_skip_zero_length_closures() {
        let mut d = doc();
        d.layers[0].paths[0].d = "M 0 0 L 10 0 L 0 0 Z M 20 20 L 40 20".into();
        execute(
            &mut d,
            "layer-1",
            "p",
            &Operation::Split { segment: 2, t: 0.5 },
        )
        .unwrap();
        assert!(d.layers[0].paths[0].d.contains("30,20"));
    }
    fn doc() -> Document {
        let mut d = Document::new(100, 100);
        d.layers[0].paths.push(Path {
            id: "p".into(),
            d: "M 0 0 C 0 100 100 100 100 0".into(),
            stroke: "black".into(),
            stroke_width: 2.0,
            stroke_linecap: crate::document::StrokeCap::Round,
            stroke_linejoin: crate::document::StrokeJoin::Round,
            stroke_miterlimit: 4.0,
            fill: "none".into(),
            closed: false,
        });
        d
    }
    #[test]
    fn curve_bounds_are_not_control_bounds() {
        let v = execute(&mut doc(), "layer-1", "p", &Operation::Bounds).unwrap();
        assert_eq!(v["height"], 75.0);
    }
    #[test]
    fn split_preserves_curve() {
        let mut d = doc();
        execute(
            &mut d,
            "layer-1",
            "p",
            &Operation::Split { segment: 0, t: 0.5 },
        )
        .unwrap();
        let b = execute(&mut d, "layer-1", "p", &Operation::Bounds).unwrap();
        assert_eq!(b["height"], 75.0);
        assert_eq!(
            BezPath::from_svg(&d.layers[0].paths[0].d)
                .unwrap()
                .segments()
                .count(),
            2
        );
    }
    #[test]
    fn move_anchor_moves_attached_handle() {
        let mut d = doc();
        execute(
            &mut d,
            "layer-1",
            "p",
            &Operation::MoveAnchor {
                index: 0,
                x: 10.0,
                y: 20.0,
            },
        )
        .unwrap();
        assert_eq!(
            BezPath::from_svg(&d.layers[0].paths[0].d)
                .unwrap()
                .elements()[1],
            PathEl::CurveTo(
                (10.0, 120.0).into(),
                (100.0, 100.0).into(),
                (100.0, 0.0).into()
            )
        );
    }
    #[test]
    fn hit_uses_curve_not_just_box() {
        let mut d = doc();
        assert_eq!(
            execute(
                &mut d,
                "layer-1",
                "p",
                &Operation::Hit {
                    x: 50.0,
                    y: 75.0,
                    tolerance: 0.1
                }
            )
            .unwrap()["hit"],
            true
        );
        assert_eq!(
            execute(
                &mut d,
                "layer-1",
                "p",
                &Operation::Hit {
                    x: 50.0,
                    y: 30.0,
                    tolerance: 0.1
                }
            )
            .unwrap()["hit"],
            false
        );
    }
}
