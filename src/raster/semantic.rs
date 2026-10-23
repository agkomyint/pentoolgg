//! Deterministic semantic path compilation into ordinary raster strokes.
use super::*;
use kurbo::{BezPath, PathEl, Point};

pub const MAX_PATH_SEGMENTS: usize = 4_096;
pub const MAX_FLATTENED_POINTS: usize = 8_192;
pub const MAX_COMPILED_STROKES: usize = 256;

#[derive(Clone, Debug)]
struct ScalarCurve(Vec<[f64; 2]>);

impl ScalarCurve {
    fn parse(name: &str, value: Option<&Value>, lo: f64, hi: f64) -> Result<Option<Self>> {
        let Some(value) = value else { return Ok(None) };
        let points = value
            .as_array()
            .filter(|points| (2..=64).contains(&points.len()))
            .with_context(|| {
                format!("[invalid-curve] {name} needs 2-64 [distance,value] points")
            })?;
        let mut out = Vec::with_capacity(points.len());
        for point in points {
            let pair = point
                .as_array()
                .filter(|pair| pair.len() == 2)
                .and_then(|pair| Some([pair[0].as_f64()?, pair[1].as_f64()?]))
                .filter(|pair| pair.iter().all(|number| number.is_finite()))
                .with_context(|| {
                    format!("[invalid-curve] {name} points must be finite [distance,value] pairs")
                })?;
            if !(0.0..=1.0).contains(&pair[0]) || !(lo..=hi).contains(&pair[1]) {
                bail!("[invalid-curve] {name} point [{},{}] is outside distance 0-1 and value {lo}-{hi}", pair[0], pair[1])
            }
            out.push(pair);
        }
        if out.first().map(|point| point[0]) != Some(0.0)
            || out.last().map(|point| point[0]) != Some(1.0)
            || out.windows(2).any(|pair| pair[1][0] <= pair[0][0])
        {
            bail!("[invalid-curve] {name} distances must strictly increase from 0 to 1")
        }
        Ok(Some(Self(out)))
    }

    fn at(&self, distance: f64) -> f64 {
        let upper = self.0.partition_point(|point| point[0] < distance);
        if upper == 0 {
            return self.0[0][1];
        }
        if upper == self.0.len() {
            return self.0[upper - 1][1];
        }
        let a = self.0[upper - 1];
        let b = self.0[upper];
        let t = (distance - a[0]) / (b[0] - a[0]);
        canonical_number(a[1] + (b[1] - a[1]) * t)
    }
}

#[derive(Clone, Debug)]
struct ColorCurve(Vec<(f64, [u8; 3])>);

impl ColorCurve {
    fn parse(value: Option<&Value>) -> Result<Option<Self>> {
        let Some(value) = value else { return Ok(None) };
        let points = value
            .as_array()
            .filter(|points| (2..=64).contains(&points.len()))
            .context("[invalid-curve] color needs 2-64 [distance,#RRGGBB] points")?;
        let mut out = Vec::with_capacity(points.len());
        for point in points {
            let pair = point
                .as_array()
                .filter(|pair| pair.len() == 2)
                .context("[invalid-curve] color points must be [distance,#RRGGBB]")?;
            let distance = pair[0]
                .as_f64()
                .filter(|n| n.is_finite() && (0.0..=1.0).contains(n))
                .context("[invalid-curve] color distance must be 0-1")?;
            let color = parse_color(
                pair[1]
                    .as_str()
                    .context("[invalid-curve] color value must be #RRGGBB")?,
            )?;
            out.push((distance, color));
        }
        if out.first().map(|point| point.0) != Some(0.0)
            || out.last().map(|point| point.0) != Some(1.0)
            || out.windows(2).any(|pair| pair[1].0 <= pair[0].0)
        {
            bail!("[invalid-curve] color distances must strictly increase from 0 to 1")
        }
        Ok(Some(Self(out)))
    }

    fn at(&self, distance: f64) -> [u8; 3] {
        let upper = self.0.partition_point(|point| point.0 < distance);
        if upper == 0 {
            return self.0[0].1;
        }
        if upper == self.0.len() {
            return self.0[upper - 1].1;
        }
        let (au, a) = self.0[upper - 1];
        let (bu, b) = self.0[upper];
        let t = (distance - au) / (bu - au);
        std::array::from_fn(|i| {
            (f64::from(a[i]) + (f64::from(b[i]) - f64::from(a[i])) * t)
                .round()
                .clamp(0.0, 255.0) as u8
        })
    }
}

