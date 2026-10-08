//! Lens profiles: pentool's JSON lens profile format (`pentool_lens_profile: 1`),
//! interpolation at a capture's focal length and aperture, and the bounded Adobe
//! LCP subset that `photo profile import-lcp` converts.
//!
//! A sample holds the `WarpRectilinear` radial and tangential model, a vignette
//! (the `FixVignetteRadial` gain polynomial or an LCP falloff polynomial) and
//! lateral chromatic aberration scales for red and blue.
use anyhow::{bail, Context, Result};
use serde_json::{json, Map, Value};

pub const MAX_SAMPLES: usize = 1024;
pub const MAX_UNSUPPORTED: usize = 256;
const MAX_XML_ELEMENTS: usize = 200_000;
const MAX_XML_DEPTH: usize = 64;

/// How a vignette's radius is measured, and its polynomial.
#[derive(Debug, Clone, PartialEq)]
pub enum VignetteModel {
    /// `FixVignetteRadial`: gain `1 + k0 r^2 + k1 r^4 + ... + k4 r^10`.
    Gain([f64; 5]),
    /// LCP: falloff `1 + a1 r^2 + a2 r^4 + a3 r^6`; the gain is its inverse.
    Falloff([f64; 3]),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Vignette {
    pub model: VignetteModel,
    /// Radius unit as a fraction of the long edge; `None` is the corner distance.
    pub scale: Option<f64>,
    /// Optical center from the top-left, in long-edge units; `None` is the image center.
    pub center: Option<[f64; 2]>,
}

/// One calibrated focal length and aperture.
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    pub focal: f64,
    pub aperture: f64,
    pub scale: Option<f64>,
    pub center: Option<[f64; 2]>,
    /// `k1, k2, k3`.
    pub radial: Option<[f64; 3]>,
    /// `t0, t1` (`kt0`, `kt1` of `WarpRectilinear`).
    pub tangential: [f64; 2],
    pub vignette: Option<Vignette>,
    /// Lateral chromatic aberration: red and blue radial scales relative to green.
    pub chromatic_aberration: Option<[f64; 2]>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LensProfile {
    pub make: String,
    pub model: String,
    pub focal_range: [f64; 2],
    pub aperture_range: [f64; 2],
    pub samples: Vec<Sample>,
}

fn invalid(what: &str) -> anyhow::Error {
    anyhow::anyhow!("[malformed-resource] lens profile {what}")
}

fn keys(object: &Map<String, Value>, allowed: &[&str], what: &str) -> Result<()> {
    if let Some(key) = object.keys().find(|k| !allowed.contains(&k.as_str())) {
        bail!(
            "[malformed-resource] lens profile {what} has unknown field {key:?}; expected one of {}",
            allowed.join(", ")
        )
    }
    Ok(())
}

fn numbers<const N: usize>(
    value: Option<&Value>,
    max_len: usize,
    range: (f64, f64),
    what: &str,
) -> Result<Option<[f64; N]>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let values: Option<Vec<f64>> = value
        .as_array()
        .filter(|v| !v.is_empty() && v.len() <= max_len)
        .and_then(|v| v.iter().map(Value::as_f64).collect());
    match values {
        Some(v) if v.iter().all(|x| (range.0..=range.1).contains(x)) => {
            let mut out = [0.0; N];
            out[..v.len()].copy_from_slice(&v);
            Ok(Some(out))
        }
        _ => Err(invalid(&format!(
            "{what} must hold 1–{max_len} numbers in {}–{}",
            range.0, range.1
        ))),
    }
}

fn positive(value: Option<&Value>, max: f64, what: &str) -> Result<Option<f64>> {
    match value {
        None => Ok(None),
        Some(v) => match v.as_f64() {
            Some(x) if x > 0.0 && x <= max => Ok(Some(x)),
            _ => Err(invalid(&format!("{what} must be a number in (0, {max}]"))),
        },
    }
}

fn text(object: &Map<String, Value>, key: &str) -> Result<String> {
    match object.get(key).and_then(Value::as_str) {
        Some(s) if !s.trim().is_empty() && s.chars().count() <= 128 => Ok(s.to_string()),
        _ => Err(invalid(&format!("{key} must be 1–128 characters"))),
    }
}

fn range(object: &Map<String, Value>, key: &str, max: f64) -> Result<[f64; 2]> {
    let r: Option<[f64; 2]> = numbers::<2>(object.get(key), 2, (f64::MIN_POSITIVE, max), key)?;
    match r {
        Some([a, b]) if a <= b && object[key].as_array().map(Vec::len) == Some(2) => Ok([a, b]),
        _ => Err(invalid(&format!(
            "{key} must be [min, max] with 0 < min ≤ max ≤ {max}"
        ))),
    }
}

fn read_vignette(value: &Value, what: &str) -> Result<Vignette> {
    let v = value
        .as_object()
        .ok_or_else(|| invalid(&format!("{what} must be an object")))?;
    keys(v, &["gain", "falloff", "scale", "center"], what)?;
    let model = match (v.get("gain"), v.get("falloff")) {
        (Some(_), None) => VignetteModel::Gain(
            numbers::<5>(v.get("gain"), 5, (-100.0, 100.0), &format!("{what}.gain"))?.unwrap(),
        ),
        (None, Some(_)) => VignetteModel::Falloff(
            numbers::<3>(
                v.get("falloff"),
                3,
                (-100.0, 100.0),
                &format!("{what}.falloff"),
            )?
            .unwrap(),
        ),
        _ => {
            return Err(invalid(&format!(
                "{what} needs exactly one of gain or falloff"
            )))
        }
    };
    Ok(Vignette {
        model,
        scale: positive(v.get("scale"), 10.0, &format!("{what}.scale"))?,
        center: numbers::<2>(v.get("center"), 2, (0.0, 1.0), &format!("{what}.center"))?,
    })
}

