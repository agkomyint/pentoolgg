//! Plate separations with explicit knockout and overprint semantics.
//! See "Proofing, inks and plate behavior" in `docs/professional-color-v1.md`.
//!
//! A separation is a second evaluation of the same scene: every painted slot of
//! every visible node contributes coverage (the shared vector rasterizer) to
//! deterministic ink plates. Plates are `C`, `M`, `Y`, `K`, then the used spot
//! ink IDs in lexical order. Only constructs with specified plate semantics are
//! accepted; everything else fails with the node and corrective action.
//!
//! Declarations live in `style`, so the same document still previews through
//! the ordinary renderer using each paint's native or `fallback` color:
//!
//! ```json
//! "style": {
//!   "fill": {"spot": {"ink": "dieline", "tint": 0.5}, "fallback": {"space": "srgb", "channels": [1, 0, 0]}},
//!   "overprint": {"fill": true, "stroke": false, "mode": 1}
//! }
//! ```
//!
//! A manual trap spreads an object's own ink outward (square element, at most
//! 32 device pixels) and raises the plates beneath only where the object's
//! value is higher; it is refused on overprint paints:
//!
//! ```json
//! "style": {"fill": {...}, "trap": {"width": 0.5}}
//! ```
use super::{
    icc::Intent,
    roles::{self, Role},
    spot,
    value::{Destination, Evaluator, NativeColor, ProfileSet},
};
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::Path;

pub const PROCESS: [&str; 4] = ["C", "M", "Y", "K"];
pub const MAX_OBJECTS: usize = 4096;
pub const MAX_PLATES: usize = PROCESS.len() + 32;