fn canonical_number(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

fn midpoint(a: Point, b: Point) -> Point {
    Point::new((a.x + b.x) * 0.5, (a.y + b.y) * 0.5)
}

fn control_distance(point: Point, a: Point, b: Point) -> f64 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let length = (dx * dx + dy * dy).sqrt();
    if length == 0.0 {
        let (px, py) = (point.x - a.x, point.y - a.y);
        (px * px + py * py).sqrt()
    } else {
        ((dy * point.x - dx * point.y + b.x * a.y - b.y * a.x).abs()) / length
    }
}

fn flatten_quad(
    out: &mut Vec<Point>,
    p0: Point,
    p1: Point,
    p2: Point,
    flatness: f64,
    depth: u8,
) -> Result<()> {
    if control_distance(p1, p0, p2) <= flatness {
        out.push(p2);
        return Ok(());
    }
    if depth == 16 {
        bail!("[limit-exceeded] quadratic path subdivision exceeded depth 16")
    }
    let (a, b) = (midpoint(p0, p1), midpoint(p1, p2));
    let middle = midpoint(a, b);
    flatten_quad(out, p0, a, middle, flatness, depth + 1)?;
    flatten_quad(out, middle, b, p2, flatness, depth + 1)
}

fn flatten_cubic(
    out: &mut Vec<Point>,
    p0: Point,
    p1: Point,
    p2: Point,
    p3: Point,
    flatness: f64,
    depth: u8,
) -> Result<()> {
    if control_distance(p1, p0, p3).max(control_distance(p2, p0, p3)) <= flatness {
        out.push(p3);
        return Ok(());
    }
    if depth == 16 {
        bail!("[limit-exceeded] cubic path subdivision exceeded depth 16")
    }
    let (a, b, c) = (midpoint(p0, p1), midpoint(p1, p2), midpoint(p2, p3));
    let (d, e) = (midpoint(a, b), midpoint(b, c));
    let middle = midpoint(d, e);
    flatten_cubic(out, p0, a, d, middle, flatness, depth + 1)?;
    flatten_cubic(out, middle, e, c, p3, flatness, depth + 1)
}

pub struct PathRequest {
    pub path: String,
    pub flatness: f64,
    pub brush: Brush,
    pub color: [u8; 3],
    pub blend: Blend,
    pub seed: u64,
    pub curves: Value,
    /// Region constraints (`Value::Null` for none); see `raster::constraints`.
    pub constraints: Value,
}

struct Curves {
    size: Option<ScalarCurve>,
    pressure: Option<ScalarCurve>,
    flow: Option<ScalarCurve>,
    opacity: Option<ScalarCurve>,
    rotation: Option<ScalarCurve>,
    color: Option<ColorCurve>,
}

impl Curves {
    fn parse(value: &Value) -> Result<Self> {
        let object = value
            .as_object()
            .context("[invalid-curve] curves must be an object")?;
        if let Some(key) = object.keys().find(|key| {
            !matches!(
                key.as_str(),
                "size" | "pressure" | "flow" | "opacity" | "rotation" | "color"
            )
        }) {
            bail!("[invalid-curve] unknown curve {key:?}; use size, pressure, flow, opacity, rotation or color")
        }
        Ok(Self {
            size: ScalarCurve::parse("size", object.get("size"), 1.0, MAX_BRUSH_SIZE)?,
            pressure: ScalarCurve::parse("pressure", object.get("pressure"), 0.0, 1.0)?,
            flow: ScalarCurve::parse("flow", object.get("flow"), 0.0, 1.0)?,
            opacity: ScalarCurve::parse("opacity", object.get("opacity"), 0.0, 1.0)?,
            rotation: ScalarCurve::parse("rotation", object.get("rotation"), -360.0, 360.0)?,
            color: ColorCurve::parse(object.get("color"))?,
        })
    }

    fn changes_style(&self) -> bool {
        self.size.is_some()
            || self.flow.is_some()
            || self.opacity.is_some()
            || self.rotation.is_some()
            || self.color.is_some()
    }
}