/// Which optional parts a sample has; every sample of a profile must match.
type Shape = (bool, bool, bool, bool, Option<(bool, bool, bool)>);

fn shape(sample: &Sample) -> Shape {
    (
        sample.scale.is_some(),
        sample.center.is_some(),
        sample.radial.is_some(),
        sample.chromatic_aberration.is_some(),
        sample.vignette.as_ref().map(|v| {
            (
                matches!(v.model, VignetteModel::Gain(_)),
                v.scale.is_some(),
                v.center.is_some(),
            )
        }),
    )
}

impl LensProfile {
    /// Parse and verify a pentool lens profile.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() as u64 > super::catalog::MAX_PROFILE_BYTES {
            bail!("[limit-exceeded] lens profile is larger than 16 MiB")
        }
        let value: Value = serde_json::from_slice(bytes)
            .map_err(|e| invalid(&format!("is not valid JSON: {e}")))?;
        let root = value
            .as_object()
            .ok_or_else(|| invalid("must be a JSON object"))?;
        keys(
            root,
            &[
                "pentool_lens_profile",
                "make",
                "model",
                "focal_range",
                "aperture_range",
                "samples",
            ],
            "",
        )?;
        if root.get("pentool_lens_profile").and_then(Value::as_u64) != Some(1) {
            bail!("[unsupported-capability] lens profile must declare \"pentool_lens_profile\": 1")
        }
        let make = text(root, "make")?;
        let model = text(root, "model")?;
        let focal_range = range(root, "focal_range", 10_000.0)?;
        let aperture_range = range(root, "aperture_range", 1000.0)?;
        let raw = root
            .get("samples")
            .and_then(Value::as_array)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| invalid("samples must be a nonempty array"))?;
        if raw.len() > MAX_SAMPLES {
            bail!(
                "[limit-exceeded] lens profile has {} samples; the limit is {MAX_SAMPLES}",
                raw.len()
            )
        }
        let mut samples = Vec::with_capacity(raw.len());
        for (index, value) in raw.iter().enumerate() {
            let what = format!("samples[{index}]");
            let s = value
                .as_object()
                .ok_or_else(|| invalid(&format!("{what} must be an object")))?;
            keys(
                s,
                &[
                    "focal",
                    "aperture",
                    "scale",
                    "center",
                    "distortion",
                    "vignette",
                    "chromatic_aberration",
                ],
                &what,
            )?;
            let focal = positive(s.get("focal"), 10_000.0, &format!("{what}.focal"))?
                .ok_or_else(|| invalid(&format!("{what}.focal is required")))?;
            let aperture = positive(s.get("aperture"), 1000.0, &format!("{what}.aperture"))?
                .ok_or_else(|| invalid(&format!("{what}.aperture is required")))?;
            if !(focal_range[0]..=focal_range[1]).contains(&focal)
                || !(aperture_range[0]..=aperture_range[1]).contains(&aperture)
            {
                return Err(invalid(&format!(
                    "{what} lies outside focal_range or aperture_range"
                )));
            }
            let (radial, tangential) = match s.get("distortion") {
                None => (None, [0.0; 2]),
                Some(d) => {
                    let where_ = format!("{what}.distortion");
                    let d = d
                        .as_object()
                        .ok_or_else(|| invalid(&format!("{where_} must be an object")))?;
                    keys(d, &["radial", "tangential"], &where_)?;
                    let radial = numbers::<3>(
                        d.get("radial"),
                        3,
                        (-10.0, 10.0),
                        &format!("{where_}.radial"),
                    )?
                    .ok_or_else(|| invalid(&format!("{where_}.radial is required")))?;
                    let tangential = numbers::<2>(
                        d.get("tangential"),
                        2,
                        (-1.0, 1.0),
                        &format!("{where_}.tangential"),
                    )?
                    .unwrap_or([0.0; 2]);
                    (Some(radial), tangential)
                }
            };
            let chromatic_aberration = match s.get("chromatic_aberration") {
                None => None,
                Some(ca) => {
                    let where_ = format!("{what}.chromatic_aberration");
                    let ca = ca
                        .as_object()
                        .ok_or_else(|| invalid(&format!("{where_} must be an object")))?;
                    keys(ca, &["red", "blue"], &where_)?;
                    let scale = |key: &str| -> Result<f64> {
                        match ca.get(key).and_then(Value::as_f64) {
                            Some(v) if (0.9..=1.1).contains(&v) => Ok(v),
                            None if !ca.contains_key(key) => Ok(1.0),
                            _ => Err(invalid(&format!("{where_}.{key} must be in 0.9–1.1"))),
                        }
                    };
                    Some([scale("red")?, scale("blue")?])
                }
            };
            let sample = Sample {
                focal,
                aperture,
                scale: positive(s.get("scale"), 10.0, &format!("{what}.scale"))?,
                center: numbers::<2>(s.get("center"), 2, (0.0, 1.0), &format!("{what}.center"))?,
                radial,
                tangential,
                vignette: s
                    .get("vignette")
                    .map(|v| read_vignette(v, &format!("{what}.vignette")))
                    .transpose()?,
                chromatic_aberration,
            };
            if s.get("center")
                .and_then(Value::as_array)
                .is_some_and(|c| c.len() != 2)
            {
                return Err(invalid(&format!("{what}.center must be [x, y]")));
            }
            if let Some(first) = samples.first() {
                if shape(first) != shape(&sample) {
                    return Err(invalid(&format!(
                        "{what} must have the same fields and vignette model as samples[0] so samples can be interpolated"
                    )));
                }
            }
            if samples
                .iter()
                .any(|o: &Sample| o.focal == focal && o.aperture == aperture)
            {
                return Err(invalid(&format!(
                    "{what} repeats focal {focal} and aperture {aperture}"
                )));
            }
            samples.push(sample);
        }
        Ok(Self {
            make,
            model,
            focal_range,
            aperture_range,
            samples,
        })
    }

    pub fn name(&self) -> String {
        format!("{} {}", self.make, self.model)
    }

    /// The parameters at `focal` and `aperture`: linear in focal length between the
    /// two bracketing calibrated focal lengths, and at each of those linear in
    /// `1/aperture`. Values outside the calibrated set are clamped to it.
    pub fn at(&self, focal: f64, aperture: f64) -> Sample {
        let mut focals: Vec<f64> = self.samples.iter().map(|s| s.focal).collect();
        focals.sort_by(f64::total_cmp);
        focals.dedup();
        let (fa, fb, t) = bracket(&focals, focal);
        let at_focal = |f: f64| -> Sample {
            let mut set: Vec<&Sample> = self.samples.iter().filter(|s| s.focal == f).collect();
            set.sort_by(|a, b| (1.0 / a.aperture).total_cmp(&(1.0 / b.aperture)));
            let inverse: Vec<f64> = set.iter().map(|s| 1.0 / s.aperture).collect();
            let (ia, ib, u) = bracket(&inverse, 1.0 / aperture);
            let index = |v: f64| inverse.iter().position(|x| *x == v).unwrap();
            lerp(set[index(ia)], set[index(ib)], u)
        };
        let mut sample = lerp(&at_focal(fa), &at_focal(fb), t);
        sample.focal = focal.clamp(focals[0], focals[focals.len() - 1]);
        sample.aperture = aperture;
        sample
    }

    /// Canonical JSON bytes of this profile.
    pub fn to_bytes(&self) -> Vec<u8> {
        let samples: Vec<Value> = self
            .samples
            .iter()
            .map(|s| {
                let mut o = Map::new();
                o.insert("focal".into(), num(s.focal));
                o.insert("aperture".into(), num(s.aperture));
                if let Some(scale) = s.scale {
                    o.insert("scale".into(), num(scale));
                }
                if let Some(c) = s.center {
                    o.insert("center".into(), json!([num(c[0]), num(c[1])]));
                }
                if let Some(r) = s.radial {
                    let mut d = Map::new();
                    d.insert("radial".into(), json!(r.map(num)));
                    if s.tangential != [0.0; 2] {
                        d.insert("tangential".into(), json!(s.tangential.map(num)));
                    }
                    o.insert("distortion".into(), Value::Object(d));
                }
                if let Some(v) = &s.vignette {
                    let mut m = Map::new();
                    match &v.model {
                        VignetteModel::Gain(k) => m.insert("gain".into(), json!(k.map(num))),
                        VignetteModel::Falloff(a) => m.insert("falloff".into(), json!(a.map(num))),
                    };
                    if let Some(scale) = v.scale {
                        m.insert("scale".into(), num(scale));
                    }
                    if let Some(c) = v.center {
                        m.insert("center".into(), json!([num(c[0]), num(c[1])]));
                    }
                    o.insert("vignette".into(), Value::Object(m));
                }
                if let Some([r, b]) = s.chromatic_aberration {
                    o.insert(
                        "chromatic_aberration".into(),
                        json!({"red": num(r), "blue": num(b)}),
                    );
                }
                Value::Object(o)
            })
            .collect();
        let value = json!({
            "pentool_lens_profile": 1,
            "make": self.make,
            "model": self.model,
            "focal_range": self.focal_range.map(num),
            "aperture_range": self.aperture_range.map(num),
            "samples": samples,
        });
        let mut bytes = serde_json::to_vec_pretty(&value).expect("serializable");
        bytes.push(b'\n');
        bytes
    }
}