#[derive(Debug, Clone)]
pub struct Options {
    /// CMYK destination for process paints; the document `output` role when absent.
    pub destination: Option<Destination>,
    pub intent: Option<Intent>,
    pub scale: f32,
    /// Automatic trap width in document units (see [`auto_trap`]); `None` keeps
    /// only manual traps.
    pub auto_trap: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Plate {
    pub id: String,
    pub spot: bool,
    /// Ink amount in [0,1], row-major, one value per pixel.
    pub samples: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Separations {
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    pub destination: Option<Destination>,
    pub intent: Option<Intent>,
    pub plates: Vec<Plate>,
    pub objects: usize,
    pub knockouts: usize,
    pub overprints: usize,
    pub clipped_paints: usize,
    /// Objects that declared a manual trap.
    pub traps: usize,
    /// Pixels the automatic trap changed (0 when it was not requested).
    pub auto_trapped_pixels: usize,
    /// Production marks painted onto every process plate.
    pub marks: usize,
    /// Color-bar patches knocked out onto the plates.
    pub color_bar_patches: usize,
    /// 1 when a slug text line was printed onto the process plates.
    pub slug_text: usize,
    pub inks: Vec<Value>,
}

#[derive(Debug, Clone, Copy)]
struct Overprint {
    fill: bool,
    stroke: bool,
    mode: u8,
}

enum Spec {
    Native(NativeColor),
    /// A linear, radial or conic gradient with opaque native stops. `ramp` is
    /// the same fill with a black-to-white stop pair: rendering it yields the
    /// gradient position per pixel (8-bit), and the stops' device values are
    /// interpolated along that position on every process plate.
    Gradient {
        stops: Vec<(f64, NativeColor)>,
        ramp: Value,
    },
    /// An embedded raster image: its sRGB8 pixels are converted to the CMYK
    /// destination through the verified profile (device values are not
    /// preserved; photographs are RGB sources).
    Image,
    Spot {
        ink: String,
        tint: f64,
    },
}

struct Painted<'a> {
    node: &'a Value,
    parent: kurbo::Affine,
    slot: &'static str,
    spec: Spec,
    overprint: Option<u8>,
    /// Manual trap spread distance in document units.
    trap: Option<f64>,
}

/// Largest manual trap spread in device pixels.
pub const MAX_TRAP_PIXELS: usize = 32;

fn name(node: &Value) -> String {
    node["id"].as_str().unwrap_or("<unnamed>").to_owned()
}

/// Parse `style.trap`: `{"width": units}`, a spread of the object's own ink
/// outward by `width` (square structuring element), applied only where it
/// raises the underlying plate value. Valid only on opaque knockout paints.
fn trap(node: &Value) -> Result<Option<f64>> {
    let Some(value) = node["style"].get("trap") else {
        return Ok(None);
    };
    let fields = value
        .as_object()
        .with_context(|| format!("[invalid-input] node {} trap must be an object", name(node)))?;
    if let Some(key) = fields.keys().find(|key| key.as_str() != "width") {
        bail!(
            "[invalid-input] node {} trap only accepts width, not {key}",
            name(node)
        );
    }
    let width = fields
        .get("width")
        .and_then(Value::as_f64)
        .filter(|w| w.is_finite() && *w > 0.0 && *w <= 1024.0)
        .with_context(|| {
            format!(
                "[invalid-input] node {} trap width must be a number in (0, 1024] units",
                name(node)
            )
        })?;
    ensure!(
        node["style"].get("overprint").is_none(),
        "[invalid-input] node {} sets both trap and overprint; a trap only applies to knockout paints",
        name(node)
    );
    Ok(Some(width))
}

fn overprint(node: &Value) -> Result<Overprint> {
    let mut out = Overprint {
        fill: false,
        stroke: false,
        mode: 0,
    };
    let Some(value) = node["style"].get("overprint") else {
        return Ok(out);
    };
    let fields = value.as_object().with_context(|| {
        format!(
            "[invalid-input] node {} overprint must be an object",
            name(node)
        )
    })?;
    for (key, value) in fields {
        match key.as_str() {
            "fill" | "stroke" => {
                let flag = value.as_bool().with_context(|| {
                    format!(
                        "[invalid-input] node {} overprint {key} must be a boolean",
                        name(node)
                    )
                })?;
                if key == "fill" {
                    out.fill = flag
                } else {
                    out.stroke = flag
                }
            }
            "mode" => {
                out.mode = match value.as_u64() {
                    Some(0) => 0,
                    Some(1) => 1,
                    _ => bail!(
                        "[invalid-input] node {} overprint mode must be 0 or 1",
                        name(node)
                    ),
                }
            }
            _ => bail!(
                "[invalid-input] node {} overprint only accepts fill, stroke and mode",
                name(node)
            ),
        }
    }
    Ok(out)
}

fn spec(node: &Value, slot: &str, styles: &Value, library: &spot::Library) -> Result<Option<Spec>> {
    let value = &node["style"][slot];
    let Some(fields) = value.as_object() else {
        return match value {
            Value::Null => Ok(None),
            _ => native(node, slot, value, styles),
        };
    };
    if let Some(key) = fields.keys().find(|key| {
        !matches!(
            key.as_str(),
            "ref" | "fallback" | "spot" | "space" | "channels" | "profile" | "alpha"
        )
    }) {
        bail!(
            "[unsupported-capability] node {} {slot} uses {key}; plate separation supports flat native and spot paints, not gradients, patterns or images",
            name(node)
        );
    }
    let Some(declared) = fields.get("spot") else {
        return native(node, slot, value, styles);
    };
    let ink = declared["ink"].as_str().with_context(|| {
        format!(
            "[invalid-input] node {} {slot} spot needs an ink ID",
            name(node)
        )
    })?;
    let tint = declared["tint"]
        .as_f64()
        .filter(|t| t.is_finite() && (0.0..=1.0).contains(t))
        .with_context(|| {
            format!(
                "[invalid-input] node {} {slot} spot tint must be in [0,1]",
                name(node)
            )
        })?;
    ensure!(
        declared.as_object().is_some_and(|o| o.len() == 2),
        "[invalid-input] node {} {slot} spot only accepts ink and tint",
        name(node)
    );
    ensure!(
        fields.contains_key("fallback"),
        "[invalid-input] node {} {slot} spot paint must declare the preview fallback color",
        name(node)
    );
    let alternate = super::gradient::color(value, styles, 0)?;
    ensure!(
        alternate.alpha() == 1.0,
        "[unsupported-capability] node {} {slot} spot paint has partial alpha; spot transparency has no specified plate semantics",
        name(node)
    );
    let id = library
        .resolve(ink)
        .with_context(|| {
            format!(
                "[missing-resource] node {} {slot} references unknown ink {ink}; define it in color_management.inks",
                name(node)
            )
        })?
        .id
        .clone();
    Ok(Some(Spec::Spot { ink: id, tint }))
}

/// Spec for a `fill` node's gradient: opaque native stops interpolated in device
/// values, with a black-to-white ramp that carries the gradient position.
fn gradient_spec(node: &Value, styles: &Value) -> Result<Spec> {
    let value = &node["fill"];
    let kind = value["kind"].as_str().unwrap_or_default();
    ensure!(
        matches!(kind, "linear" | "radial" | "conic"),
        "[unsupported-capability] node {} fill kind {kind} has no plate separation; use a linear, radial or conic gradient",
        name(node)
    );
    ensure!(
        value.get("space").is_none(),
        "[unsupported-capability] node {} gradient declares native interpolation space; plate separation interpolates device values between stops",
        name(node)
    );
    let stops = super::gradient::sources(value, styles)?;
    ensure!(
        stops.iter().all(|(_, color)| color.alpha() == 1.0),
        "[unsupported-capability] node {} gradient has a stop with partial alpha; separation needs opaque paints or flatten transparency first",
        name(node)
    );
    let mut ramp = value.clone();
    ramp["stops"] = json!([
        {"position":0,"color":"#000000ff"},
        {"position":1,"color":"#ffffffff"}
    ]);
    Ok(Spec::Gradient { stops, ramp })
}

fn native(node: &Value, slot: &str, value: &Value, styles: &Value) -> Result<Option<Spec>> {
    let color = super::gradient::color(value, styles, 0)?;
    if color.alpha() == 0.0 {
        return Ok(None);
    }
    ensure!(
        color.alpha() == 1.0,
        "[unsupported-capability] node {} {slot} has partial alpha {}; separation needs opaque paints or flatten transparency first",
        name(node),
        color.alpha()
    );
    Ok(Some(Spec::Native(color)))
}

fn unsupported(node: &Value) -> Result<()> {
    for key in ["effects", "transforms", "mask", "clip", "clipping"] {
        ensure!(
            node.get(key).is_none(),
            "[unsupported-capability] node {} uses {key}; plate separation has no specified semantics for it",
            name(node)
        );
    }
    ensure!(
        node.get("opacity").is_none_or(|v| v.as_f64() == Some(1.0)),
        "[unsupported-capability] node {} has opacity below 1; partial-alpha interactions have no specified plate semantics",
        name(node)
    );
    ensure!(
        node.get("blend_mode").is_none_or(|v| v == "normal"),
        "[unsupported-capability] node {} uses blend mode {}; only normal blending separates",
        name(node),
        node["blend_mode"]
    );
    Ok(())
}

fn walk<'a>(
    nodes: &'a [Value],
    parent: kurbo::Affine,
    styles: &Value,
    library: &spot::Library,
    isolated: bool,
    out: &mut Vec<Painted<'a>>,
) -> Result<()> {
    for node in nodes {
        if node.get("visible").and_then(Value::as_bool) == Some(false) {
            continue;
        }
        unsupported(node)?;
        match node["kind"].as_str().unwrap_or_default() {
            "group" => {
                let isolation = node.get("isolation") == Some(&json!("isolated"));
                walk(
                    node["children"].as_array().map_or(&[][..], Vec::as_slice),
                    parent * crate::composite::transform(node)?,
                    styles,
                    library,
                    isolated || isolation,
                    out,
                )?;
            }
            "image" | "photo" => {
                ensure!(
                    out.len() < MAX_OBJECTS,
                    "[limit-exceeded] separation supports at most {MAX_OBJECTS} painted slots"
                );
                out.push(Painted {
                    node,
                    parent,
                    slot: "fill",
                    spec: Spec::Image,
                    overprint: None,
                    trap: None,
                });
            }
            "fill" => {
                ensure!(
                    out.len() < MAX_OBJECTS,
                    "[limit-exceeded] separation supports at most {MAX_OBJECTS} painted slots"
                );
                out.push(Painted {
                    node,
                    parent,
                    slot: "fill",
                    spec: gradient_spec(node, styles)?,
                    overprint: None,
                    trap: None,
                });
            }
            "rect" | "ellipse" | "path" | "line" | "text" => {
                let flags = overprint(node)?;
                let trap = trap(node)?;
                for slot in ["fill", "stroke"] {
                    if slot == "stroke" && node["kind"] == "text" {
                        continue;
                    }
                    let Some(spec) = spec(node, slot, styles, library)? else {
                        continue;
                    };
                    let on = if slot == "fill" {
                        flags.fill
                    } else {
                        flags.stroke
                    };
                    ensure!(
                        !(on && isolated),
                        "[unsupported-capability] node {} overprints inside an isolated transparency group; overprint there has no specified plate semantics",
                        name(node)
                    );
                    ensure!(
                        out.len() < MAX_OBJECTS,
                        "[limit-exceeded] separation supports at most {MAX_OBJECTS} painted slots"
                    );
                    out.push(Painted {
                        node,
                        parent,
                        slot: if slot == "fill" { "fill" } else { "stroke" },
                        spec,
                        overprint: on.then_some(flags.mode),
                        trap,
                    });
                }
            }
            kind => bail!(
                "[unsupported-capability] node {} has kind {kind}; plate separation supports vector, text and plain group nodes",
                name(node)
            ),
        }
    }
    Ok(())
}