fn flatten(path: &str, flatness: f64) -> Result<(usize, Vec<Point>)> {
    if !(0.01..=4.0).contains(&flatness) || !flatness.is_finite() {
        bail!("[invalid-path] flatness must be 0.01-4")
    }
    if path.chars().any(|c| {
        matches!(
            c,
            'm' | 'l' | 'q' | 'c' | 'a' | 'h' | 'v' | 's' | 't' | 'z' | 'H' | 'V' | 'S' | 'T' | 'Z'
        )
    }) {
        bail!("[invalid-path] paths use absolute M, L, Q, C and A commands only; subpaths and close are not supported")
    }
    let source_segments = path
        .chars()
        .filter(|command| matches!(command, 'L' | 'Q' | 'C' | 'A'))
        .count();
    if source_segments == 0 || source_segments > MAX_PATH_SEGMENTS {
        bail!("[invalid-path] a path needs 1-{MAX_PATH_SEGMENTS} drawing segments after one M command")
    }
    let bez = BezPath::from_svg(path).context("[invalid-path] malformed SVG path")?;
    let elements = bez.elements();
    if elements
        .first()
        .is_none_or(|element| !matches!(element, PathEl::MoveTo(_)))
    {
        bail!("[invalid-path] a path needs 1-{MAX_PATH_SEGMENTS} drawing segments after one M command")
    }
    if elements
        .iter()
        .skip(1)
        .any(|element| matches!(element, PathEl::MoveTo(_) | PathEl::ClosePath))
    {
        bail!("[invalid-path] a path must contain one subpath and cannot close")
    }
    let mut points = Vec::new();
    let mut current = None;
    for element in bez {
        match element {
            PathEl::MoveTo(point) => {
                points.push(point);
                current = Some(point);
            }
            PathEl::LineTo(point) => {
                points.push(point);
                current = Some(point);
            }
            PathEl::QuadTo(control, point) => {
                flatten_quad(
                    &mut points,
                    current.context("[invalid-path] Q before M")?,
                    control,
                    point,
                    flatness,
                    0,
                )?;
                current = Some(point);
            }
            PathEl::CurveTo(control1, control2, point) => {
                flatten_cubic(
                    &mut points,
                    current.context("[invalid-path] C before M")?,
                    control1,
                    control2,
                    point,
                    flatness,
                    0,
                )?;
                current = Some(point);
            }
            PathEl::ClosePath => unreachable!("close paths were rejected"),
        }
        if points.len() > MAX_FLATTENED_POINTS {
            bail!(
                "[limit-exceeded] path flattening produced more than {MAX_FLATTENED_POINTS} points"
            )
        }
    }
    points.dedup();
    // Coincident endpoints are a zero-length line: one point, painted as nothing.
    let minimum = if source_segments > 0 { 1 } else { 2 };
    if points.len() < minimum || points.len() > MAX_FLATTENED_POINTS {
        bail!(
            "[limit-exceeded] path flattening produced {} points; allowed 2-{MAX_FLATTENED_POINTS}",
            points.len()
        )
    }
    Ok((source_segments, points))
}

