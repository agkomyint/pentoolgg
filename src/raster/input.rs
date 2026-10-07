//! Input normalization and brush dynamics.
//!
//! Device events become canonical stroke samples once, at input time, and the
//! journal records only the canonical samples. Replay never sees timestamps or
//! raw event rates. Channels a device does not report stay absent; dynamics
//! then use their explicit fallback value.
use super::*;

/// Raw device events accepted by [`normalize_input`] before folding.
pub const MAX_EVENTS: usize = 65_536;
/// Events closer than this to the last kept sample are folded into it.
pub const FOLD_DISTANCE: f64 = 0.5;
/// Velocity is in pixels per millisecond and clamped to this.
pub const MAX_VELOCITY: f64 = 100.0;
const MAX_CURVE_POINTS: usize = 16;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sample {
    pub x: f64,
    pub y: f64,
    /// Always present; a device without pressure records 1.
    pub pressure: f64,
    /// Degrees from perpendicular, 0-90.
    pub tilt: Option<f64>,
    /// Degrees, 0 <= a < 360.
    pub azimuth: Option<f64>,
    /// Barrel rotation in degrees, 0 <= a < 360.
    pub twist: Option<f64>,
    /// Pixels per millisecond, 0-100.
    pub velocity: Option<f64>,
}

impl Sample {
    fn extended(&self) -> bool {
        self.tilt.is_some()
            || self.azimuth.is_some()
            || self.twist.is_some()
            || self.velocity.is_some()
    }
}