fn num(v: f64) -> Value {
    super::dng::number(v)
}

/// The two neighbours of `x` in the sorted `values` and the weight of the second.
fn bracket(values: &[f64], x: f64) -> (f64, f64, f64) {
    let first = values[0];
    let last = values[values.len() - 1];
    if x <= first || x.is_nan() {
        return (first, first, 0.0);
    }
    if x >= last {
        return (last, last, 0.0);
    }
    let upper = values.iter().position(|v| *v >= x).unwrap();
    let (a, b) = (values[upper - 1], values[upper]);
    (a, b, (x - a) / (b - a))
}

fn mix(a: f64, b: f64, t: f64) -> f64 {
    if t == 0.0 {
        a
    } else {
        a + (b - a) * t
    }
}

fn mix_n<const N: usize>(a: [f64; N], b: [f64; N], t: f64) -> [f64; N] {
    std::array::from_fn(|i| mix(a[i], b[i], t))
}

fn mix_option<const N: usize>(
    a: Option<[f64; N]>,
    b: Option<[f64; N]>,
    t: f64,
) -> Option<[f64; N]> {
    match (a, b) {
        (Some(a), Some(b)) => Some(mix_n(a, b, t)),
        _ => None,
    }
}

/// Samples of one profile share their shape, which `parse` verifies.
fn lerp(a: &Sample, b: &Sample, t: f64) -> Sample {
    Sample {
        focal: mix(a.focal, b.focal, t),
        aperture: mix(a.aperture, b.aperture, t),
        scale: mix_option(a.scale.map(|v| [v]), b.scale.map(|v| [v]), t).map(|v| v[0]),
        center: mix_option(a.center, b.center, t),
        radial: mix_option(a.radial, b.radial, t),
        tangential: mix_n(a.tangential, b.tangential, t),
        vignette: match (&a.vignette, &b.vignette) {
            (Some(x), Some(y)) => Some(Vignette {
                model: match (&x.model, &y.model) {
                    (VignetteModel::Gain(p), VignetteModel::Gain(q)) => {
                        VignetteModel::Gain(mix_n(*p, *q, t))
                    }
                    (VignetteModel::Falloff(p), VignetteModel::Falloff(q)) => {
                        VignetteModel::Falloff(mix_n(*p, *q, t))
                    }
                    (p, _) => p.clone(),
                },
                scale: mix_option(x.scale.map(|v| [v]), y.scale.map(|v| [v]), t).map(|v| v[0]),
                center: mix_option(x.center, y.center, t),
            }),
            _ => None,
        },
        chromatic_aberration: mix_option(a.chromatic_aberration, b.chromatic_aberration, t),
    }
}