pub fn paint_path(
    raw: &mut Value,
    page: Option<&str>,
    id: &str,
    request: PathRequest,
) -> Result<Value> {
    let curves = Curves::parse(&request.curves)?;
    let (source_segments, points) = flatten(&request.path, request.flatness)?;
    if points.len() == 1 {
        return Ok(json!({
            "constraints": Value::Null,
            "source": {"path": request.path, "segments": source_segments, "flatness": request.flatness},
            "generated": {"points": 1, "samples": 0, "strokes": 0, "dabs": 0},
            "strokes": Vec::<String>::new()
        }));
    }
    let mut lengths = Vec::with_capacity(points.len());
    lengths.push(0.0);
    for pair in points.windows(2) {
        let delta = pair[1] - pair[0];
        lengths.push(lengths.last().copied().unwrap_or(0.0) + delta.hypot());
    }
    let total = *lengths.last().unwrap_or(&0.0);
    if total == 0.0 {
        bail!("[invalid-path] path length is zero")
    }
    let sample = |index: usize| Sample {
        x: canonical_number(points[index].x),
        y: canonical_number(points[index].y),
        pressure: curves
            .pressure
            .as_ref()
            .map_or(1.0, |curve| curve.at(lengths[index] / total)),
        ..Sample::default()
    };
    let spec = if request.constraints.is_null() {
        None
    } else {
        Some(constraints::Spec::parse(&request.constraints)?)
    };
    let follow = spec.as_ref().and_then(|s| s.follow);
    if follow.is_some() && curves.rotation.is_some() {
        bail!("[invalid-request] constraints.follow and a rotation curve both set the brush angle; use one")
    }
    let compiled_strokes = if curves.changes_style() || follow.is_some() {
        points.len() - 1
    } else {
        1
    };
    if compiled_strokes > MAX_COMPILED_STROKES {
        bail!("[limit-exceeded] property curves compile to {compiled_strokes} strokes; increase flatness or split the path (maximum {MAX_COMPILED_STROKES})")
    }
    let mut next = raw.clone();
    let mut resolved_summary = Value::Null;
    let mut previous_active = None;
    let mut guide = None;
    if let Some(spec) = &spec {
        let resolved = constraints::resolve(&next, id, spec)?;
        resolved_summary = resolved.summary.clone();
        if follow.is_some() {
            let node = all_rasters(&next)
                .into_iter()
                .find(|node| node["id"] == id)
                .context("[not-found] raster layer disappeared")?;
            guide = Some(Surface::load(&next, node)?);
        }
        if resolved.plane.iter().all(|c| *c == 0) {
            bail!("[empty-constraint] the constraints leave no paintable pixel; relax inside/avoid/barriers (mask {})", resolved.summary["mask_hash"])
        }
        previous_active = Some(constraints::install(&mut next, id, &resolved)?);
    }
    let mut dabs = 0_u64;
    let mut results = Vec::with_capacity(compiled_strokes);
    for index in 0..compiled_strokes {
        let (start, end, u) = if compiled_strokes == 1 {
            (0, points.len() - 1, 0.5)
        } else {
            (
                index,
                index + 1,
                ((lengths[index] + lengths[index + 1]) * 0.5) / total,
            )
        };
        let mut brush = request.brush.clone();
        if let Some(curve) = &curves.size {
            brush.size = curve.at(u)
        }
        if let Some(curve) = &curves.flow {
            brush.flow = curve.at(u)
        }
        if let Some(curve) = &curves.opacity {
            brush.opacity = curve.at(u)
        }
        if let Some(curve) = &curves.rotation {
            brush.angle = curve.at(u)
        }
        if let (Some(follow), Some(surface)) = (follow, &guide) {
            let mid = points[start].midpoint(points[end]);
            if let Some(milli) = constraints::follow_angle_milli(surface, follow, mid.x, mid.y) {
                brush.angle = milli as f64 / 1000.0;
            }
        }
        let color = curves
            .color
            .as_ref()
            .map_or(request.color, |curve| curve.at(u));
        let samples = if compiled_strokes == 1 {
            (start..=end).map(&sample).collect()
        } else {
            vec![sample(start), sample(end)]
        };
        let result = paint(
            &mut next,
            page,
            id,
            StrokeRequest {
                brush,
                samples,
                color,
                blend: request.blend,
                seed: request.seed.wrapping_add(index as u64),
                clone: None,
            },
        )?;
        dabs += result["dabs"].as_u64().unwrap_or(0);
        results.push(result);
    }
    if let Some(previous) = previous_active {
        constraints::restore_active(&mut next, id, previous);
    }
    *raw = next;
    Ok(json!({
        "constraints": resolved_summary,
        "source": {"path": request.path, "segments": source_segments, "flatness": request.flatness},
        "generated": {"points": points.len(), "samples": if compiled_strokes == 1 { points.len() } else { compiled_strokes * 2 }, "strokes": compiled_strokes, "dabs": dabs},
        "strokes": results.iter().filter_map(|result| result["stroke"].as_str()).collect::<Vec<_>>()
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_quadratic_cubic_and_arc_flatten_deterministically() {
        let (_, a) = flatten(
            "M0 0 L10 0 Q15 0 20 5 C25 10 30 10 35 5 A5 5 0 0 1 40 10",
            0.25,
        )
        .unwrap();
        let (_, b) = flatten(
            "M0 0 L10 0 Q15 0 20 5 C25 10 30 10 35 5 A5 5 0 0 1 40 10",
            0.25,
        )
        .unwrap();
        assert_eq!(a, b);
        assert_eq!(a.first(), Some(&Point::new(0.0, 0.0)));
        assert_eq!(a.last(), Some(&Point::new(40.0, 10.0)));
    }

    #[test]
    fn curves_require_strict_complete_distance_domains() {
        for bad in [
            json!([[0.1, 1], [1, 2]]),
            json!([[0, 1], [0, 2], [1, 3]]),
            json!([[0, 1], [0.9, 2]]),
        ] {
            assert!(ScalarCurve::parse("size", Some(&bad), 1.0, 10.0).is_err());
        }
    }
}