/// Validate every separation declaration in the document without rendering.
pub fn validate(raw: &Value) -> Result<()> {
    let library = spot::document_library(raw)?;
    let mut sink = Vec::new();
    for page in raw["pages"].as_array().into_iter().flatten() {
        for layer in page["layers"].as_array().into_iter().flatten() {
            if layer.get("visible").and_then(Value::as_bool) == Some(false) {
                continue;
            }
            // Only declarations are checked here; unsupported-construct errors are
            // reserved for the explicit separation command.
            declarations(
                layer["nodes"].as_array().map_or(&[][..], Vec::as_slice),
                &raw["styles"],
                &library,
                &mut sink,
            )?;
        }
    }
    Ok(())
}

fn declarations(
    nodes: &[Value],
    styles: &Value,
    library: &spot::Library,
    sink: &mut Vec<()>,
) -> Result<()> {
    for node in nodes {
        if node["style"].get("overprint").is_some() {
            overprint(node)?;
        }
        trap(node)?;
        for slot in ["fill", "stroke"] {
            if node["style"][slot].get("spot").is_some() {
                spec(node, slot, styles, library)?;
                sink.clear();
            }
        }
        if let Some(children) = node.get("children").and_then(Value::as_array) {
            declarations(children, styles, library, sink)?;
        }
    }
    Ok(())
}

fn cmyk_destination(raw: &Value, options: &Options) -> Result<(Destination, Intent)> {
    let role = match (&options.destination, options.intent) {
        (Some(_), Some(_)) => None,
        _ => Some(roles::effective(raw, Role::Output)?),
    };
    let destination = match (&options.destination, &role) {
        (Some(destination), _) => destination.clone(),
        (None, Some(role)) => role.destination.clone(),
        _ => unreachable!(),
    };
    let intent = match (options.intent, &role) {
        (Some(intent), _) => intent,
        (None, Some(role)) => role.intent,
        _ => unreachable!(),
    };
    ensure!(
        destination.space() == "icc-cmyk",
        "[unsupported-capability] process separations require an ICC CMYK destination; assign a CMYK output role or pass one explicitly"
    );
    Ok((destination, intent))
}