// ---------------------------------------------------------------------------
// Adobe LCP subset.

#[derive(Debug, Default)]
struct Element {
    name: String,
    attributes: Vec<(String, String)>,
    children: Vec<Element>,
    text: String,
}

fn local(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

fn unescape(text: &str) -> Result<String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let end = rest[at..].find(';').ok_or_else(|| {
            anyhow::anyhow!("[malformed-resource] LCP has an unterminated entity")
        })?;
        let entity = &rest[at + 1..at + end];
        let ch = match entity {
            "lt" => '<',
            "gt" => '>',
            "amp" => '&',
            "quot" => '"',
            "apos" => '\'',
            _ => {
                let code = if let Some(hex) = entity.strip_prefix("#x") {
                    u32::from_str_radix(hex, 16).ok()
                } else if let Some(dec) = entity.strip_prefix('#') {
                    dec.parse().ok()
                } else {
                    None
                };
                code.and_then(char::from_u32).ok_or_else(|| {
                    anyhow::anyhow!("[malformed-resource] LCP uses unknown entity &{entity};")
                })?
            }
        };
        out.push(ch);
        rest = &rest[at + end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// A bounded, non-validating XML reader: elements, attributes and text. DTDs
/// are refused, so entity expansion cannot amplify input.
fn parse_xml(text: &str) -> Result<Element> {
    let malformed =
        |what: &str| anyhow::anyhow!("[malformed-resource] LCP is not well-formed XML: {what}");
    let mut stack: Vec<Element> = vec![Element::default()];
    let mut count = 0usize;
    let mut rest = text.trim_start_matches('\u{feff}');
    while !rest.is_empty() {
        let Some(open) = rest.find('<') else {
            stack.last_mut().unwrap().text.push_str(&unescape(rest)?);
            break;
        };
        if open > 0 {
            stack
                .last_mut()
                .unwrap()
                .text
                .push_str(&unescape(&rest[..open])?);
        }
        rest = &rest[open..];
        if let Some(body) = rest.strip_prefix("<?") {
            let end = body
                .find("?>")
                .ok_or_else(|| malformed("unterminated <?"))?;
            rest = &body[end + 2..];
        } else if let Some(body) = rest.strip_prefix("<!--") {
            let end = body
                .find("-->")
                .ok_or_else(|| malformed("unterminated comment"))?;
            rest = &body[end + 3..];
        } else if let Some(body) = rest.strip_prefix("<![CDATA[") {
            let end = body
                .find("]]>")
                .ok_or_else(|| malformed("unterminated CDATA"))?;
            stack.last_mut().unwrap().text.push_str(&body[..end]);
            rest = &body[end + 3..];
        } else if rest.starts_with("<!") {
            bail!("[unsupported-capability] LCP files with a DOCTYPE are refused; remove the declaration")
        } else if let Some(body) = rest.strip_prefix("</") {
            let end = body
                .find('>')
                .ok_or_else(|| malformed("unterminated end tag"))?;
            let name = body[..end].trim();
            let element = stack.pop().ok_or_else(|| malformed("unbalanced end tag"))?;
            if element.name != name || stack.is_empty() {
                return Err(malformed(&format!(
                    "</{name}> does not close <{}>",
                    element.name
                )));
            }
            stack.last_mut().unwrap().children.push(element);
            rest = &body[end + 1..];
        } else {
            let body = &rest[1..];
            let mut end = None;
            let mut quote = None;
            for (i, c) in body.char_indices() {
                match (quote, c) {
                    (None, '"' | '\'') => quote = Some(c),
                    (Some(q), c) if c == q => quote = None,
                    (None, '>') => {
                        end = Some(i);
                        break;
                    }
                    _ => {}
                }
            }
            let end = end.ok_or_else(|| malformed("unterminated start tag"))?;
            let mut tag = &body[..end];
            let empty = tag.ends_with('/');
            if empty {
                tag = &tag[..tag.len() - 1];
            }
            let name_end = tag.find(|c: char| c.is_whitespace()).unwrap_or(tag.len());
            let name = &tag[..name_end];
            if name.is_empty() {
                return Err(malformed("empty element name"));
            }
            let mut element = Element {
                name: name.to_string(),
                ..Element::default()
            };
            let mut attrs = tag[name_end..].trim();
            while !attrs.is_empty() {
                let eq = attrs
                    .find('=')
                    .ok_or_else(|| malformed("attribute without value"))?;
                let key = attrs[..eq].trim().to_string();
                let after = attrs[eq + 1..].trim_start();
                let q = after
                    .chars()
                    .next()
                    .filter(|c| *c == '"' || *c == '\'')
                    .ok_or_else(|| malformed("unquoted attribute"))?;
                let close = after[1..]
                    .find(q)
                    .ok_or_else(|| malformed("unterminated attribute"))?;
                element
                    .attributes
                    .push((key, unescape(&after[1..1 + close])?));
                attrs = after[close + 2..].trim_start();
            }
            count += 1;
            if count > MAX_XML_ELEMENTS {
                bail!("[limit-exceeded] LCP has more than {MAX_XML_ELEMENTS} elements")
            }
            if empty {
                stack.last_mut().unwrap().children.push(element);
            } else {
                if stack.len() > MAX_XML_DEPTH {
                    bail!("[limit-exceeded] LCP nests deeper than {MAX_XML_DEPTH} elements")
                }
                stack.push(element);
            }
            rest = &body[end + 1..];
        }
    }
    if stack.len() != 1 {
        return Err(malformed("unclosed elements"));
    }
    Ok(stack.pop().unwrap())
}

enum Prop<'a> {
    Text(String),
    Node(&'a Element),
}

/// RDF properties of a resource: attributes, attributes and children of nested
/// `rdf:Description`, and child elements (text or nested resources).
fn props(element: &Element) -> Vec<(String, Prop<'_>)> {
    let mut out = Vec::new();
    for (key, value) in &element.attributes {
        if key.starts_with("xmlns") || key.starts_with("rdf:") || key.starts_with("xml:") {
            continue;
        }
        out.push((local(key).to_string(), Prop::Text(value.trim().to_string())));
    }
    for child in &element.children {
        if local(&child.name) == "Description" {
            out.extend(props(child));
        } else if child.children.is_empty()
            && !child
                .attributes
                .iter()
                .any(|(k, _)| !k.starts_with("xmlns") && !k.starts_with("rdf:"))
        {
            out.push((
                local(&child.name).to_string(),
                Prop::Text(child.text.trim().to_string()),
            ));
        } else {
            out.push((local(&child.name).to_string(), Prop::Node(child)));
        }
    }
    out
}

fn find<'a>(element: &'a Element, name: &str) -> Option<&'a Element> {
    if local(&element.name) == name {
        return Some(element);
    }
    element.children.iter().find_map(|c| find(c, name))
}