/// Samples are rounded to 1/1000 so a journal replays exactly.
fn canon(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

fn wrap_degrees(value: f64) -> f64 {
    let wrapped = value % 360.0;
    if wrapped < 0.0 {
        wrapped + 360.0
    } else {
        wrapped
    }
}

/// Interpolate every channel at `t` in [0, 1]. Azimuth and twist take the
/// shorter way around the circle.
pub(super) fn lerp(a: &Sample, b: &Sample, t: f64) -> Sample {
    let linear = |p: Option<f64>, q: Option<f64>| match (p, q) {
        (Some(p), Some(q)) => Some(p + (q - p) * t),
        (p, _) => p,
    };
    let angular = |p: Option<f64>, q: Option<f64>| match (p, q) {
        (Some(p), Some(q)) => {
            let mut d = (q - p) % 360.0;
            if d > 180.0 {
                d -= 360.0;
            } else if d < -180.0 {
                d += 360.0;
            }
            Some(wrap_degrees(p + d * t))
        }
        (p, _) => p,
    };
    Sample {
        x: a.x + (b.x - a.x) * t,
        y: a.y + (b.y - a.y) * t,
        pressure: a.pressure + (b.pressure - a.pressure) * t,
        tilt: linear(a.tilt, b.tilt),
        azimuth: angular(a.azimuth, b.azimuth),
        twist: angular(a.twist, b.twist),
        velocity: linear(a.velocity, b.velocity),
    }
}

/// Canonical journal form: `[x, y, pressure]`, or an object when the sample
/// carries tilt, azimuth, twist or velocity.
pub fn sample_json(sample: &Sample) -> Value {
    if !sample.extended() {
        return json!([sample.x, sample.y, sample.pressure]);
    }
    let mut out = json!({"x": sample.x, "y": sample.y, "pressure": sample.pressure});
    for (key, value) in [
        ("tilt", sample.tilt),
        ("azimuth", sample.azimuth),
        ("twist", sample.twist),
        ("velocity", sample.velocity),
    ] {
        if let Some(value) = value {
            out[key] = json!(value);
        }
    }
    out
}

const EVENT_KEYS: [&str; 8] = [
    "x", "y", "pressure", "tilt", "azimuth", "twist", "velocity", "t",
];

/// One event or sample. `t` (milliseconds) is accepted only from device input.
fn parse_point(item: &Value, index: usize, allow_t: bool) -> Result<(Sample, Option<f64>)> {
    let number = |value: Option<&Value>, key: &str| -> Result<Option<f64>> {
        match value {
            None => Ok(None),
            Some(value) => value
                .as_f64()
                .filter(|n| n.is_finite())
                .map(Some)
                .with_context(|| {
                    format!("[invalid-stroke] sample {index} {key} must be a finite number")
                }),
        }
    };
    let (x, y, pressure, mut sample, t) = match item {
        Value::Array(values) if (2..=3).contains(&values.len()) => (
            number(values.first(), "x")?,
            number(values.get(1), "y")?,
            number(values.get(2), "pressure")?,
            Sample::default(),
            None,
        ),
        Value::Object(object) => {
            if let Some(key) = object.keys().find(|k| !EVENT_KEYS.contains(&k.as_str())) {
                bail!(
                    "[invalid-stroke] sample {index} has unknown channel {key:?}; supported: {}",
                    EVENT_KEYS.join(", ")
                )
            }
            let ranged = |key: &str, hi: f64| -> Result<Option<f64>> {
                let value = number(object.get(key), key)?;
                if value.is_some_and(|v| !(0.0..=hi).contains(&v)) {
                    bail!("[invalid-stroke] sample {index} {key} must be between 0 and {hi}")
                }
                Ok(value)
            };
            let sample = Sample {
                tilt: ranged("tilt", 90.0)?,
                azimuth: ranged("azimuth", 360.0)?.map(wrap_degrees),
                twist: ranged("twist", 360.0)?.map(wrap_degrees),
                velocity: ranged("velocity", MAX_VELOCITY)?,
                ..Sample::default()
            };
            let t = number(object.get("t"), "t")?;
            if t.is_some() && !allow_t {
                bail!("[invalid-stroke] sample {index}: canonical samples record velocity, not t")
            }
            if t.is_some_and(|t| !(0.0..=1.0e9).contains(&t)) {
                bail!("[invalid-stroke] sample {index} t must be 0-1e9 milliseconds")
            }
            (
                number(object.get("x"), "x")?,
                number(object.get("y"), "y")?,
                number(object.get("pressure"), "pressure")?,
                sample,
                t,
            )
        }
        _ => (None, None, None, Sample::default(), None),
    };
    let (Some(x), Some(y)) = (x, y) else {
        bail!("[invalid-stroke] sample {index} must be [x, y], [x, y, pressure] or {{x, y, pressure, tilt, azimuth, twist, velocity}}")
    };
    if x.abs() > 1.0e6 || y.abs() > 1.0e6 {
        bail!("[invalid-stroke] sample {index} has an absurd coordinate")
    }
    let pressure = pressure.unwrap_or(1.0);
    if !(0.0..=1.0).contains(&pressure) {
        bail!("[invalid-stroke] sample {index} pressure must be between 0 and 1")
    }
    (sample.x, sample.y, sample.pressure) = (x, y, pressure);
    Ok((sample, t))
}

/// A channel must be reported on every sample of a stroke or on none.
fn check_channels(samples: &[Sample], times: &[Option<f64>]) -> Result<()> {
    let channels = |s: &Sample| [s.tilt, s.azimuth, s.twist, s.velocity];
    for (index, name) in ["tilt", "azimuth", "twist", "velocity"].iter().enumerate() {
        let count = samples
            .iter()
            .filter(|s| channels(s)[index].is_some())
            .count();
        if count != 0 && count != samples.len() {
            bail!("[invalid-stroke] {name} must be reported on every sample of a stroke or on none")
        }
    }
    let timed = times.iter().filter(|t| t.is_some()).count();
    if timed != 0 && timed != times.len() {
        bail!("[invalid-stroke] t must be reported on every event of a stroke or on none")
    }
    if timed != 0 && samples.iter().any(|s| s.velocity.is_some()) {
        bail!("[invalid-stroke] give either t or velocity, not both")
    }
    Ok(())
}

fn canonical(sample: Sample) -> Sample {
    Sample {
        x: canon(sample.x),
        y: canon(sample.y),
        pressure: canon(sample.pressure),
        tilt: sample.tilt.map(canon),
        azimuth: sample.azimuth.map(|a| wrap_degrees(canon(a))),
        twist: sample.twist.map(|a| wrap_degrees(canon(a))),
        velocity: sample.velocity.map(canon),
    }
}

/// Parse canonical samples, as recorded in a journal. No folding happens here.
pub fn parse_samples(value: &Value) -> Result<Vec<Sample>> {
    let list = value
        .as_array()
        .context("[invalid-stroke] samples must be an array")?;
    if list.is_empty() || list.len() > MAX_SAMPLES {
        bail!(
            "[limit-exceeded] a stroke needs 1-{MAX_SAMPLES} samples, got {}",
            list.len()
        )
    }
    let mut out = Vec::with_capacity(list.len());
    for (index, item) in list.iter().enumerate() {
        out.push(canonical(parse_point(item, index, false)?.0));
    }
    check_channels(&out, &[])?;
    Ok(out)
}

pub struct NormalizedInput {
    pub samples: Vec<Sample>,
    /// Device events received.
    pub events: usize,
    /// Events folded into a neighbor closer than [`FOLD_DISTANCE`].
    pub folded: usize,
}

impl NormalizedInput {
    pub fn summary(&self) -> Value {
        json!({"events": self.events, "samples": self.samples.len(), "folded": self.folded})
    }
}

/// Normalize device events into canonical stroke samples:
///
/// 1. validate each event and that every optional channel is on all events or none;
/// 2. with `t`, derive velocity `distance / dt` per event in px/ms, clamped to
///    [`MAX_VELOCITY`]. A zero `dt` repeats the previous velocity, and the first
///    event copies the second;
/// 3. fold any event closer than [`FOLD_DISTANCE`] px to the last kept sample, but
///    always keep the final event so the stroke ends where the pen lifted;
/// 4. round to 1/1000 and enforce [`MAX_SAMPLES`].
pub fn normalize_input(value: &Value) -> Result<NormalizedInput> {
    let list = value
        .as_array()
        .context("[invalid-stroke] samples must be an array of events")?;
    if list.is_empty() || list.len() > MAX_EVENTS {
        bail!(
            "[limit-exceeded] a stroke needs 1-{MAX_EVENTS} input events, got {}",
            list.len()
        )
    }
    let mut events = Vec::with_capacity(list.len());
    let mut times = Vec::with_capacity(list.len());
    for (index, item) in list.iter().enumerate() {
        let (sample, t) = parse_point(item, index, true)?;
        events.push(sample);
        times.push(t);
    }
    check_channels(&events, &times)?;
    if times.first().is_some_and(Option::is_some) {
        let times: Vec<f64> = times.iter().map(|t| t.unwrap_or(0.0)).collect();
        if times.windows(2).any(|w| w[1] < w[0]) {
            bail!("[invalid-stroke] event t must not decrease")
        }
        let mut velocity = vec![0.0; events.len()];
        for i in 1..events.len() {
            let (dx, dy) = (events[i].x - events[i - 1].x, events[i].y - events[i - 1].y);
            let dt = times[i] - times[i - 1];
            velocity[i] = if dt > 0.0 {
                ((dx * dx + dy * dy).sqrt() / dt).min(MAX_VELOCITY)
            } else {
                velocity[i - 1]
            };
        }
        if events.len() > 1 {
            velocity[0] = velocity[1];
        }
        for (event, v) in events.iter_mut().zip(velocity) {
            event.velocity = Some(v);
        }
    }
    let total = events.len();
    let mut kept: Vec<Sample> = Vec::with_capacity(total.min(MAX_SAMPLES + 1));
    for (index, event) in events.into_iter().enumerate() {
        let is_last = index + 1 == total;
        let near = kept.last().is_some_and(|last| {
            let (dx, dy) = (event.x - last.x, event.y - last.y);
            dx * dx + dy * dy < FOLD_DISTANCE * FOLD_DISTANCE
        });
        if !near || is_last {
            kept.push(canonical(event));
        }
        if kept.len() > MAX_SAMPLES {
            bail!("[limit-exceeded] the input folds to more than {MAX_SAMPLES} samples; split the stroke")
        }
    }
    Ok(NormalizedInput {
        folded: total - kept.len(),
        samples: kept,
        events: total,
    })
}

// ---------------------------------------------------------------------------
// Dynamics
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    Pressure,
    Tilt,
    Azimuth,
    Twist,
    Velocity,
}