pub fn separate(
    raw: &Value,
    document: &Path,
    page_id: Option<&str>,
    options: &Options,
) -> Result<Separations> {
    ensure!(
        (0.1..=8.0).contains(&options.scale),
        "[invalid-input] separation scale must be in 0.1..8"
    );
    ensure!(
        raw["version"] == super::VERSION,
        "[unsupported-version] plate separation requires a version {} document",
        super::VERSION
    );
    crate::scene::validate(raw)?;
    let resolved = crate::scene::with_resolved_tokens(raw)?;
    let raw = &resolved;
    let library = spot::document_library(raw)?;
    let pages = raw["pages"].as_array().context("pages missing")?;
    let page = match page_id {
        Some(id) => pages
            .iter()
            .find(|page| page["id"] == id)
            .with_context(|| format!("[missing-resource] page not found: {id}"))?,
        None => pages.first().context("document has no pages")?,
    };
    let mut items = Vec::new();
    for layer in page["layers"].as_array().context("layers missing")? {
        if layer.get("visible").and_then(Value::as_bool) == Some(false) {
            continue;
        }
        walk(
            layer["nodes"].as_array().map_or(&[][..], Vec::as_slice),
            kurbo::Affine::IDENTITY,
            &raw["styles"],
            &library,
            false,
            &mut items,
        )?;
    }
    let canvas = |key: &str| -> Result<u32> {
        Ok((page["canvas"][key]
            .as_f64()
            .with_context(|| format!("canvas {key} missing"))?
            * f64::from(options.scale))
        .round()
        .max(1.0) as u32)
    };
    let (width, height) = (canvas("width")?, canvas("height")?);
    let spots: BTreeSet<&str> = items
        .iter()
        .filter_map(|item| match &item.spec {
            Spec::Spot { ink, .. } => Some(ink.as_str()),
            Spec::Native(_) | Spec::Gradient { .. } | Spec::Image => None,
        })
        .collect();
    let count = PROCESS.len() + spots.len();
    ensure!(
        count <= MAX_PLATES,
        "[limit-exceeded] separation supports at most {MAX_PLATES} plates"
    );
    ensure!(
        u64::from(width) * u64::from(height) * count as u64 * 8
            <= crate::blend::LinearSurface::MAX_BYTES as u64,
        "[limit-exceeded] plate memory exceeds the working budget; reduce canvas size or scale"
    );
    let needs_process = items.iter().any(|item| {
        matches!(
            item.spec,
            Spec::Native(_) | Spec::Gradient { .. } | Spec::Image
        )
    });
    let process = if needs_process {
        Some(cmyk_destination(raw, options)?)
    } else {
        None
    };
    let set = match &process {
        Some((destination, _)) => ProfileSet::load(
            raw,
            document,
            destination
                .profile()
                .into_iter()
                .chain(items.iter().flat_map(|item| {
                    match &item.spec {
                        Spec::Native(color) => color.destination().profile().into_iter().collect(),
                        Spec::Gradient { stops, .. } => stops
                            .iter()
                            .filter_map(|(_, color)| color.destination().profile())
                            .collect::<Vec<_>>(),
                        Spec::Spot { .. } | Spec::Image => Vec::new(),
                    }
                })),
        )?,
        None => ProfileSet::default(),
    };
    let mut engine = match &process {
        Some((_, intent)) => Some(Evaluator::new(&set, *intent)?),
        None => None,
    };
    let mut plates: Vec<Plate> = PROCESS
        .iter()
        .map(|id| ((*id).to_owned(), false))
        .chain(spots.iter().map(|id| ((*id).to_owned(), true)))
        .map(|(id, spot)| Plate {
            id,
            spot,
            samples: vec![0.0; width as usize * height as usize],
        })
        .collect();
    let (mut knockouts, mut overprints, mut clipped_paints, mut traps) = (0, 0, 0, 0);
    let mut image_cache: std::collections::HashMap<[u8; 3], [f64; 4]> =
        std::collections::HashMap::new();
    for item in &items {
        crate::composite::check_cancelled()?;
        // Per-plate value this paint deposits, and whether each plate is touched.
        let mut value = vec![0.0; count];
        let mut ramp: Option<Vec<(f64, [f64; 4])>> = None;
        match &item.spec {
            Spec::Image => {}
            Spec::Gradient { stops, .. } => {
                ensure!(
                    item.trap.is_none(),
                    "[unsupported-capability] node {} combines a gradient with a manual trap; trapping a varying paint has no specified plate semantics",
                    name(item.node)
                );
                let (destination, _) = process.as_ref().unwrap();
                let mut converted = Vec::with_capacity(stops.len());
                for (position, color) in stops {
                    let evaluation = engine
                        .as_mut()
                        .unwrap()
                        .convert(color, destination)
                        .with_context(|| {
                            format!(
                                "[invalid-input] node {} {} cannot be separated",
                                name(item.node),
                                item.slot
                            )
                        })?;
                    clipped_paints += usize::from(evaluation.clipped);
                    let channels = evaluation.color.channels();
                    converted.push((
                        *position,
                        [channels[0], channels[1], channels[2], channels[3]],
                    ));
                }
                ramp = Some(converted);
            }
            Spec::Native(color) => {
                let (destination, _) = process.as_ref().unwrap();
                let evaluation = engine
                    .as_mut()
                    .unwrap()
                    .convert(color, destination)
                    .with_context(|| {
                        format!(
                            "[invalid-input] node {} {} cannot be separated",
                            name(item.node),
                            item.slot
                        )
                    })?;
                clipped_paints += usize::from(evaluation.clipped);
                value[..4].copy_from_slice(evaluation.color.channels());
            }
            Spec::Spot { ink, tint } => {
                let index = plates.iter().position(|p| p.spot && &p.id == ink).unwrap();
                value[index] = *tint;
            }
        }
        // Knockout clears every plate under the paint; overprint retains plates
        // the paint does not eligibly replace. A spot overprint touches only its
        // own plate, and process mode 1 skips zero channels.
        let touched: Vec<bool> = match (&item.spec, item.overprint) {
            (_, None) => vec![true; count],
            (Spec::Spot { ink, .. }, Some(_)) => plates
                .iter()
                .map(|plate| plate.spot && &plate.id == ink)
                .collect(),
            (Spec::Native(_), Some(mode)) => (0..count)
                .map(|i| i < 4 && (mode == 0 || value[i] > 0.0))
                .collect(),
            (Spec::Gradient { .. } | Spec::Image, Some(_)) => (0..count).map(|i| i < 4).collect(),
        };
        if item.overprint.is_some() {
            overprints += 1
        } else {
            knockouts += 1
        }
        let mut content = item.node.clone();
        let coverage = if let Spec::Gradient { ramp, .. } = &item.spec {
            content["fill"] = ramp.clone();
            crate::fill::render(
                raw,
                &content,
                document,
                item.parent,
                (width, height),
                options.scale,
                None,
            )?
        } else if matches!(item.spec, Spec::Image) {
            let fragment = crate::image::fragment_svg(raw, page, document, &content, item.parent)?;
            crate::render::scene_to_rgba_proxy(&fragment, options.scale)?
        } else {
            content["style"]["fill"] =
                json!({"fallback":if item.slot == "fill" { "#ffffffff" } else { "none" }});
            content["style"]["stroke"] =
                json!({"fallback":if item.slot == "stroke" { "#ffffffff" } else { "none" }});
            let fragment = crate::image::fragment_svg(raw, page, document, &content, item.parent)?;
            crate::render::scene_to_rgba_proxy(&fragment, options.scale)?
        };
        ensure!(
            coverage.dimensions() == (width, height),
            "[invalid-composite] node {} rasterized at an unexpected size",
            name(item.node)
        );
        // Manual trap: spread the paint's own coverage outward and let each
        // touched plate take the larger of its value and the paint's value in
        // that fringe. Painted later objects still knock the fringe out.
        let mut fringe: Vec<f64> = Vec::new();
        if let Some(units) = item.trap {
            let radius = (units * f64::from(options.scale)).ceil() as usize;
            ensure!(
                radius <= MAX_TRAP_PIXELS,
                "[limit-exceeded] node {} trap of {units} units is {radius} px at this scale; the limit is {MAX_TRAP_PIXELS} px",
                name(item.node)
            );
            let alpha: Vec<f64> = coverage
                .pixels()
                .map(|pixel| f64::from(pixel[3]) / 255.0)
                .collect();
            fringe = dilate(&alpha, width as usize, height as usize, radius);
            for (f, a) in fringe.iter_mut().zip(&alpha) {
                *f = (*f - a).max(0.0);
            }
            traps += 1;
        }
        for (index, f) in fringe.iter().enumerate() {
            if *f <= 0.0 {
                continue;
            }
            for (plate, value) in plates.iter_mut().zip(&value) {
                let old = plate.samples[index];
                if *value > old {
                    plate.samples[index] = old + (*value - old) * *f;
                }
            }
        }
        for (index, pixel) in coverage.pixels().enumerate() {
            if pixel[3] == 0 {
                continue;
            }
            let c = f64::from(pixel[3]) / 255.0;
            let mut sampled = value.clone();
            if matches!(item.spec, Spec::Image) {
                let key = [pixel[0], pixel[1], pixel[2]];
                let cmyk = match image_cache.get(&key) {
                    Some(found) => *found,
                    None => {
                        let (destination, _) = process.as_ref().unwrap();
                        let color = NativeColor::parse(&json!({"space":"srgb","channels":[
                            f64::from(key[0]) / 255.0,
                            f64::from(key[1]) / 255.0,
                            f64::from(key[2]) / 255.0]}))?;
                        let evaluation = engine
                            .as_mut()
                            .unwrap()
                            .convert(&color, destination)
                            .with_context(|| {
                                format!(
                                    "[invalid-input] image {} cannot be separated",
                                    name(item.node)
                                )
                            })?;
                        let c = evaluation.color.channels();
                        let found = [c[0], c[1], c[2], c[3]];
                        image_cache.insert(key, found);
                        found
                    }
                };
                sampled[..4].copy_from_slice(&cmyk);
            }
            if let Some(stops) = &ramp {
                sampled[..4].copy_from_slice(&ramp_at(stops, f64::from(pixel[0]) / 255.0));
            }
            let value = &sampled;
            for (i, (plate, (touched, value))) in
                plates.iter_mut().zip(touched.iter().zip(value)).enumerate()
            {
                // Process mode 1 leaves a plate alone where the sampled ink is zero.
                let keep = ramp.is_some() && item.overprint == Some(1) && *value <= 0.0 && i < 4;
                if *touched && !keep {
                    let old = plate.samples[index];
                    plate.samples[index] = old * (1.0 - c) + value * c;
                }
            }
        }
    }
    let auto_trapped_pixels = match options.auto_trap {
        Some(units) => {
            ensure!(
                units.is_finite() && units > 0.0 && units <= 1024.0,
                "[invalid-input] automatic trap width must be a number in (0, 1024] units"
            );
            let radius = (units * f64::from(options.scale)).ceil() as usize;
            ensure!(
                radius <= MAX_AUTO_TRAP_PIXELS,
                "[limit-exceeded] automatic trap of {units} units is {radius} px at this scale; the limit is {MAX_AUTO_TRAP_PIXELS} px"
            );
            auto_trap(&mut plates, width as usize, height as usize, radius)
        }
        None => 0,
    };
    // Production marks print in registration color: full ink on every process plate.
    let mut marks = 0;
    if let Some(geometry) = super::production::resolve(page, false)? {
        marks = geometry.paint_marks(width, height, |x, y, coverage| {
            let at = y as usize * width as usize + x as usize;
            for plate in plates.iter_mut().filter(|plate| !plate.spot) {
                let old = plate.samples[at];
                plate.samples[at] = old + (1.0 - old) * coverage;
            }
        });
    }
    let mut slug_text = 0;
    if let Some(geometry) = super::production::resolve(page, false)? {
        slug_text = geometry.paint_slug_text(raw, page, document, options.scale, |x, y, c| {
            if x >= width || y >= height {
                return;
            }
            let at = y as usize * width as usize + x as usize;
            for plate in plates.iter_mut().filter(|plate| !plate.spot) {
                let old = plate.samples[at];
                plate.samples[at] = old + (1.0 - old) * c;
            }
        })?;
    }
    // Color-bar patches knock out everything beneath them and carry only their
    // own process inks; spot plates stay clear.
    let mut bars = 0;
    if let Some(geometry) = super::production::resolve(page, false)? {
        bars = geometry.paint_color_bars(width, height, |x, y, coverage, cmyk| {
            let at = y as usize * width as usize + x as usize;
            for (index, plate) in plates.iter_mut().enumerate() {
                let target = if plate.spot { 0.0 } else { cmyk[index] };
                let old = plate.samples[at];
                plate.samples[at] = old + (target - old) * coverage;
            }
        });
    }
    let inks = spots
        .iter()
        .map(|id| {
            let ink = library.resolve(id)?;
            Ok(
                json!({"id":ink.id,"name":ink.name,"alternate":ink.alternate,
                "redistribution":ink.redistribution}),
            )
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Separations {
        width,
        height,
        scale: options.scale,
        destination: process.as_ref().map(|(d, _)| d.clone()),
        intent: process.as_ref().map(|(_, i)| *i),
        plates,
        objects: items.len(),
        knockouts,
        overprints,
        clipped_paints,
        traps,
        auto_trapped_pixels,
        marks,
        color_bar_patches: bars,
        slug_text,
        inks,
    })
}

/// Largest automatic trap spread in device pixels.
pub const MAX_AUTO_TRAP_PIXELS: usize = 8;

/// Minimum ink-density step between neighbors that counts as an edge to trap.
const AUTO_TRAP_EDGE: f64 = 0.25;

/// Perceptual ink density of one pixel: process plates weighted by how dark
/// each ink prints, spot plates at half their tint.
fn density(plates: &[Plate], index: usize) -> f64 {
    const WEIGHTS: [f64; 4] = [0.30, 0.59, 0.11, 1.0];
    let mut total = 0.0;
    for (i, plate) in plates.iter().enumerate() {
        total += plate.samples[index] * WEIGHTS.get(i).copied().unwrap_or(0.5);
    }
    total.min(1.0)
}

/// Automatic trapping by the classic rule: where a lighter area meets a darker
/// one (ink density differing by at least [`AUTO_TRAP_EDGE`]), the lighter
/// neighbor's ink is spread up to `radius` pixels under the darker area, taking
/// the larger per-plate value. Evaluated against a snapshot of the final plates,
/// so the result is order independent and deterministic. Returns the number of
/// pixels that gained ink. Applies to the plate result only; manual traps and
/// overprint declarations are unchanged.
fn auto_trap(plates: &mut [Plate], width: usize, height: usize, radius: usize) -> usize {
    let n = width * height;
    if n == 0 || radius == 0 {
        return 0;
    }
    let dens: Vec<f64> = (0..n).map(|i| density(plates, i)).collect();
    // Separable windowed minimum finds pixels that have a lighter neighbor.
    let mut horizontal = vec![0.0; n];
    for y in 0..height {
        for x in 0..width {
            let lo = x.saturating_sub(radius);
            let hi = (x + radius).min(width - 1);
            horizontal[y * width + x] = dens[y * width + lo..=y * width + hi]
                .iter()
                .fold(f64::MAX, |a, b| a.min(*b));
        }
    }
    let snapshot: Vec<Vec<f64>> = plates.iter().map(|p| p.samples.clone()).collect();
    let mut changed = 0;
    for y in 0..height {
        for x in 0..width {
            let at = y * width + x;
            let lo_y = y.saturating_sub(radius);
            let hi_y = (y + radius).min(height - 1);
            let lightest = (lo_y..=hi_y).fold(f64::MAX, |a, r| a.min(horizontal[r * width + x]));
            if dens[at] - lightest < AUTO_TRAP_EDGE {
                continue;
            }
            let lo_x = x.saturating_sub(radius);
            let hi_x = (x + radius).min(width - 1);
            let mut best: Vec<f64> = snapshot.iter().map(|plate| plate[at]).collect();
            let mut grew = false;
            for ny in lo_y..=hi_y {
                for nx in lo_x..=hi_x {
                    let other = ny * width + nx;
                    if dens[at] - dens[other] < AUTO_TRAP_EDGE {
                        continue;
                    }
                    for (value, plate) in best.iter_mut().zip(&snapshot) {
                        if plate[other] > *value {
                            *value = plate[other];
                            grew = true;
                        }
                    }
                }
            }
            if grew {
                for (plate, value) in plates.iter_mut().zip(best) {
                    plate.samples[at] = value;
                }
                changed += 1;
            }
        }
    }
    changed
}

/// Device values at gradient position `t`, interpolated linearly between stops.
fn ramp_at(stops: &[(f64, [f64; 4])], t: f64) -> [f64; 4] {
    let index = stops.partition_point(|(p, _)| *p < t);
    if index == 0 {
        return stops[0].1;
    }
    let Some(next) = stops.get(index) else {
        return stops[stops.len() - 1].1;
    };
    let previous = &stops[index - 1];
    let f = ((t - previous.0) / (next.0 - previous.0)).clamp(0.0, 1.0);
    let mut out = [0.0; 4];
    for (o, (a, b)) in out.iter_mut().zip(previous.1.iter().zip(&next.1)) {
        *o = a + (b - a) * f;
    }
    out
}

/// Square-structuring-element maximum filter (separable, deterministic).
fn dilate(source: &[f64], width: usize, height: usize, radius: usize) -> Vec<f64> {
    let mut horizontal = vec![0.0; source.len()];
    for y in 0..height {
        for x in 0..width {
            let lo = x.saturating_sub(radius);
            let hi = (x + radius).min(width - 1);
            horizontal[y * width + x] = source[y * width + lo..=y * width + hi]
                .iter()
                .fold(0.0_f64, |a, b| a.max(*b));
        }
    }
    let mut out = vec![0.0; source.len()];
    for x in 0..width {
        for y in 0..height {
            let lo = y.saturating_sub(radius);
            let hi = (y + radius).min(height - 1);
            out[y * width + x] = (lo..=hi).fold(0.0_f64, |a, r| a.max(horizontal[r * width + x]));
        }
    }
    out
}

impl Separations {
    /// Largest plate raster a composite preview is computed for.
    pub const MAX_PREVIEW_PIXELS: usize = 4_000_000;

    /// Composite preview of the plates: the C, M, Y, K values of each pixel are
    /// read through the separation's ICC output profile to the display role (as
    /// `color proof-image` does), then every spot plate is laid over it as a
    /// multiply of its ink's sRGB alternate at the plate's tint. The spot step is
    /// an approximation (no spectral or overprint-density model); the report says
    /// so. Returns the PNG bytes and a report. Reads the plates only.
    pub fn composite_preview(&self, raw: &Value, document: &Path) -> Result<(Vec<u8>, Value)> {
        use super::{pcs, roles::Role as R};
        let pixels = self.width as usize * self.height as usize;
        ensure!(
            pixels <= Self::MAX_PREVIEW_PIXELS,
            "[limit-exceeded] a composite preview is limited to {} pixels; this separation has {pixels}; separate at a lower --scale",
            Self::MAX_PREVIEW_PIXELS
        );
        let destination = self.destination.as_ref().context(
            "[invalid-input] this separation has no process destination to preview through",
        )?;
        ensure!(
            destination.space() == "icc-cmyk",
            "[unsupported-capability] composite preview needs an ICC CMYK destination, not {}",
            destination.space()
        );
        let reference = destination
            .profile()
            .context("[invalid-input] CMYK destination has no profile")?
            .to_owned();
        let intent = self.intent.context("[internal] separation has no intent")?;
        let display = roles::effective(raw, R::Display)?;
        ensure!(
            display.destination.channel_count() == 3,
            "[unsupported-capability] the display role must be a three-channel RGB destination for a composite preview"
        );
        let mut spots: Vec<(usize, [f64; 3])> = Vec::new();
        for (index, plate) in self.plates.iter().enumerate().filter(|(_, p)| p.spot) {
            let ink = self
                .inks
                .iter()
                .find(|ink| ink["id"] == plate.id.as_str())
                .with_context(|| format!("[internal] ink {} missing from the report", plate.id))?;
            let alternate = &ink["alternate"];
            let channels = (alternate["space"] == "srgb")
                .then(|| alternate["channels"].as_array())
                .flatten()
                .filter(|c| c.len() == 3)
                .and_then(|c| {
                    Some([c[0].as_f64()?, c[1].as_f64()?, c[2].as_f64()?])
                })
                .with_context(|| {
                    format!(
                        "[unsupported-capability] spot ink {} has no sRGB alternate; a composite preview multiplies sRGB alternates",
                        plate.id
                    )
                })?;
            spots.push((index, channels));
        }
        let source_set = ProfileSet::load(raw, document, [reference.as_str()])?;
        let mut source_engine = Evaluator::new(&source_set, intent)?;
        let display_set = ProfileSet::load(raw, document, display.destination.profile())?;
        let mut display_engine = Evaluator::new(&display_set, display.intent)?;
        let mut cache: std::collections::HashMap<[u16; 4], [f64; 3]> =
            std::collections::HashMap::new();
        let mut rgb = Vec::with_capacity(pixels * 3);
        let mut clipped = 0u64;
        for at in 0..pixels {
            crate::composite::check_cancelled()?;
            let mut key = [0u16; 4];
            for (slot, plate) in key.iter_mut().zip(&self.plates) {
                *slot = (plate.samples[at].clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u16;
            }
            let mut value = match cache.get(&key) {
                Some(value) => *value,
                None => {
                    let channels: Vec<f64> = key.iter().map(|v| f64::from(*v) / 255.0).collect();
                    let source = NativeColor::parse(&json!({
                        "space":"icc-cmyk","profile":reference,"channels":channels}))?;
                    let (xyz, _) = source_engine.to_xyz(&source)?;
                    let lab = pcs::xyz_to_lab(xyz)?;
                    let appearance =
                        NativeColor::parse(&json!({"space":"lab-d50","channels":lab,"alpha":1.0}))?;
                    let converted = display_engine.convert(&appearance, &display.destination)?;
                    let device = converted.color.channels();
                    ensure!(
                        device.len() == 3,
                        "[internal] display conversion returned {} channels",
                        device.len()
                    );
                    clipped += u64::from(converted.clipped);
                    let value = [
                        device[0].clamp(0.0, 1.0),
                        device[1].clamp(0.0, 1.0),
                        device[2].clamp(0.0, 1.0),
                    ];
                    cache.insert(key, value);
                    value
                }
            };
            for (index, alternate) in &spots {
                let tint = self.plates[*index].samples[at].clamp(0.0, 1.0);
                if tint > 0.0 {
                    for (v, a) in value.iter_mut().zip(alternate) {
                        *v *= 1.0 - tint * (1.0 - a.clamp(0.0, 1.0));
                    }
                }
            }
            rgb.extend(value.iter().map(|v| (v * 255.0 + 0.5).floor() as u8));
        }
        let buffer = image::RgbImage::from_raw(self.width, self.height, rgb)
            .context("[internal] preview buffer size")?;
        let mut png = Vec::new();
        buffer.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)?;
        let report = json!({
            "version":1,
            "scope":"composite-plate-preview",
            "width":self.width,
            "height":self.height,
            "scale":self.scale,
            "plates":self.plates.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            "unique_process_colors":cache.len(),
            "clipped_colors":clipped,
            "output_profile":reference,
            "intent":intent,
            "display":display.destination.json(),
            "method":"process-plates-through-output-profile-then-spot-alternate-multiply",
            "spot_model":"multiply-of-srgb-alternate-at-tint",
            "paper_simulation":false,
            "spectral_paper_or_ink_model":false,
            "sha256":hex::encode(Sha256::digest(&png)),
        });
        Ok((png, report))
    }

    fn gray(&self, plate: &Plate) -> image::GrayImage {
        // Plate previews are negatives of ink: white paper, black full ink.
        image::GrayImage::from_fn(self.width, self.height, |x, y| {
            let v = plate.samples[y as usize * self.width as usize + x as usize];
            image::Luma([(255.0 - v.clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u8])
        })
    }

    fn gray16(&self, plate: &Plate) -> image::ImageBuffer<image::Luma<u16>, Vec<u16>> {
        image::ImageBuffer::from_fn(self.width, self.height, |x, y| {
            let v = plate.samples[y as usize * self.width as usize + x as usize];
            image::Luma([(65535.0 - v.clamp(0.0, 1.0) * 65535.0 + 0.5).floor() as u16])
        })
    }

    /// Compact deterministic report; plate samples are never inlined.
    pub fn json(&self) -> Value {
        let plates: Vec<Value> = self
            .plates
            .iter()
            .enumerate()
            .map(|(index, plate)| {
                let inked = plate.samples.iter().filter(|v| **v > 0.0).count();
                let max = plate.samples.iter().fold(0.0_f64, |a, b| a.max(*b));
                json!({"index":index,"id":plate.id,"kind":if plate.spot {"spot"} else {"process"},
                    "inked_pixels":inked,"max":max})
            })
            .collect();
        json!({
            "version":1,
            "scope":"plate-separations",
            "width":self.width,
            "height":self.height,
            "scale":self.scale,
            "destination":self.destination.as_ref().map(Destination::json),
            "intent":self.intent,
            "objects":self.objects,
            "knockouts":self.knockouts,
            "overprints":self.overprints,
            "clipped_paints":self.clipped_paints,
            "traps":self.traps,
            "auto_trapped_pixels":self.auto_trapped_pixels,
            "marks":self.marks,
            "color_bar_patches":self.color_bar_patches,
            "slug_text":self.slug_text,
            "plates":plates,
            "inks":self.inks,
        })
    }

    /// Write one 8-bit (default) or 16-bit grayscale PNG per plate plus `separations.json` carrying
    /// the ink map, dimensions and file digests. Existing files are never replaced.
    pub fn write(&self, directory: &Path) -> Result<Value> {
        self.write_with_depth(directory, 8)
    }

    /// As [`Separations::write`], with 8- or 16-bit grayscale plates. Any other
    /// depth is refused before anything is written.
    pub fn write_with_depth(&self, directory: &Path, bits: u8) -> Result<Value> {
        ensure!(
            bits == 8 || bits == 16,
            "[invalid-input] plate bit depth must be 8 or 16, not {bits}"
        );
        std::fs::create_dir_all(directory)
            .with_context(|| format!("create separation directory {}", directory.display()))?;
        let mut report = self.json();
        let mut files = Vec::new();
        let mut outputs: Vec<(String, Vec<u8>)> = Vec::new();
        for (index, plate) in self.plates.iter().enumerate() {
            let file = if plate.spot {
                format!("plate-{index:02}-spot.png")
            } else {
                format!("plate-{index:02}-{}.png", plate.id)
            };
            let mut bytes = Vec::new();
            if bits == 16 {
                self.gray16(plate).write_to(
                    &mut std::io::Cursor::new(&mut bytes),
                    image::ImageFormat::Png,
                )?;
            } else {
                self.gray(plate).write_to(
                    &mut std::io::Cursor::new(&mut bytes),
                    image::ImageFormat::Png,
                )?;
            }
            files.push(json!({"plate":plate.id,"file":file,"bits":bits,
                "sha256":format!("{:x}",Sha256::digest(&bytes))}));
            outputs.push((file, bytes));
        }
        report["bits"] = json!(bits);
        report["files"] = json!(files);
        let mut manifest = serde_json::to_vec_pretty(&report)?;
        manifest.push(b'\n');
        outputs.push(("separations.json".to_owned(), manifest));
        for (file, _) in &outputs {
            ensure!(
                !directory.join(file).exists(),
                "[conflict] {} already exists; choose an empty output directory",
                directory.join(file).display()
            );
        }
        for (file, bytes) in &outputs {
            std::fs::write(directory.join(file), bytes).with_context(|| format!("write {file}"))?;
        }
        Ok(report)
    }
}