fn items(element: &Element) -> Vec<&Element> {
    let mut out = Vec::new();
    for child in &element.children {
        if local(&child.name) == "li" {
            out.push(child);
        } else {
            out.extend(items(child));
        }
    }
    out
}

/// A child element: its name, numeric values and other child names.
type Node = (String, Vec<(String, f64)>, Vec<String>);

struct Model {
    values: Vec<(String, f64)>,
    nodes: Vec<Node>,
    other: Vec<String>,
}

fn model(element: &Element) -> Result<Model> {
    let mut m = Model {
        values: Vec::new(),
        nodes: Vec::new(),
        other: Vec::new(),
    };
    for (key, prop) in props(element) {
        match prop {
            Prop::Text(text) => match text.parse::<f64>() {
                Ok(v) if v.is_finite() => m.values.push((key, v)),
                _ => m.other.push(key),
            },
            Prop::Node(node) => {
                let inner = model(node)?;
                let mut nested = inner.other;
                nested.extend(inner.nodes.into_iter().map(|(k, ..)| k));
                m.nodes.push((key, inner.values, nested));
            }
        }
    }
    Ok(m)
}

fn get(values: &[(String, f64)], key: &str) -> Option<f64> {
    values.iter().find(|(k, _)| k == key).map(|(_, v)| *v)
}

/// The result of converting an LCP file.
pub struct Converted {
    pub profile: LensProfile,
    pub unsupported: Vec<String>,
}