impl Input {
    fn parse(text: &str) -> Result<Self> {
        Ok(match text {
            "pressure" => Self::Pressure,
            "tilt" => Self::Tilt,
            "azimuth" => Self::Azimuth,
            "twist" => Self::Twist,
            "velocity" => Self::Velocity,
            other => bail!("[invalid-brush] dynamics input {other:?} is not supported; use pressure, tilt, azimuth, twist or velocity"),
        })
    }
    fn name(self) -> &'static str {
        match self {
            Self::Pressure => "pressure",
            Self::Tilt => "tilt",
            Self::Azimuth => "azimuth",
            Self::Twist => "twist",
            Self::Velocity => "velocity",
        }
    }
    fn domain(self) -> f64 {
        match self {
            Self::Pressure => 1.0,
            Self::Tilt => 90.0,
            Self::Azimuth | Self::Twist => 360.0,
            Self::Velocity => MAX_VELOCITY,
        }
    }
    /// The stable value used when a device does not report this input.
    fn default_fallback(self) -> f64 {
        match self {
            Self::Pressure => 1.0,
            _ => 0.0,
        }
    }
    fn read(self, sample: &Sample) -> Option<f64> {
        match self {
            Self::Pressure => Some(sample.pressure),
            Self::Tilt => sample.tilt,
            Self::Azimuth => sample.azimuth,
            Self::Twist => sample.twist,
            Self::Velocity => sample.velocity,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Dynamic {
    pub input: Input,
    /// Piecewise-linear `[input, output]` points with strictly increasing input.
    pub curve: Vec<[f64; 2]>,
    pub fallback: f64,
}

impl Dynamic {
    pub fn eval(&self, sample: &Sample) -> f64 {
        let v = self.input.read(sample).unwrap_or(self.fallback);
        let (first, last) = (self.curve[0], self.curve[self.curve.len() - 1]);
        if v <= first[0] {
            return first[1];
        }
        if v >= last[0] {
            return last[1];
        }
        for pair in self.curve.windows(2) {
            let ([xa, ya], [xb, yb]) = (pair[0], pair[1]);
            if v <= xb {
                return ya + (yb - ya) * (v - xa) / (xb - xa);
            }
        }
        last[1]
    }

    fn min_output(&self) -> f64 {
        self.curve
            .iter()
            .map(|p| p[1])
            .fold(f64::INFINITY, f64::min)
    }

    fn to_json(&self) -> Value {
        json!({"input": self.input.name(), "curve": self.curve, "fallback": self.fallback})
    }
}

/// Brush properties driven by inputs. `size`, `flow` and `roundness` multiply the
/// brush value (curve outputs 0-1); `angle` adds degrees (-360 to 360).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Dynamics {
    pub size: Option<Dynamic>,
    pub flow: Option<Dynamic>,
    pub roundness: Option<Dynamic>,
    pub angle: Option<Dynamic>,
}

const TARGETS: [&str; 4] = ["size", "flow", "roundness", "angle"];

impl Dynamics {
    pub fn is_empty(&self) -> bool {
        self.size.is_none()
            && self.flow.is_none()
            && self.roundness.is_none()
            && self.angle.is_none()
    }

    pub fn parse(value: Option<&Value>) -> Result<Self> {
        let Some(value) = value else {
            return Ok(Self::default());
        };
        let object = value
            .as_object()
            .context("[invalid-brush] dynamics must be an object")?;
        if let Some(key) = object.keys().find(|k| !TARGETS.contains(&k.as_str())) {
            bail!(
                "[invalid-brush] unknown dynamics target {key:?}; supported: {}",
                TARGETS.join(", ")
            )
        }
        let parse = |target: &str| -> Result<Option<Dynamic>> {
            object
                .get(target)
                .map(|spec| parse_dynamic(target, spec))
                .transpose()
        };
        Ok(Self {
            size: parse("size")?,
            flow: parse("flow")?,
            roundness: parse("roundness")?,
            angle: parse("angle")?,
        })
    }

    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        for (key, dynamic) in [
            ("size", &self.size),
            ("flow", &self.flow),
            ("roundness", &self.roundness),
            ("angle", &self.angle),
        ] {
            if let Some(dynamic) = dynamic {
                out.insert(key.into(), dynamic.to_json());
            }
        }
        Value::Object(out)
    }

    /// The smallest roundness multiplier any input can produce; bounds stroke work.
    pub fn min_roundness(&self) -> f64 {
        self.roundness.as_ref().map_or(1.0, Dynamic::min_output)
    }
}