/// Convert the rectilinear distortion, vignette and lateral CA models of an
/// Adobe LCP file. Everything else is listed in `unsupported`, not approximated.
pub fn import_lcp(bytes: &[u8]) -> Result<Converted> {
    if bytes.len() as u64 > super::catalog::MAX_PROFILE_BYTES {
        bail!("[limit-exceeded] LCP file is larger than 16 MiB")
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| anyhow::anyhow!("[malformed-resource] LCP file is not UTF-8"))?;
    let root = parse_xml(text)?;
    let profiles = find(&root, "CameraProfiles").context(
        "[malformed-resource] LCP file has no photoshop:CameraProfiles; is it a lens correction profile?",
    )?;
    let mut unsupported: Vec<String> = Vec::new();
    let mut note = |what: String| {
        if !unsupported.contains(&what) && unsupported.len() < MAX_UNSUPPORTED {
            unsupported.push(what);
        }
    };
    let mut make = None;
    let mut lens = None;
    // (focal, aperture, focus distance, sample)
    let mut entries: Vec<(f64, f64, f64, Sample)> = Vec::new();
    for (index, item) in items(profiles).into_iter().enumerate() {
        let m = model(item)?;
        for key in &m.other {
            match key.as_str() {
                "Make" | "Model" | "Lens" | "LensPrettyName" | "CameraPrettyName"
                | "ProfileName" | "Author" | "UniqueCameraModel" | "LensInfo" | "LensID" => {}
                other => note(format!("profile {index}: {other}")),
            }
        }
        let texts = props(item);
        let value = |key: &str| {
            texts.iter().find_map(|(k, p)| match p {
                Prop::Text(t) if k == key && !t.is_empty() => Some(t.clone()),
                _ => None,
            })
        };
        if make.is_none() {
            make = value("Make");
        }
        if lens.is_none() {
            lens = value("LensPrettyName").or_else(|| value("Lens"));
        }
        let Some(focal) = get(&m.values, "FocalLength").filter(|f| *f > 0.0 && *f <= 10_000.0)
        else {
            note(format!("profile {index}: no FocalLength"));
            continue;
        };
        let aperture = match (get(&m.values, "ApertureValue"), get(&m.values, "FNumber")) {
            (_, Some(n)) if n > 0.0 && n <= 1000.0 => n,
            (Some(av), _) if (-2.0..=20.0).contains(&av) => super::math::exp2(av / 2.0),
            _ => {
                note(format!("profile {index}: no ApertureValue"));
                continue;
            }
        };
        let focus = get(&m.values, "FocusDistance").unwrap_or(f64::INFINITY);
        for (key, _) in &m.values {
            if !matches!(
                key.as_str(),
                "FocalLength"
                    | "ApertureValue"
                    | "FNumber"
                    | "FocusDistance"
                    | "SensorFormatFactor"
                    | "ImageWidth"
                    | "ImageLength"
                    | "CameraRawProfile"
            ) {
                note(key.to_string());
            }
        }
        let mut sample = Sample {
            focal,
            aperture,
            scale: None,
            center: None,
            radial: None,
            tangential: [0.0; 2],
            vignette: None,
            chromatic_aberration: None,
        };
        let mut found = false;
        for (key, values, nested) in &m.nodes {
            if key != "PerspectiveModel" {
                note(key.clone());
                continue;
            }
            found = true;
            let fx = get(values, "FocalLengthX").filter(|v| *v > 0.0 && *v <= 10.0);
            let fy = get(values, "FocalLengthY");
            let cx = get(values, "ImageXCenter").unwrap_or(0.5);
            let cy = get(values, "ImageYCenter").unwrap_or(0.5);
            let Some(fx) = fx else {
                note("PerspectiveModel without FocalLengthX".into());
                continue;
            };
            if fy.is_some_and(|fy| (fy - fx).abs() > 1e-9) {
                note("PerspectiveModel FocalLengthY (anisotropic focal length)".into());
            }
            sample.scale = Some(fx);
            sample.center = Some([cx.clamp(0.0, 1.0), cy.clamp(0.0, 1.0)]);
            let k = |n: u8| get(values, &format!("RadialDistortParam{n}")).unwrap_or(0.0);
            sample.radial = Some([k(1), k(2), k(3)].map(|v| v.clamp(-10.0, 10.0)));
            let t = |n: u8| get(values, &format!("TangentialDistortParam{n}")).unwrap_or(0.0);
            sample.tangential = [t(1), t(2)].map(|v| v.clamp(-1.0, 1.0));
            for (key, _) in values {
                if !matches!(
                    key.as_str(),
                    "Version"
                        | "FocalLengthX"
                        | "FocalLengthY"
                        | "ImageXCenter"
                        | "ImageYCenter"
                        | "ScaleFactor"
                        | "RadialDistortParam1"
                        | "RadialDistortParam2"
                        | "RadialDistortParam3"
                        | "TangentialDistortParam1"
                        | "TangentialDistortParam2"
                ) {
                    note(format!("PerspectiveModel {key}"));
                }
            }
            if get(values, "ScaleFactor").is_some_and(|s| s != 1.0) {
                note("PerspectiveModel ScaleFactor".into());
            }
            for name in nested {
                note(format!("PerspectiveModel {name}"));
            }
            // Submodels are nested resources of the perspective model.
            let node = item_node(item, "PerspectiveModel");
            if let Some(node) = node {
                for (sub, prop) in props(node) {
                    let Prop::Node(sub_node) = prop else { continue };
                    let sm = model(sub_node)?;
                    match sub.as_str() {
                        "VignetteModel" => {
                            let a = |n: u8| {
                                get(&sm.values, &format!("VignetteModelParam{n}")).unwrap_or(0.0)
                            };
                            sample.vignette = Some(Vignette {
                                model: VignetteModel::Falloff(
                                    [a(1), a(2), a(3)].map(|v| v.clamp(-100.0, 100.0)),
                                ),
                                scale: get(&sm.values, "FocalLengthX")
                                    .filter(|v| *v > 0.0 && *v <= 10.0)
                                    .or(Some(fx)),
                                center: Some([
                                    get(&sm.values, "ImageXCenter")
                                        .unwrap_or(cx)
                                        .clamp(0.0, 1.0),
                                    get(&sm.values, "ImageYCenter")
                                        .unwrap_or(cy)
                                        .clamp(0.0, 1.0),
                                ]),
                            });
                            for (key, _) in &sm.values {
                                if !matches!(
                                    key.as_str(),
                                    "FocalLengthX"
                                        | "FocalLengthY"
                                        | "ImageXCenter"
                                        | "ImageYCenter"
                                        | "VignetteModelParam1"
                                        | "VignetteModelParam2"
                                        | "VignetteModelParam3"
                                ) {
                                    note(format!("VignetteModel {key}"));
                                }
                            }
                        }
                        "ChromaticRedGreenModel" | "ChromaticBlueGreenModel" => {
                            let scale = get(&sm.values, "ScaleFactor")
                                .unwrap_or(1.0)
                                .clamp(0.9, 1.1);
                            let ca = sample.chromatic_aberration.get_or_insert([1.0, 1.0]);
                            if sub == "ChromaticRedGreenModel" {
                                ca[0] = scale;
                            } else {
                                ca[1] = scale;
                            }
                            for (key, v) in &sm.values {
                                let radial = key.starts_with("RadialDistortParam")
                                    || key.starts_with("TangentialDistortParam");
                                let known = matches!(
                                    key.as_str(),
                                    "ScaleFactor"
                                        | "FocalLengthX"
                                        | "FocalLengthY"
                                        | "ImageXCenter"
                                        | "ImageYCenter"
                                );
                                if (radial && *v != 0.0) || (!radial && !known) {
                                    note(format!("{sub} {key}"));
                                }
                            }
                        }
                        other => note(other.to_string()),
                    }
                }
            }
        }
        if !found {
            note(format!("profile {index}: no PerspectiveModel"));
            continue;
        }
        if let Some(existing) = entries
            .iter_mut()
            .find(|(f, a, ..)| *f == focal && *a == aperture)
        {
            note("FocusDistance variation (the farthest focus distance is kept)".into());
            if focus > existing.2 {
                *existing = (focal, aperture, focus, sample);
            }
            continue;
        }
        entries.push((focal, aperture, focus, sample));
    }
    if entries.is_empty() {
        bail!("[unsupported-capability] the LCP file has no rectilinear perspective model this build converts; unsupported: {}", unsupported.join(", "))
    }
    if entries.len() > MAX_SAMPLES {
        bail!("[limit-exceeded] the LCP file has more than {MAX_SAMPLES} profiles")
    }
    // Every sample must have the same shape; fill absent parts with identity.
    let any_vignette = entries.iter().any(|e| e.3.vignette.is_some());
    let any_ca = entries.iter().any(|e| e.3.chromatic_aberration.is_some());
    for (.., sample) in &mut entries {
        if any_vignette && sample.vignette.is_none() {
            sample.vignette = Some(Vignette {
                model: VignetteModel::Falloff([0.0; 3]),
                scale: sample.scale,
                center: sample.center,
            });
        }
        if any_ca && sample.chromatic_aberration.is_none() {
            sample.chromatic_aberration = Some([1.0, 1.0]);
        }
    }
    entries.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    let samples: Vec<Sample> = entries.into_iter().map(|e| e.3).collect();
    let fold = |f: fn(&Sample) -> f64| {
        let values: Vec<f64> = samples.iter().map(f).collect();
        [
            values.iter().copied().fold(f64::INFINITY, f64::min),
            values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        ]
    };
    let cut = |s: Option<String>, fallback: &str| {
        let s = s.unwrap_or_else(|| fallback.to_string());
        s.chars().take(128).collect::<String>()
    };
    let profile = LensProfile {
        make: cut(make, "Unknown"),
        model: cut(lens, "Unknown lens"),
        focal_range: fold(|s| s.focal),
        aperture_range: fold(|s| s.aperture),
        samples,
    };
    // Round-trip through the canonical form so stored bytes always re-parse.
    let profile = LensProfile::parse(&profile.to_bytes())?;
    Ok(Converted {
        profile,
        unsupported,
    })
}

fn item_node<'a>(item: &'a Element, name: &str) -> Option<&'a Element> {
    props(item).into_iter().find_map(|(k, p)| match p {
        Prop::Node(node) if k == name => Some(node),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> Vec<u8> {
        serde_json::to_vec(&json!({
            "pentool_lens_profile": 1,
            "make": "Test",
            "model": "Zoom 24-70",
            "focal_range": [24, 70],
            "aperture_range": [2.8, 22],
            "samples": [
                {"focal": 24, "aperture": 2.8, "distortion": {"radial": [-0.2, 0, 0]}, "chromatic_aberration": {"red": 1.002, "blue": 0.998}},
                {"focal": 24, "aperture": 8, "distortion": {"radial": [-0.1, 0, 0]}, "chromatic_aberration": {"red": 1.001, "blue": 0.999}},
                {"focal": 70, "aperture": 2.8, "distortion": {"radial": [0.1, 0, 0]}, "chromatic_aberration": {"red": 1.0, "blue": 1.0}}
            ]
        }))
        .unwrap()
    }

    #[test]
    fn samples_interpolate_in_focal_then_inverse_aperture() {
        let p = LensProfile::parse(&profile()).unwrap();
        assert_eq!(p.at(24.0, 2.8).radial, Some([-0.2, 0.0, 0.0]));
        // Halfway in 1/aperture between f/2.8 and f/8.
        let mid = 2.0 / (1.0 / 2.8 + 1.0 / 8.0);
        let k = p.at(24.0, mid).radial.unwrap()[0];
        assert!((k + 0.15).abs() < 1e-12, "{k}");
        // Clamped below the smallest focal length and beyond the apertures.
        assert_eq!(p.at(10.0, 64.0).radial, Some([-0.1, 0.0, 0.0]));
        // Halfway in focal at f/2.8.
        let k = p.at(47.0, 2.8).radial.unwrap()[0];
        assert!((k + 0.05).abs() < 1e-12, "{k}");
        // Canonical bytes re-parse to the same profile.
        assert_eq!(LensProfile::parse(&p.to_bytes()).unwrap(), p);
    }

    #[test]
    fn malformed_profiles_are_refused() {
        let mut bad = serde_json::from_slice::<Value>(&profile()).unwrap();
        bad["samples"][1]["vignette"] = json!({"gain": [0.1]});
        assert!(LensProfile::parse(&serde_json::to_vec(&bad).unwrap())
            .unwrap_err()
            .to_string()
            .contains("same fields"));
        for broken in [
            json!({"pentool_lens_profile": 2}),
            json!({"pentool_lens_profile": 1, "make": "a", "model": "b", "focal_range": [70, 24], "aperture_range": [1, 2], "samples": [{"focal": 30, "aperture": 1}]}),
            json!({"pentool_lens_profile": 1, "make": "a", "model": "b", "focal_range": [24, 70], "aperture_range": [1, 2], "samples": [{"focal": 80, "aperture": 1}]}),
            json!({"pentool_lens_profile": 1, "make": "a", "model": "b", "focal_range": [24, 70], "aperture_range": [1, 2], "samples": [{"focal": 30, "aperture": 1, "distortion": {"radial": [99]}}]}),
        ] {
            assert!(
                LensProfile::parse(&serde_json::to_vec(&broken).unwrap()).is_err(),
                "{broken}"
            );
        }
    }

    #[test]
    fn lcp_subset_converts_and_lists_the_rest() {
        let lcp = r#"<?xml version="1.0" encoding="UTF-8"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:photoshop="http://ns.adobe.com/photoshop/1.0/" xmlns:stCamera="http://ns.adobe.com/photoshop/1.0/camera-profile">
   <photoshop:CameraProfiles>
    <rdf:Seq>
     <rdf:li rdf:parseType="Resource">
      <stCamera:Make>Pentool</stCamera:Make>
      <stCamera:Lens>Test 35mm</stCamera:Lens>
      <stCamera:FocalLength>35</stCamera:FocalLength>
      <stCamera:ApertureValue>2</stCamera:ApertureValue>
      <stCamera:PerspectiveModel rdf:parseType="Resource">
       <stCamera:Version>2</stCamera:Version>
       <stCamera:FocalLengthX>0.9</stCamera:FocalLengthX>
       <stCamera:FocalLengthY>0.9</stCamera:FocalLengthY>
       <stCamera:ImageXCenter>0.5</stCamera:ImageXCenter>
       <stCamera:ImageYCenter>0.3333</stCamera:ImageYCenter>
       <stCamera:RadialDistortParam1>-0.05</stCamera:RadialDistortParam1>
       <stCamera:VignetteModel rdf:parseType="Resource">
        <stCamera:FocalLengthX>0.9</stCamera:FocalLengthX>
        <stCamera:VignetteModelParam1>-0.3</stCamera:VignetteModelParam1>
       </stCamera:VignetteModel>
       <stCamera:ChromaticRedGreenModel>
        <rdf:Description stCamera:ScaleFactor="1.0004" stCamera:RadialDistortParam1="0.001"/>
       </stCamera:ChromaticRedGreenModel>
      </stCamera:PerspectiveModel>
      <stCamera:FisheyeModel rdf:parseType="Resource"><stCamera:FisheyeModelParam1>1</stCamera:FisheyeModelParam1></stCamera:FisheyeModel>
     </rdf:li>
    </rdf:Seq>
   </photoshop:CameraProfiles>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;
        let c = import_lcp(lcp.as_bytes()).unwrap();
        let s = &c.profile.samples[0];
        assert_eq!(c.profile.make, "Pentool");
        assert_eq!(c.profile.model, "Test 35mm");
        assert!((s.aperture - 2.0).abs() < 1e-12);
        assert_eq!(s.radial, Some([-0.05, 0.0, 0.0]));
        assert_eq!(s.scale, Some(0.9));
        assert_eq!(s.center, Some([0.5, 0.3333]));
        assert_eq!(s.chromatic_aberration, Some([1.0004, 1.0]));
        assert!(
            matches!(s.vignette.as_ref().unwrap().model, VignetteModel::Falloff([a, 0.0, 0.0]) if a == -0.3)
        );
        assert!(
            c.unsupported.iter().any(|u| u.contains("FisheyeModel")),
            "{:?}",
            c.unsupported
        );
        assert!(
            c.unsupported
                .iter()
                .any(|u| u.contains("ChromaticRedGreenModel RadialDistortParam1")),
            "{:?}",
            c.unsupported
        );
        assert!(import_lcp(b"<!DOCTYPE x [<!ENTITY a 'b'>]><x/>").is_err());
        assert!(import_lcp(b"<a><b></a>").is_err());
    }
}