fn parse_dynamic(target: &str, spec: &Value) -> Result<Dynamic> {
    let object = spec
        .as_object()
        .with_context(|| format!("[invalid-brush] dynamics.{target} must be an object"))?;
    if let Some(key) = object
        .keys()
        .find(|k| !["input", "curve", "fallback"].contains(&k.as_str()))
    {
        bail!("[invalid-brush] dynamics.{target} has unknown property {key:?}; supported: input, curve, fallback")
    }
    let input = Input::parse(
        object
            .get("input")
            .and_then(Value::as_str)
            .with_context(|| format!("[invalid-brush] dynamics.{target}.input is required"))?,
    )?;
    let domain = input.domain();
    let (lo, hi) = if target == "angle" {
        (-360.0, 360.0)
    } else {
        (0.0, 1.0)
    };
    let curve: Vec<[f64; 2]> = match object.get("curve") {
        Some(curve) => {
            let points = curve
                .as_array()
                .filter(|p| (2..=MAX_CURVE_POINTS).contains(&p.len()))
                .with_context(|| {
                    format!("[invalid-brush] dynamics.{target}.curve needs 2-{MAX_CURVE_POINTS} [input, output] points")
                })?;
            let mut out = Vec::with_capacity(points.len());
            for point in points {
                let pair = point
                    .as_array()
                    .filter(|p| p.len() == 2)
                    .and_then(|p| Some([p[0].as_f64()?, p[1].as_f64()?]))
                    .filter(|[x, y]| x.is_finite() && y.is_finite())
                    .with_context(|| {
                        format!("[invalid-brush] dynamics.{target}.curve points must be [input, output] numbers")
                    })?;
                out.push(pair);
            }
            out
        }
        // Defaults: multipliers ramp 0 -> 1 over the input; an angle follows an
        // angular input one-to-one. Other angle inputs need an explicit curve.
        None if target != "angle" => vec![[0.0, 0.0], [domain, 1.0]],
        None if matches!(input, Input::Azimuth | Input::Twist | Input::Tilt) => {
            vec![[0.0, 0.0], [domain, domain]]
        }
        None => bail!(
            "[invalid-brush] dynamics.angle from {} needs an explicit curve",
            input.name()
        ),
    };
    for [x, y] in &curve {
        if !(0.0..=domain).contains(x) {
            bail!(
                "[invalid-brush] dynamics.{target}.curve input {x} is outside the {} range 0-{domain}",
                input.name()
            )
        }
        if !(lo..=hi).contains(y) {
            bail!(
                "[invalid-brush] dynamics.{target}.curve output {y} must be between {lo} and {hi}"
            )
        }
    }
    if curve.windows(2).any(|w| w[1][0] <= w[0][0]) {
        bail!("[invalid-brush] dynamics.{target}.curve inputs must strictly increase")
    }
    let fallback = match object.get("fallback") {
        None => input.default_fallback(),
        Some(value) => value
            .as_f64()
            .filter(|v| (0.0..=domain).contains(v))
            .with_context(|| {
                format!(
                    "[invalid-brush] dynamics.{target}.fallback must be a {} value 0-{domain}",
                    input.name()
                )
            })?,
    };
    Ok(Dynamic {
        input,
        curve,
        fallback,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surface() -> Surface {
        Surface {
            width: 300,
            height: 200,
            tiles: BTreeMap::new(),
        }
    }

    fn dot(brush: Value, sample: Value) -> Surface {
        let mut s = surface();
        let stroke = Stroke {
            brush: Brush::parse(&brush).unwrap(),
            samples: parse_samples(&json!([sample])).unwrap(),
            color: [0, 0, 0],
            blend: Blend::Normal,
            clone: None,
            seed: 0,
            tip: None,
        };
        apply_stroke(&mut s, &stroke).unwrap();
        s
    }

    #[test]
    fn velocity_comes_from_t_and_close_events_fold() {
        let input = normalize_input(&json!([
            {"x": 0, "y": 0, "t": 0},
            {"x": 10, "y": 0, "t": 5},
            {"x": 10.2, "y": 0, "t": 6},
            {"x": 10, "y": 30, "t": 6},
            {"x": 10.1, "y": 30, "t": 9}
        ]))
        .unwrap();
        let v: Vec<f64> = input.samples.iter().map(|s| s.velocity.unwrap()).collect();
        // Event 2 (0.2 px away) folds; the last event is kept though it is close.
        assert_eq!(input.events, 5);
        assert_eq!(input.folded, 1);
        let xs: Vec<f64> = input.samples.iter().map(|s| s.x).collect();
        assert_eq!(xs, [0.0, 10.0, 10.0, 10.1]);
        // v0 copies v1; dt = 0 repeats the previous velocity.
        assert_eq!(v, [2.0, 2.0, 0.2, 0.033]);
        assert_eq!(input.summary()["samples"], 4);
        // A single event survives.
        assert_eq!(normalize_input(&json!([[5, 5]])).unwrap().samples.len(), 1);
    }

    #[test]
    fn malformed_device_input_is_refused() {
        for bad in [
            json!([{"x": 0, "y": 0, "t": 5}, {"x": 9, "y": 0, "t": 4}]),
            json!([{"x": 0, "y": 0, "t": 0, "velocity": 1}]),
            json!([{"x": 0, "y": 0, "tilt": 10}, {"x": 9, "y": 0}]),
            json!([{"x": 0, "y": 0, "t": 1}, {"x": 9, "y": 0}]),
            json!([{"x": 0, "y": 0, "tilt": 91}]),
            json!([{"x": 0, "y": 0, "azimuth": -1}]),
            json!([{"x": 0, "y": 0, "speed": 1}]),
            json!([{"x": 0, "y": 0, "pressure": null}]),
            json!([]),
        ] {
            assert!(normalize_input(&bad).is_err(), "{bad}");
        }
        // Journals carry velocity, never t.
        assert!(parse_samples(&json!([{"x": 0, "y": 0, "t": 1}])).is_err());
    }

    #[test]
    fn extended_samples_round_trip_through_the_journal_form() {
        let samples = parse_samples(&json!([
            {"x": 1.00049, "y": 2, "pressure": 0.5, "tilt": 30, "azimuth": 360, "twist": 12.5},
            {"x": 4, "y": 5, "tilt": 45, "azimuth": 359.5, "twist": 0}
        ]))
        .unwrap();
        assert_eq!(samples[0].x, 1.0);
        assert_eq!(samples[0].azimuth, Some(0.0), "360 wraps to 0");
        let journal: Vec<Value> = samples.iter().map(sample_json).collect();
        assert!(journal[0].get("velocity").is_none());
        assert_eq!(parse_samples(&json!(journal)).unwrap(), samples);
        // Plain samples keep the compact array form.
        assert_eq!(
            sample_json(&parse_samples(&json!([[1, 2]])).unwrap()[0]),
            json!([1.0, 2.0, 1.0])
        );
        // Azimuth interpolates the short way across 0.
        let mid = lerp(&samples[0], &samples[1], 0.5);
        assert_eq!(mid.azimuth, Some(359.75));
    }

    #[test]
    fn curves_interpolate_clamp_and_fall_back() {
        let dynamics = Dynamics::parse(Some(&json!({
            "size": {"input": "pressure", "curve": [[0.2, 0.1], [0.6, 0.5], [1, 1]]},
            "flow": {"input": "tilt", "fallback": 45}
        })))
        .unwrap();
        let size = dynamics.size.as_ref().unwrap();
        let at = |pressure| Sample {
            pressure,
            ..Sample::default()
        };
        assert_eq!(size.eval(&at(0.0)), 0.1);
        assert!((size.eval(&at(0.4)) - 0.3).abs() < 1e-12);
        assert!((size.eval(&at(0.8)) - 0.75).abs() < 1e-12);
        assert_eq!(size.eval(&at(1.0)), 1.0);
        let flow = dynamics.flow.as_ref().unwrap();
        assert_eq!(
            flow.eval(&at(1.0)),
            0.5,
            "no tilt reported: fallback 45 of 90"
        );
        let tilted = Sample {
            tilt: Some(90.0),
            ..at(1.0)
        };
        assert_eq!(flow.eval(&tilted), 1.0);
        let parsed = Dynamics::parse(Some(&dynamics.to_json())).unwrap();
        assert_eq!(parsed, dynamics);
        for bad in [
            json!({"spin": {"input": "pressure"}}),
            json!({"size": {"input": "gravity"}}),
            json!({"size": {"input": "pressure", "curve": [[0, 0]]}}),
            json!({"size": {"input": "pressure", "curve": [[0.5, 0], [0.5, 1]]}}),
            json!({"size": {"input": "pressure", "curve": [[0, 0], [2, 1]]}}),
            json!({"size": {"input": "pressure", "curve": [[0, 0], [1, 1.5]]}}),
            json!({"size": {"input": "tilt", "fallback": 120}}),
            json!({"angle": {"input": "pressure"}}),
            json!({"size": {"input": "pressure", "gain": 2}}),
        ] {
            assert!(Dynamics::parse(Some(&bad)).is_err(), "{bad}");
        }
        assert!(Brush::parse(
            &json!({"pressure_size": true, "dynamics": {"size": {"input": "pressure"}}})
        )
        .is_err());
    }

    #[test]
    fn angle_follows_azimuth() {
        let brush = json!({"kind": "calligraphic", "size": 40, "angle": 0, "roundness": 0.25,
            "dynamics": {"angle": {"input": "azimuth"}}});
        let flat = dot(brush.clone(), json!({"x": 100, "y": 100, "azimuth": 0}));
        let upright = dot(brush.clone(), json!({"x": 100, "y": 100, "azimuth": 90}));
        assert_eq!(flat.pixel(115, 100)[3], 255);
        assert_eq!(flat.pixel(100, 115)[3], 0);
        assert_eq!(upright.pixel(115, 100)[3], 0);
        assert_eq!(upright.pixel(100, 115)[3], 255);
        // A mouse reports no azimuth: the fallback (0) applies.
        let mouse = dot(brush, json!([100, 100]));
        assert_eq!(mouse.tile_map_hash(), flat.tile_map_hash());
    }

    #[test]
    fn tilt_drives_size_with_a_stable_fallback() {
        let brush = |fallback: f64| json!({"size": 40, "dynamics": {"size": {"input": "tilt", "fallback": fallback}}});
        let thin = dot(brush(0.0), json!([100, 100]));
        let full = dot(brush(90.0), json!([100, 100]));
        assert_eq!(full.pixel(115, 100)[3], 255);
        assert_eq!(thin.pixel(115, 100)[3], 0);
        let tilted = dot(brush(0.0), json!({"x": 100, "y": 100, "tilt": 90}));
        assert_eq!(tilted.tile_map_hash(), full.tile_map_hash());
    }
}
