//! Bounded static SVG subset importer for ordinary v4 scene nodes.
//!
//! This first pass intentionally handles geometry and presentation attributes,
//! not the full SVG rendering model. Unsupported static features are reported;
//! active or external content is rejected before a document is returned.

use crate::{
    interchange::{FidelityReport, FidelityReportBuilder, FidelityStatus, ImportLimits},
    scene,
};
use anyhow::{bail, ensure, Context, Result};
use kurbo::Affine;
use roxmltree::{Document, Node, ParsingOptions};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};

const SVG_NS: &str = "http://www.w3.org/2000/svg";
const MAX_PATH_BYTES: usize = 1_000_000;
const MAX_DIMENSION: f64 = 16_384.0;
const MAX_SVG_DOM_NODES: usize = 4_000_000;
const MAX_CSS_RULES: usize = 100_000;
const MAX_CSS_EVALUATION_WORK: usize = 20_000_000;

#[derive(Debug)]
pub struct SvgImport {
    pub document: Value,
    pub report: FidelityReport,
}

/// Import the supported static SVG geometry subset as an ordinary v4 scene.
///
/// Supported elements are `svg`, `g`, `rect`, `circle`, `ellipse`, `line`,
/// `polyline`, `polygon`, and `path`; local `<use>` may instantiate one of
/// these geometries or a static group. Transforms and presentation attributes are
/// retained. Basic single-style text is imported as editable text. CSS,
/// gradients, rectangular clipping, simple static rectangle masks, and filters
/// are handled in the supported subset. Script, animation,
/// foreign namespaces, event handlers, and external references are rejected.
pub fn import_svg(bytes: &[u8], source: &str, limits: ImportLimits) -> Result<SvgImport> {
    crate::interchange::validate_limits(limits)?;
    limits.check_input(bytes)?;
    let svg = std::str::from_utf8(bytes).context("[malformed-resource] SVG must be UTF-8")?;
    ensure!(
        !svg.to_ascii_lowercase().contains("<!doctype"),
        "[unsafe-input] SVG document type declarations are not accepted"
    );
    let dom_node_limit = limits
        .max_nodes
        .saturating_mul(2)
        .saturating_add(2)
        .min(MAX_SVG_DOM_NODES)
        .min(u32::MAX as usize) as u32;
    let xml = Document::parse_with_options(
        svg,
        ParsingOptions {
            allow_dtd: false,
            nodes_limit: dom_node_limit,
        },
    )
    .map_err(|error| {
        if matches!(error, roxmltree::Error::NodesLimitReached) {
            anyhow::anyhow!(
                "[limit-exceeded] SVG XML DOM exceeds the bounded node limit {dom_node_limit}"
            )
        } else {
            anyhow::anyhow!("[malformed-resource] SVG is not well-formed XML: {error}")
        }
    })?;
    let root = xml.root_element();
    ensure!(
        root.tag_name().name() == "svg",
        "[invalid-input] root element must be <svg>"
    );
    ensure!(
        root.tag_name().namespace().is_none_or(|ns| ns == SVG_NS),
        "[unsupported-capability] SVG root uses a foreign namespace"
    );

    let mut elements = 0usize;
    for node in root.descendants().filter(Node::is_element) {
        elements = elements.saturating_add(1);
        limits.check_node_count(elements)?;
        let depth = node
            .ancestors()
            .filter(Node::is_element)
            .count()
            .saturating_sub(1);
        limits.check_nesting(depth)?;
        ensure!(
            node.tag_name().namespace().is_none_or(|ns| ns == SVG_NS),
            "[unsafe-input] foreign namespace element <{}> is rejected",
            node.tag_name().name()
        );
        let name = node.tag_name().name();
        ensure!(
            !matches!(
                name,
                "script"
                    | "foreignObject"
                    | "animate"
                    | "animateMotion"
                    | "animateTransform"
                    | "set"
            ),
            "[unsafe-input] active SVG element <{name}> is rejected"
        );
        for attr in node.attributes() {
            let aname = attr.name().to_ascii_lowercase();
            let value = attr.value().trim();
            ensure!(
                !aname.starts_with("on"),
                "[unsafe-input] SVG event attribute {:?} is rejected",
                attr.name()
            );
            ensure!(
                !(aname == "href" || aname.ends_with(":href")) || value.starts_with('#'),
                "[unsafe-input] external SVG references are rejected"
            );
            ensure!(
                !value.to_ascii_lowercase().contains("url(") || value.contains("url(#"),
                "[unsafe-input] external SVG resource references are rejected"
            );
        }
    }

    let (width, height, root_transform) = viewport(root)?;
    let mut report = FidelityReportBuilder::new("svg", source);
    let css_rules = collect_css_rules(root, &mut report)?;
    ensure!(
        estimated_css_evaluation_work(elements, css_rules.len()) <= MAX_CSS_EVALUATION_WORK,
        "[limit-exceeded] SVG stylesheet evaluation work exceeds the bounded limit"
    );
    let gradients = collect_gradients(root, &mut report)?;
    let clip_rects = collect_clip_rects(root, &mut report)?;
    let masks = collect_masks(root, &css_rules, &mut report)?;
    let mut used_ids = HashSet::new();
    let mut use_stack = Vec::new();
    let mut child_nodes = Vec::new();
    let root_style = computed_style(root, Style::default(), &css_rules)?;
    let root_opacity = root_style.opacity;
    let root_visible = !root_style.display_none;
    import_children(
        root,
        &mut child_nodes,
        &mut used_ids,
        &mut use_stack,
        &mut report,
        limits,
        0,
        root_style,
        &css_rules,
        &gradients,
        &clip_rects,
        &masks,
    )?;
    let wrapper_id = unique_id("svg-root", &mut used_ids);
    let nodes = vec![
        json!({"kind":"group","id":wrapper_id,"transform":affine_array(root_transform),"opacity":root_opacity,"visible":root_visible,"children":child_nodes}),
    ];
    let has_shape_gradient = contains_shape_gradient(&nodes);
    let has_shape_mask = contains_shape_mask(&nodes);
    let has_mask_resources = !masks.is_empty();
    let mut document = scene::new_document(width.ceil() as u32, height.ceil() as u32);
    if has_shape_gradient || has_shape_mask || has_mask_resources {
        document = scene::migrate_to_v12(document)?;
    }
    document["name"] = json!(root
        .attribute("aria-label")
        .or_else(|| root.attribute("id"))
        .unwrap_or("Imported SVG"));
    document["pages"][0]["layers"][0]["nodes"] = json!(nodes);
    if has_mask_resources {
        let mut resources = serde_json::Map::new();
        for mask in masks.values() {
            resources.insert(mask.digest.clone(), mask.resource.clone());
        }
        document["mask_resources"] = Value::Object(resources);
    }
    if has_shape_gradient || has_shape_mask || has_mask_resources {
        scene::validate(&document)
            .context("[invalid-input] imported SVG produced an invalid v12 scene")?;
    } else {
        scene::validate(&document)
            .context("[invalid-input] imported SVG produced an invalid scene")?;
    }
    Ok(SvgImport {
        document,
        report: report.finish(),
    })
}

fn preserve_mapping(
    vx: f64,
    vy: f64,
    vw: f64,
    vh: f64,
    width: f64,
    height: f64,
    value: &str,
) -> Result<Affine> {
    let parts = value.split_ascii_whitespace().collect::<Vec<_>>();
    let parts = if parts.first() == Some(&"defer") {
        &parts[1..]
    } else {
        &parts[..]
    };
    let align = parts.first().copied().unwrap_or("xMidYMid");
    let mode = parts.get(1).copied().unwrap_or("meet");
    ensure!(
        parts.len() <= 2 && matches!(mode, "meet" | "slice"),
        "[invalid-input] SVG preserveAspectRatio must use an alignment and meet or slice"
    );
    let (sx, sy, ox, oy) = if align == "none" {
        ensure!(
            parts.len() == 1,
            "[invalid-input] SVG preserveAspectRatio none cannot include meet or slice"
        );
        (width / vw, height / vh, 0.0, 0.0)
    } else {
        ensure!(
            matches!(
                align,
                "xMinYMin"
                    | "xMidYMin"
                    | "xMaxYMin"
                    | "xMinYMid"
                    | "xMidYMid"
                    | "xMaxYMid"
                    | "xMinYMax"
                    | "xMidYMax"
                    | "xMaxYMax"
            ),
            "[invalid-input] unsupported SVG preserveAspectRatio alignment {align:?}"
        );
        let scale = if mode == "slice" {
            (width / vw).max(height / vh)
        } else {
            (width / vw).min(height / vh)
        };
        let extra_x = width - vw * scale;
        let extra_y = height - vh * scale;
        let ax = if align.contains("xMax") {
            1.0
        } else if align.contains("xMid") {
            0.5
        } else {
            0.0
        };
        let ay = if align.ends_with("YMax") {
            1.0
        } else if align.ends_with("YMid") {
            0.5
        } else {
            0.0
        };
        (scale, scale, extra_x * ax, extra_y * ay)
    };
    Ok(Affine::translate((ox, oy))
        * Affine::scale_non_uniform(sx, sy)
        * Affine::translate((-vx, -vy)))
}

fn viewport(root: Node<'_, '_>) -> Result<(f64, f64, Affine)> {
    let vb = root.attribute("viewBox").map(parse_numbers).transpose()?;
    let (vx, vy, vw, vh) = match vb.as_deref() {
        Some([x, y, w, h]) if *w > 0.0 && *h > 0.0 => (*x, *y, *w, *h),
        Some(_) => {
            bail!("[invalid-input] SVG viewBox must contain four finite numbers with positive size")
        }
        None => {
            let w = root
                .attribute("width")
                .map(length)
                .transpose()?
                .unwrap_or(300.0);
            let h = root
                .attribute("height")
                .map(length)
                .transpose()?
                .unwrap_or(150.0);
            (0.0, 0.0, w, h)
        }
    };
    let width = root
        .attribute("width")
        .map(length)
        .transpose()?
        .unwrap_or(vw);
    let height = root
        .attribute("height")
        .map(length)
        .transpose()?
        .unwrap_or(vh);
    ensure!(
        [vx, vy, vw, vh, width, height]
            .iter()
            .all(|v| v.is_finite())
            && vw > 0.0
            && vh > 0.0
            && width > 0.0
            && height > 0.0,
        "[invalid-input] SVG viewport dimensions must be finite and positive"
    );
    ensure!(
        width <= MAX_DIMENSION && height <= MAX_DIMENSION,
        "[limit-exceeded] SVG canvas dimensions exceed {MAX_DIMENSION}"
    );
    let preserve = root
        .attribute("preserveAspectRatio")
        .unwrap_or("xMidYMid meet");
    let preserve = preserve.split_ascii_whitespace().collect::<Vec<_>>();
    let preserve = if preserve.first() == Some(&"defer") {
        &preserve[1..]
    } else {
        &preserve[..]
    };
    let align = preserve.first().copied().unwrap_or("xMidYMid");
    let mode = preserve.get(1).copied().unwrap_or("meet");
    ensure!(
        preserve.len() <= 2 && matches!(mode, "meet" | "slice"),
        "[invalid-input] SVG preserveAspectRatio must use an alignment and meet or slice"
    );
    let (sx, sy, ox, oy) = if align == "none" {
        ensure!(
            preserve.len() == 1,
            "[invalid-input] SVG preserveAspectRatio none cannot include meet or slice"
        );
        (width / vw, height / vh, 0.0, 0.0)
    } else {
        ensure!(
            matches!(
                align,
                "xMinYMin"
                    | "xMidYMin"
                    | "xMaxYMin"
                    | "xMinYMid"
                    | "xMidYMid"
                    | "xMaxYMid"
                    | "xMinYMax"
                    | "xMidYMax"
                    | "xMaxYMax"
            ),
            "[invalid-input] unsupported SVG preserveAspectRatio alignment {align:?}"
        );
        let scale = if mode == "slice" {
            (width / vw).max(height / vh)
        } else {
            (width / vw).min(height / vh)
        };
        let extra_x = width - vw * scale;
        let extra_y = height - vh * scale;
        let ax = if align.contains("xMax") {
            1.0
        } else if align.contains("xMid") {
            0.5
        } else {
            0.0
        };
        let ay = if align.ends_with("YMax") {
            1.0
        } else if align.ends_with("YMid") {
            0.5
        } else {
            0.0
        };
        (scale, scale, extra_x * ax, extra_y * ay)
    };
    Ok((
        width,
        height,
        Affine::translate((ox, oy))
            * Affine::scale_non_uniform(sx, sy)
            * Affine::translate((-vx, -vy)),
    ))
}

#[derive(Clone)]
struct CssRule {
    selector: String,
    specificity: (u8, u8, u8),
    order: usize,
    declarations: Vec<(String, String)>,
}

/// Parse a deliberately bounded static CSS subset. Only simple element,
/// class, ID, and tag+class/tag+ID selectors are accepted; no selector can
/// cross an SVG tree boundary or trigger network/resource resolution.
fn collect_css_rules(
    root: Node<'_, '_>,
    report: &mut FidelityReportBuilder,
) -> Result<Vec<CssRule>> {
    let mut rules = Vec::new();
    let mut order = 0usize;
    for style_node in root
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "style")
    {
        let source_id = style_node.attribute("id").unwrap_or("$stylesheet");
        let css = style_node.text().unwrap_or("");
        ensure!(
            !css.to_ascii_lowercase().contains("url(")
                && !css.to_ascii_lowercase().contains("@import"),
            "[unsafe-input] external or resource CSS references are rejected"
        );
        let css = strip_css_comments(css)?;
        let mut rest = css.as_str();
        while !rest.trim().is_empty() {
            let open = rest
                .find('{')
                .context("[invalid-input] malformed SVG stylesheet rule")?;
            let close = rest[open + 1..]
                .find('}')
                .context("[invalid-input] unclosed SVG stylesheet rule")?
                + open
                + 1;
            let selectors = rest[..open].trim();
            let declarations = parse_css_declarations(&rest[open + 1..close]);
            if has_unsupported_css_declarations(&rest[open + 1..close]) {
                report.record(
                    source_id,
                    "css_declarations",
                    FidelityStatus::Dropped,
                    "unsupported CSS declarations such as display, variables, or !important were not applied",
                    Some("use supported static presentation properties without !important".into()),
                );
            }
            if declarations.is_empty() {
                report.record(
                    source_id,
                    "stylesheet_rule",
                    FidelityStatus::Dropped,
                    "CSS rule contains no supported static presentation declarations",
                    Some("use supported fill, stroke, typography, or opacity properties".into()),
                );
            }
            for selector in selectors.split(',').map(str::trim) {
                if let Some(specificity) = selector_specificity(selector) {
                    ensure!(
                        rules.len() < MAX_CSS_RULES,
                        "[limit-exceeded] SVG stylesheet exceeds the {MAX_CSS_RULES} rule limit"
                    );
                    order = order.saturating_add(1);
                    rules.push(CssRule {
                        selector: selector.to_owned(),
                        specificity,
                        order,
                        declarations: declarations.clone(),
                    });
                } else {
                    report.record(
                        source_id,
                        "css_selector",
                        FidelityStatus::Dropped,
                        format!("CSS selector `{selector}` is outside the supported simple-selector subset"),
                        Some("replace it with a tag, class, ID, or tag.class selector".into()),
                    );
                }
            }
            rest = &rest[close + 1..];
        }
    }
    Ok(rules)
}

fn estimated_css_evaluation_work(elements: usize, rules: usize) -> usize {
    let sort_factor = usize::BITS as usize - rules.leading_zeros() as usize;
    elements
        .saturating_mul(rules)
        .saturating_mul(sort_factor.max(1))
}

fn strip_css_comments(css: &str) -> Result<String> {
    let mut result = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        result.push_str(&rest[..start]);
        let end = rest[start + 2..]
            .find("*/")
            .context("[invalid-input] unterminated CSS comment")?
            + start
            + 2;
        rest = &rest[end + 2..];
    }
    result.push_str(rest);
    Ok(result)
}

fn selector_specificity(selector: &str) -> Option<(u8, u8, u8)> {
    if selector == "*" {
        return Some((0, 0, 0));
    }
    let parts = selector.split(['.', '#']).collect::<Vec<_>>();
    if parts.len() > 2
        || parts.iter().skip(1).any(|part| part.is_empty())
        || (parts[0].is_empty() && !(selector.starts_with('.') || selector.starts_with('#')))
    {
        return None;
    }
    if !parts[0].is_empty()
        && !parts[0]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return None;
    }
    if parts.len() == 2
        && !parts[1]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return None;
    }
    let mut ids = 0;
    let mut classes = 0;
    let mut tags = 0;
    if selector.contains('#') {
        ids = 1;
    } else if selector.contains('.') {
        classes = 1;
    }
    if !parts[0].is_empty() {
        tags = 1;
    }
    Some((ids, classes, tags))
}

fn selector_matches(node: Node<'_, '_>, selector: &str) -> bool {
    if selector == "*" {
        return true;
    }
    let (tag, id, class) = if let Some((tag, id)) = selector.split_once('#') {
        (tag, Some(id), None)
    } else if let Some((tag, class)) = selector.split_once('.') {
        (tag, None, Some(class))
    } else if let Some(id) = selector.strip_prefix('#') {
        ("", Some(id), None)
    } else if let Some(class) = selector.strip_prefix('.') {
        ("", None, Some(class))
    } else {
        (selector, None, None)
    };
    (tag.is_empty() || node.tag_name().name() == tag)
        && id.is_none_or(|id| node.attribute("id") == Some(id))
        && class.is_none_or(|class| {
            node.attribute("class")
                .is_some_and(|value| value.split_ascii_whitespace().any(|item| item == class))
        })
}

fn parse_css_declarations(input: &str) -> Vec<(String, String)> {
    input
        .split(';')
        .filter_map(|declaration| {
            let (property, value) = declaration.split_once(':')?;
            let property = property.trim().to_ascii_lowercase();
            let value = value.trim();
            (matches!(
                property.as_str(),
                "fill"
                    | "stroke"
                    | "stroke-width"
                    | "opacity"
                    | "stroke-linecap"
                    | "stroke-linejoin"
                    | "stroke-miterlimit"
                    | "font-family"
                    | "font-size"
                    | "font-weight"
                    | "font-style"
                    | "text-anchor"
                    | "letter-spacing"
            ) && !value.to_ascii_lowercase().contains("!important"))
            .then(|| (property, value.to_owned()))
        })
        .collect()
}

fn has_unsupported_css_declarations(input: &str) -> bool {
    input.split(';').any(|declaration| {
        let Some((property, value)) = declaration.split_once(':') else {
            return !declaration.trim().is_empty();
        };
        style_slot_opt(&property.trim().to_ascii_lowercase()).is_none()
            || value.to_ascii_lowercase().contains("!important")
    })
}

#[derive(Clone)]
struct SvgGradient {
    paint: Option<Value>,
    fallback: String,
}

#[derive(Clone, Copy)]
struct ClipRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    object_bounding_box: bool,
}

#[derive(Clone)]
struct SvgMask {
    digest: String,
    resource: Value,
}

fn collect_clip_rects(
    root: Node<'_, '_>,
    report: &mut FidelityReportBuilder,
) -> Result<BTreeMap<String, ClipRect>> {
    let mut clips = BTreeMap::new();
    for clip in root
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "clipPath")
    {
        let Some(id) = clip.attribute("id") else {
            continue;
        };
        let mut elements = clip.children().filter(Node::is_element);
        let Some(rect) = elements.next() else {
            continue;
        };
        let units = clip.attribute("clipPathUnits").unwrap_or("userSpaceOnUse");
        let safe = rect.tag_name().name() == "rect"
            && elements.next().is_none()
            && matches!(units, "userSpaceOnUse" | "objectBoundingBox")
            && clip.attribute("transform").is_none()
            && rect.attribute("transform").is_none()
            && rect.attribute("rx").is_none()
            && rect.attribute("ry").is_none();
        if !safe {
            report.record(
                id,
                "clip_path_definition",
                FidelityStatus::Dropped,
                "Clip path is outside the single untransformed rectangular clip subset",
                Some(
                    "Use one plain rectangle in userSpaceOnUse or objectBoundingBox coordinates"
                        .into(),
                ),
            );
            continue;
        }
        let values = ClipRect {
            x: attr_num(rect, "x", 0.0)?,
            y: attr_num(rect, "y", 0.0)?,
            width: attr_num(rect, "width", 0.0)?,
            height: attr_num(rect, "height", 0.0)?,
            object_bounding_box: units == "objectBoundingBox",
        };
        ensure!(
            values.width >= 0.0 && values.height >= 0.0,
            "[invalid-input] SVG clip rectangle dimensions must be nonnegative"
        );
        clips.insert(id.to_owned(), values);
        report.record(
            id,
            "clip_path_definition",
            FidelityStatus::Exact,
            "Single rectangle clip path can be represented by clipped rectangular geometry",
            None,
        );
    }
    Ok(clips)
}

fn collect_masks(
    root: Node<'_, '_>,
    css_rules: &[CssRule],
    report: &mut FidelityReportBuilder,
) -> Result<BTreeMap<String, SvgMask>> {
    let mut masks = BTreeMap::new();
    for mask in root
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "mask")
    {
        let Some(id) = mask.attribute("id") else {
            continue;
        };
        let mut invalid = || {
            report.record(id, "mask_definition", FidelityStatus::Dropped,
                "SVG mask is outside the one solid rectangular user-space mask subset",
                Some("Use one plain rect with explicit matching user-space mask bounds and no external resources".into()));
        };
        let mut elements = mask.children().filter(Node::is_element);
        let Some(rect) = elements.next() else {
            invalid();
            continue;
        };
        let units_ok = mask.attribute("maskUnits") == Some("userSpaceOnUse")
            && mask.attribute("maskContentUnits") == Some("userSpaceOnUse");
        let geometry_ok = rect.tag_name().name() == "rect"
            && elements.next().is_none()
            && mask.attribute("transform").is_none()
            && rect.attribute("transform").is_none()
            && rect.attribute("rx").is_none()
            && rect.attribute("ry").is_none();
        let bounds = (|| -> Result<(f64, f64, f64, f64)> {
            Ok((
                attr_num(rect, "x", 0.0)?,
                attr_num(rect, "y", 0.0)?,
                attr_num(rect, "width", 0.0)?,
                attr_num(rect, "height", 0.0)?,
            ))
        })();
        let Ok((x, y, width, height)) = bounds else {
            invalid();
            continue;
        };
        let matching_region = mask.attribute("x").and_then(|v| length(v).ok()) == Some(x)
            && mask.attribute("y").and_then(|v| length(v).ok()) == Some(y)
            && mask.attribute("width").and_then(|v| length(v).ok()) == Some(width)
            && mask.attribute("height").and_then(|v| length(v).ok()) == Some(height);
        let mask_type = mask.attribute("mask-type").unwrap_or("luminance");
        if !units_ok
            || !geometry_ok
            || !matching_region
            || width < 0.0
            || height < 0.0
            || !matches!(mask_type, "alpha" | "luminance")
        {
            invalid();
            continue;
        }
        let style = match computed_style(rect, Style::default(), css_rules) {
            Ok(style) => style,
            Err(_) => {
                invalid();
                continue;
            }
        };
        let paint = match svg_color(&style.fill, style.fill_opacity * style.opacity) {
            Ok(paint) if !style.fill.starts_with("url(") => paint,
            _ => {
                invalid();
                continue;
            }
        };
        let hex = paint.trim_start_matches('#');
        let (Ok(red), Ok(green), Ok(blue), Ok(alpha)) = (
            u8::from_str_radix(&hex[0..2], 16),
            u8::from_str_radix(&hex[2..4], 16),
            u8::from_str_radix(&hex[4..6], 16),
            u8::from_str_radix(&hex[6..8], 16),
        ) else {
            invalid();
            continue;
        };
        if style.stroke != "none" {
            invalid();
            continue;
        }
        let opacity = if mask_type == "alpha" {
            f64::from(alpha) / 255.0
        } else {
            let linear = |channel: u8| {
                let srgb = f64::from(channel) / 255.0;
                if srgb <= 0.04045 {
                    srgb / 12.92
                } else {
                    ((srgb + 0.055) / 1.055).powf(2.4)
                }
            };
            let luminance = 0.2126 * linear(red) + 0.7152 * linear(green) + 0.0722 * linear(blue);
            luminance * f64::from(alpha) / 255.0
        };
        let node_id = format!("svg-mask-{}", stable_token(id));
        let node = json!({"kind":"rect","id":node_id,"x":x,"y":y,"width":width,"height":height,
            "style":{"fill":{"fallback":svg_color("white", opacity)?}}});
        let resource = json!({"kind":"vector","node":node});
        let digest = crate::asset::hash_bytes(&serde_json::to_vec(&resource)?);
        ensure!(
            masks.contains_key(id) || masks.len() < crate::mask::MAX_MASKS,
            "[limit-exceeded] SVG contains more than {} supported mask definitions",
            crate::mask::MAX_MASKS
        );
        masks.insert(id.to_owned(), SvgMask { digest, resource });
        report.record(
            id,
            "mask_definition",
            FidelityStatus::Exact,
            format!("Single rectangle {mask_type} mask retained as a reusable vector mask"),
            None,
        );
    }
    Ok(masks)
}

fn stable_token(input: &str) -> String {
    let digest = crate::asset::hash_bytes(input.as_bytes());
    digest[..16].to_owned()
}

fn collect_gradients(
    root: Node<'_, '_>,
    report: &mut FidelityReportBuilder,
) -> Result<BTreeMap<String, SvgGradient>> {
    let mut gradients = BTreeMap::new();
    let nodes = root
        .descendants()
        .filter(Node::is_element)
        .filter(|node| matches!(node.tag_name().name(), "linearGradient" | "radialGradient"))
        .filter_map(|node| node.attribute("id").map(|id| (id.to_owned(), node)))
        .collect::<BTreeMap<_, _>>();
    for (id, node) in &nodes {
        let first = gradient_chain(*node, &nodes)
            .ok()
            .and_then(|chain| chain.into_iter().find_map(gradient_stops))
            .and_then(|stop| {
                let color = stop
                    .attribute("stop-color")
                    .or_else(|| style_declaration(stop, "stop-color"))
                    .unwrap_or("black");
                svg_color(color, 1.0).ok()
            })
            .unwrap_or_else(|| "#000000".into());
        match parse_svg_gradient(*node, &nodes) {
            Ok(paint) => {
                report.record(
                    id,
                    node.tag_name().name(),
                    FidelityStatus::Exact,
                    "Supported objectBoundingBox gradient definition parsed",
                    None,
                );
                gradients.insert(
                    id.clone(),
                    SvgGradient {
                        paint: Some(paint),
                        fallback: first,
                    },
                );
            }
            Err(error) => {
                report.record(
                    id,
                    node.tag_name().name(),
                    FidelityStatus::Dropped,
                    format!("SVG gradient definition was unsupported: {error}"),
                    Some(
                        "use an objectBoundingBox linear or radial gradient with solid hex stops"
                            .into(),
                    ),
                );
                gradients.insert(
                    id.clone(),
                    SvgGradient {
                        paint: None,
                        fallback: first,
                    },
                );
            }
        }
    }
    Ok(gradients)
}

fn gradient_stops<'a, 'input>(node: Node<'a, 'input>) -> Option<Node<'a, 'input>> {
    node.children()
        .find(|child| child.is_element() && child.tag_name().name() == "stop")
}

fn gradient_chain<'a, 'input>(
    node: Node<'a, 'input>,
    nodes: &BTreeMap<String, Node<'a, 'input>>,
) -> Result<Vec<Node<'a, 'input>>> {
    let mut chain = vec![node];
    let mut seen = HashSet::new();
    if let Some(id) = node.attribute("id") {
        seen.insert(id.to_owned());
    }
    loop {
        let current = *chain.last().unwrap();
        let reference = current
            .attribute("href")
            .or_else(|| current.attribute(("http://www.w3.org/1999/xlink", "href")));
        let Some(reference) = reference else { break };
        let id = reference
            .strip_prefix('#')
            .context("gradient inheritance must use a local fragment")?;
        ensure!(
            seen.insert(id.to_owned()),
            "cyclic gradient inheritance reference"
        );
        let parent = *nodes
            .get(id)
            .with_context(|| format!("missing inherited gradient #{id}"))?;
        ensure!(
            parent.tag_name().name() == node.tag_name().name(),
            "gradient inheritance cannot change gradient kind"
        );
        chain.push(parent);
        ensure!(
            chain.len() <= 256,
            "gradient inheritance exceeds 256 definitions"
        );
    }
    Ok(chain)
}

fn inherited_attribute<'a, 'input>(chain: &[Node<'a, 'input>], name: &str) -> Option<&'a str> {
    chain.iter().find_map(|node| node.attribute(name))
}

fn parse_svg_gradient<'a, 'input>(
    node: Node<'a, 'input>,
    nodes: &BTreeMap<String, Node<'a, 'input>>,
) -> Result<Value> {
    let chain = gradient_chain(node, nodes)?;
    let tag = node.tag_name().name();
    let kind = match tag {
        "linearGradient" => "linear",
        "radialGradient" => "radial",
        _ => bail!("not a gradient element"),
    };
    ensure!(
        inherited_attribute(&chain, "gradientUnits")
            .is_none_or(|units| units == "objectBoundingBox"),
        "only objectBoundingBox units are supported"
    );
    let mut gradient = json!({"kind":kind,"spread":inherited_attribute(&chain,"spreadMethod").unwrap_or("pad"),"space":if inherited_attribute(&chain,"color-interpolation")==Some("linearRGB") {"linear"} else {"srgb"}});
    if let Some(transform) = inherited_attribute(&chain, "gradientTransform") {
        gradient["transform"] = json!(affine_array(parse_transform(Some(transform))?));
    }
    if kind == "linear" {
        for (key, default) in [("x1", 0.0), ("y1", 0.0), ("x2", 1.0), ("y2", 0.0)] {
            gradient[key] = json!(bbox_coordinate(inherited_attribute(&chain, key), default)?);
        }
    } else {
        for (key, default) in [("cx", 0.5), ("cy", 0.5), ("radius", 0.5)] {
            gradient[key] = json!(bbox_coordinate(inherited_attribute(&chain, key), default)?);
        }
        let cx = gradient["cx"].as_f64().unwrap();
        let cy = gradient["cy"].as_f64().unwrap();
        for key in ["fx", "fy"] {
            if let Some(value) = inherited_attribute(&chain, key) {
                let focal = bbox_coordinate(Some(value), if key == "fx" { cx } else { cy })?;
                ensure!(
                    (focal - if key == "fx" { cx } else { cy }).abs() < 1e-12,
                    "off-center radial focal points are unsupported"
                );
            }
        }
    }
    let mut stops = Vec::new();
    let mut previous = 0.0f64;
    let stop_source = chain
        .iter()
        .find(|candidate| gradient_stops(**candidate).is_some())
        .context("gradient has no direct stops in its inheritance chain")?;
    for stop in stop_source
        .children()
        .filter(|child| child.is_element() && child.tag_name().name() == "stop")
    {
        let offset = stop
            .attribute("offset")
            .map(parse_offset)
            .transpose()?
            .unwrap_or(previous);
        ensure!(
            offset >= previous && offset <= 1.0,
            "gradient stop offsets must be nondecreasing in 0..=1"
        );
        let color = stop
            .attribute("stop-color")
            .or_else(|| style_declaration(stop, "stop-color"))
            .unwrap_or("black");
        let opacity = stop
            .attribute("stop-opacity")
            .or_else(|| style_declaration(stop, "stop-opacity"))
            .map(parse_opacity)
            .transpose()?
            .unwrap_or(1.0);
        stops.push(json!({"position":offset,"color":svg_color(color, opacity)?}));
        previous = offset;
    }
    ensure!(
        stops.len() >= 2,
        "gradient requires at least two direct stop children"
    );
    ensure!(
        stops
            .windows(2)
            .all(|pair| pair[0]["position"] != pair[1]["position"]),
        "duplicate stop offsets (hard stops) are unsupported"
    );
    if stops.first().unwrap()["position"] != 0.0 {
        let mut first = stops[0].clone();
        first["position"] = json!(0.0);
        stops.insert(0, first);
    }
    if stops.last().unwrap()["position"] != 1.0 {
        let mut last = stops.last().unwrap().clone();
        last["position"] = json!(1.0);
        stops.push(last);
    }
    gradient["stops"] = json!(stops);
    Ok(gradient)
}

fn bbox_coordinate(value: Option<&str>, default: f64) -> Result<f64> {
    let Some(value) = value else {
        return Ok(default);
    };
    let value = value.trim();
    let coordinate = if let Some(percent) = value.strip_suffix('%') {
        percent.parse::<f64>()? / 100.0
    } else {
        value.parse::<f64>()?
    };
    ensure!(
        coordinate.is_finite() && coordinate.abs() <= 1_000_000.0,
        "gradient coordinate must be finite and bounded"
    );
    Ok(coordinate)
}

fn parse_offset(value: &str) -> Result<f64> {
    let offset = bbox_coordinate(Some(value), 0.0)?;
    ensure!(
        (0.0..=1.0).contains(&offset),
        "gradient stop offset must be in 0..=1"
    );
    Ok(offset)
}

fn parse_opacity(value: &str) -> Result<f64> {
    let opacity = value.trim().parse::<f64>()?;
    ensure!(
        opacity.is_finite() && (0.0..=1.0).contains(&opacity),
        "stop opacity must be in 0..=1"
    );
    Ok(opacity)
}

fn svg_color(value: &str, opacity: f64) -> Result<String> {
    let value = value.trim().to_ascii_lowercase();
    let value = match value.as_str() {
        "black" => "#000000".to_owned(),
        "white" => "#ffffff".to_owned(),
        "red" => "#ff0000".to_owned(),
        "green" => "#008000".to_owned(),
        "blue" => "#0000ff".to_owned(),
        "yellow" => "#ffff00".to_owned(),
        "gray" | "grey" => "#808080".to_owned(),
        "transparent" | "none" => "#00000000".to_owned(),
        other if other.starts_with('#') => other.to_owned(),
        _ => bail!("stop color must be hex or one of the supported basic color names"),
    };
    let hex = value.strip_prefix('#').unwrap();
    let expanded = match hex.len() {
        3 => format!(
            "{}ff",
            hex.chars().flat_map(|ch| [ch, ch]).collect::<String>()
        ),
        4 => hex.chars().flat_map(|ch| [ch, ch]).collect::<String>(),
        6 => format!("{hex}ff"),
        8 => hex.to_owned(),
        _ => bail!("stop color must use #RGB, #RGBA, #RRGGBB, or #RRGGBBAA"),
    };
    ensure!(
        expanded.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "stop color has invalid hex digits"
    );
    let alpha = u8::from_str_radix(&expanded[6..8], 16)?;
    let alpha = (f64::from(alpha) * opacity).round() as u8;
    Ok(format!("#{}{:02x}", &expanded[..6], alpha))
}

fn style_declaration<'a>(node: Node<'a, 'a>, property: &str) -> Option<&'a str> {
    node.attribute("style")?.split(';').find_map(|declaration| {
        let (key, value) = declaration.split_once(':')?;
        (key.trim() == property).then_some(value.trim())
    })
}

fn contains_shape_gradient(nodes: &[Value]) -> bool {
    nodes.iter().any(|node| {
        ["fill", "stroke"].iter().any(|slot| {
            matches!(
                node.pointer(&format!("/style/{slot}/fallback/kind"))
                    .and_then(Value::as_str),
                Some("linear" | "radial")
            )
        }) || node["children"]
            .as_array()
            .is_some_and(|children| contains_shape_gradient(children))
    })
}

fn contains_shape_mask(nodes: &[Value]) -> bool {
    nodes.iter().any(|node| {
        node.get("mask").is_some()
            || node["children"]
                .as_array()
                .is_some_and(|children| contains_shape_mask(children))
    })
}

// These values are the per-import parse context and are mutated together as nodes arrive.
#[allow(clippy::too_many_arguments)]
fn import_children<'a, 'input>(
    parent: Node<'a, 'input>,
    out: &mut Vec<Value>,
    ids: &mut HashSet<String>,
    use_stack: &mut Vec<String>,
    report: &mut FidelityReportBuilder,
    limits: ImportLimits,
    depth: usize,
    inherited: Style,
    css_rules: &[CssRule],
    gradients: &BTreeMap<String, SvgGradient>,
    clip_rects: &BTreeMap<String, ClipRect>,
    masks: &BTreeMap<String, SvgMask>,
) -> Result<()> {
    limits.check_nesting(depth)?;
    for node in parent.children().filter(Node::is_element) {
        let tag = node.tag_name().name();
        let source_id = node
            .attribute("id")
            .map(str::to_owned)
            .unwrap_or_else(|| format!("{tag}-{}", ids.len() + 1));
        let id = unique_id(&source_id, ids);
        ensure!(
            ids.len() <= limits.max_nodes,
            "[limit-exceeded] expanded SVG content exceeds the bounded node limit"
        );
        report_unsupported_declarations(node, &source_id, report);
        if node.attribute("filter").is_some() {
            report.record(
                source_id.clone(),
                "filter",
                FidelityStatus::Dropped,
                "SVG filter is not representable by the v4 scene renderer and was not applied",
                Some("Bake the effect into paths or pixels before import".into()),
            );
        }
        match tag {
            "g" => {
                if node.attribute("mask").is_some() {
                    report.record(
                        source_id.clone(),
                        "mask",
                        FidelityStatus::Dropped,
                        "SVG group masks are outside the simple rectangular shape mask subset",
                        None,
                    );
                }
                if node.attribute("clip-path").is_some() {
                    report.record(source_id.clone(), "clip_path", FidelityStatus::Dropped,
                        "Group clipping is not representable by the scene schema and was not applied",
                        Some("Apply a supported rectangular clip directly to an untransformed fill-only rectangle".into()));
                }
                let mut children = Vec::new();
                let style = computed_style(node, inherited.clone(), css_rules)?;
                import_children(
                    node,
                    &mut children,
                    ids,
                    use_stack,
                    report,
                    limits,
                    depth + 1,
                    style.clone(),
                    css_rules,
                    gradients,
                    clip_rects,
                    masks,
                )?;
                let transform = parse_transform(node.attribute("transform"))?;
                let group = json!({"kind":"group","id":id,"transform":affine_array(transform),"children":children,"opacity":style.opacity,"visible":!style.display_none});
                out.push(group);
                report.record(
                    source_id,
                    "group",
                    FidelityStatus::Exact,
                    "Group structure and transform preserved",
                    None,
                );
            }
            "rect" | "circle" | "ellipse" | "line" | "polyline" | "polygon" | "path" => {
                let node_value = geometry_node(
                    node,
                    tag,
                    id,
                    inherited.clone(),
                    SvgStyleContext {
                        css_rules,
                        gradients,
                        clip_rects,
                        masks,
                        allow_rect_clip: depth == 0,
                    },
                    &source_id,
                    report,
                )?;
                out.push(node_value);
                report.record(
                    source_id,
                    "geometry_and_presentation",
                    FidelityStatus::Exact,
                    "Static geometry and supported presentation attributes preserved",
                    None,
                );
            }
            "svg" => {
                let mut children = Vec::new();
                let style = computed_style(node, inherited.clone(), css_rules)?;
                import_children(
                    node,
                    &mut children,
                    ids,
                    use_stack,
                    report,
                    limits,
                    depth + 1,
                    style.clone(),
                    css_rules,
                    gradients,
                    clip_rects,
                    masks,
                )?;
                out.push(json!({"kind":"group","id":id,"transform":affine_array(parse_transform(node.attribute("transform"))?),"children":children,"opacity":style.opacity,"visible":!style.display_none}));
                report.record(source_id,"nested_viewport",FidelityStatus::Approximated,"Nested SVG viewport dimensions are not independently applied; only its transform is preserved",Some("Flatten the nested viewport before import".into()));
            }
            "defs" => {
                for feature in node.descendants().filter(Node::is_element).skip(1) {
                    let feature_tag = feature.tag_name().name();
                    if !matches!(feature_tag, "title" | "desc" | "metadata") {
                        let feature_id = feature.attribute("id").unwrap_or(feature_tag);
                        if is_expanded_use_definition(feature) {
                            continue;
                        }
                        if matches!(feature_tag, "linearGradient" | "radialGradient")
                            && gradients.contains_key(feature_id)
                        {
                            continue;
                        }
                        if feature_tag == "clipPath" && clip_rects.contains_key(feature_id) {
                            continue;
                        }
                        if feature_tag == "mask" && masks.contains_key(feature_id) {
                            continue;
                        }
                        if feature_tag == "stop"
                            && feature
                                .ancestors()
                                .find(|ancestor| {
                                    ancestor.is_element()
                                        && matches!(
                                            ancestor.tag_name().name(),
                                            "linearGradient" | "radialGradient"
                                        )
                                })
                                .and_then(|gradient| gradient.attribute("id"))
                                .is_some_and(|gradient_id| gradients.contains_key(gradient_id))
                        {
                            continue;
                        }
                        report.record(feature_id, feature_tag, FidelityStatus::Dropped,
                            format!("SVG definition <{feature_tag}> is not imported or resolved in this subset"),
                            Some("Inline or convert the referenced effect to paths before import".into()));
                    }
                }
            }
            "text" => {
                if node.attribute("mask").is_some() {
                    report.record(
                        source_id.clone(),
                        "mask",
                        FidelityStatus::Dropped,
                        "SVG text masks are outside the simple rectangular shape mask subset",
                        None,
                    );
                }
                let has_spans = node
                    .descendants()
                    .filter(Node::is_element)
                    .any(|child| child.tag_name().name() == "tspan");
                let (text, approximate) = import_text(
                    node,
                    id,
                    inherited.clone(),
                    css_rules,
                    gradients,
                    &source_id,
                    report,
                )?;
                out.push(text);
                if has_spans || approximate {
                    report.record(source_id, "text_runs", FidelityStatus::Approximated,
                        "Text content and basic typography were preserved; per-run styling or advanced text positioning was approximated",
                        Some("Split styled SVG runs into separate Pentool text nodes and position unsupported glyph runs explicitly".into()));
                } else {
                    report.record(source_id, "text", FidelityStatus::Exact,
                        "Text content, basic typography, anchor, position, and transform preserved as editable text", None);
                }
            }
            "title" | "desc" | "metadata" => {}
            "use" => {
                let href = node
                    .attribute("href")
                    .or_else(|| node.attribute(("http://www.w3.org/1999/xlink", "href")));
                let target = href
                    .and_then(|href| href.strip_prefix('#'))
                    .and_then(|reference| {
                        node.document()
                            .root_element()
                            .descendants()
                            .find(|candidate| candidate.attribute("id") == Some(reference))
                    });
                let target_tag = target.map(|target| target.tag_name().name());
                if let (Some(target), Some(target_tag)) = (
                    target,
                    target_tag.filter(|tag| {
                        matches!(
                            *tag,
                            "g" | "symbol"
                                | "rect"
                                | "circle"
                                | "ellipse"
                                | "line"
                                | "polyline"
                                | "polygon"
                                | "path"
                        )
                    }),
                ) {
                    ensure!(
                        ids.len().saturating_add(2) <= limits.max_nodes,
                        "[limit-exceeded] expanded SVG use instances exceed the bounded node limit"
                    );
                    let use_style = computed_style(node, inherited.clone(), css_rules)?;
                    let x = node.attribute("x").map(length).transpose()?.unwrap_or(0.0);
                    let y = node.attribute("y").map(length).transpose()?.unwrap_or(0.0);
                    let use_transform =
                        parse_transform(node.attribute("transform"))? * Affine::translate((x, y));
                    if target_tag == "g" || target_tag == "symbol" {
                        let target_source_id = target
                            .attribute("id")
                            .unwrap_or("anonymous-group")
                            .to_owned();
                        report_unsupported_declarations(target, &target_source_id, report);
                        for (attribute, feature) in [
                            ("mask", "mask"),
                            ("clip-path", "clip_path"),
                            ("filter", "filter"),
                        ] {
                            if target.attribute(attribute).is_some() {
                                report.record(
                                    &target_source_id,
                                    feature,
                                    FidelityStatus::Dropped,
                                    format!("Referenced SVG group {feature} is outside the supported static subset"),
                                    None,
                                );
                            }
                        }
                        let target_style = computed_style(target, use_style.clone(), css_rules)?;
                        let mut children = Vec::new();
                        let target_ref = target.attribute("id").unwrap_or("<anonymous>").to_owned();
                        ensure!(
                            !use_stack.contains(&target_ref),
                            "[invalid-input] cyclic SVG use reference to #{target_ref}"
                        );
                        use_stack.push(target_ref.clone());
                        let children_result = import_children(
                            target,
                            &mut children,
                            ids,
                            use_stack,
                            report,
                            limits,
                            depth + 1,
                            target_style.clone(),
                            css_rules,
                            gradients,
                            clip_rects,
                            masks,
                        );
                        use_stack.pop();
                        children_result?;
                        let mut instance_transform = use_transform;
                        if target_tag == "symbol" {
                            let vb = target.attribute("viewBox").map(parse_numbers).transpose()?;
                            let Some([vx, vy, vw, vh]) = vb.as_deref() else {
                                bail!("[invalid-input] SVG symbol #{target_ref} requires a four-number viewBox")
                            };
                            ensure!(
                                *vw > 0.0 && *vh > 0.0,
                                "[invalid-input] SVG symbol viewBox must have positive size"
                            );
                            let width = node
                                .attribute("width")
                                .map(length)
                                .transpose()?
                                .unwrap_or(*vw);
                            let height = node
                                .attribute("height")
                                .map(length)
                                .transpose()?
                                .unwrap_or(*vh);
                            ensure!(width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0 && width <= MAX_DIMENSION && height <= MAX_DIMENSION,
                                "[invalid-input] SVG symbol instance dimensions must be finite, positive, and bounded");
                            instance_transform *= preserve_mapping(
                                *vx,
                                *vy,
                                *vw,
                                *vh,
                                width,
                                height,
                                target
                                    .attribute("preserveAspectRatio")
                                    .unwrap_or("xMidYMid meet"),
                            )?;
                        }
                        let group_id = unique_id(&format!("{source_id}-content"), ids);
                        let target_group = json!({
                            "kind":"group",
                            "id":group_id,
                            "transform":affine_array(parse_transform(target.attribute("transform"))?),
                            "opacity":target_style.opacity,
                            "visible":!target_style.visibility_hidden && !target_style.display_none,
                            "children":children
                        });
                        out.push(json!({
                            "kind":"group",
                            "id":id,
                            "transform":affine_array(instance_transform),
                            "opacity":use_style.opacity,
                            "visible":!use_style.visibility_hidden && !use_style.display_none,
                            "children":[target_group]
                        }));
                    } else {
                        let target_id = unique_id(&format!("{source_id}-content"), ids);
                        let mut shape = geometry_node(
                            target,
                            target_tag,
                            target_id,
                            use_style,
                            SvgStyleContext {
                                css_rules,
                                gradients,
                                clip_rects,
                                masks,
                                allow_rect_clip: false,
                            },
                            &source_id,
                            report,
                        )?;
                        let target_transform = parse_transform(target.attribute("transform"))?;
                        shape["transform"] = json!(affine_array(use_transform * target_transform));
                        out.push(json!({"kind":"group","id":id,"transform":affine_array(Affine::IDENTITY),"children":[shape]}));
                    }
                    report.record(source_id, "use_reference", FidelityStatus::Exact,
                        "Local SVG use of supported static geometry, a group, or a viewBox symbol expanded into editable nodes", None);
                } else {
                    report.record(source_id, "use_reference", FidelityStatus::Dropped,
                        "SVG use reference is missing or targets an element outside the supported static subset",
                        Some("Reference one local static group, viewBox symbol, rect, circle, ellipse, line, polyline, polygon, or path".into()));
                }
            }
            "style" => report.record(
                source_id,
                "stylesheet",
                FidelityStatus::Exact,
                "supported static CSS declarations were applied to matching SVG elements",
                None,
            ),
            "linearGradient" | "radialGradient" | "clipPath" | "mask" | "filter" | "tspan"
            | "pattern" | "marker" => {
                report.record(
                    source_id,
                    tag,
                    FidelityStatus::Dropped,
                    format!("SVG {tag} is outside the supported static geometry subset"),
                    Some("Convert to paths or rasterize before import".into()),
                );
            }
            _ => report.record(
                source_id,
                tag,
                FidelityStatus::Dropped,
                format!("Unsupported SVG element <{tag}> was not imported"),
                None,
            ),
        }
    }
    Ok(())
}

struct SvgStyleContext<'a> {
    css_rules: &'a [CssRule],
    gradients: &'a BTreeMap<String, SvgGradient>,
    clip_rects: &'a BTreeMap<String, ClipRect>,
    masks: &'a BTreeMap<String, SvgMask>,
    allow_rect_clip: bool,
}

fn geometry_node(
    node: Node<'_, '_>,
    tag: &str,
    id: String,
    inherited: Style,
    style_context: SvgStyleContext<'_>,
    source_id: &str,
    report: &mut FidelityReportBuilder,
) -> Result<Value> {
    let style = computed_style(node, inherited, style_context.css_rules)?;
    let mut transform = parse_transform(node.attribute("transform"))?;
    let mut val = match tag {
        "rect" => {
            let x = attr_num(node, "x", 0.0)?;
            let y = attr_num(node, "y", 0.0)?;
            let w = attr_num(node, "width", 0.0)?;
            let h = attr_num(node, "height", 0.0)?;
            ensure!(
                w >= 0.0 && h >= 0.0,
                "[invalid-input] SVG rect dimensions must be nonnegative"
            );
            let mut v = json!({"kind":"rect","id":id,"x":x,"y":y,"width":w,"height":h});
            if let Some(rx) = node.attribute("rx") {
                let r = length(rx)?;
                v["radius_x"] = json!(r);
                v["radius_y"] = json!(node.attribute("ry").map(length).transpose()?.unwrap_or(r));
            }
            v
        }
        "circle" => {
            let r = attr_num(node, "r", 0.0)?;
            let cx = attr_num(node, "cx", 0.0)?;
            let cy = attr_num(node, "cy", 0.0)?;
            ensure!(
                r >= 0.0,
                "[invalid-input] circle radius must be nonnegative"
            );
            json!({"kind":"ellipse","id":id,"cx":cx,"cy":cy,"radius_x":r,"radius_y":r})
        }
        "ellipse" => {
            let rx = attr_num(node, "rx", 0.0)?;
            let ry = attr_num(node, "ry", 0.0)?;
            let cx = attr_num(node, "cx", 0.0)?;
            let cy = attr_num(node, "cy", 0.0)?;
            ensure!(
                rx >= 0.0 && ry >= 0.0,
                "[invalid-input] ellipse radii must be nonnegative"
            );
            json!({"kind":"ellipse","id":id,"cx":cx,"cy":cy,"radius_x":rx,"radius_y":ry})
        }
        "line" => {
            let x1 = attr_num(node, "x1", 0.0)?;
            let y1 = attr_num(node, "y1", 0.0)?;
            let x2 = attr_num(node, "x2", 0.0)?;
            let y2 = attr_num(node, "y2", 0.0)?;
            json!({"kind":"line","id":id,"x1":x1,"y1":y1,"x2":x2,"y2":y2})
        }
        "path" => {
            let d = node
                .attribute("d")
                .context("[invalid-input] SVG path has no d attribute")?;
            ensure!(
                d.len() <= MAX_PATH_BYTES,
                "[limit-exceeded] SVG path exceeds {MAX_PATH_BYTES} bytes"
            );
            for segment in svgtypes::PathParser::from(d) {
                segment.context("[invalid-input] malformed SVG path")?;
            }
            json!({"kind":"path","id":id,"d":d,"closed":d.trim_end().ends_with(['z','Z'])})
        }
        "polygon" | "polyline" => {
            let pts = parse_numbers(node.attribute("points").unwrap_or(""))?;
            ensure!(
                pts.len() >= 4 && pts.len() % 2 == 0,
                "[invalid-input] SVG {tag} requires at least two coordinate pairs"
            );
            let mut d = format!("M {} {}", pts[0], pts[1]);
            for p in pts[2..].chunks_exact(2) {
                d.push_str(&format!(" L {} {}", p[0], p[1]));
            }
            let closed = tag == "polygon";
            if closed {
                d.push_str(" Z");
            }
            json!({"kind":"path","id":id,"d":d,"closed":closed})
        }
        _ => unreachable!(),
    };
    if let Some(t) = node.attribute("transform") {
        transform = parse_transform(Some(t))?;
    }
    val["transform"] = json!(affine_array(transform));
    let fill = imported_paint(
        &style.fill,
        style.fill_opacity,
        style_context.gradients,
        source_id,
        "fill",
        report,
    )?;
    let stroke = imported_paint(
        &style.stroke,
        style.stroke_opacity,
        style_context.gradients,
        source_id,
        "stroke",
        report,
    )?;
    val["style"] = json!({"fill":{"fallback":fill},"stroke":{"fallback":stroke},"stroke_width":{"fallback":style.stroke_width},"stroke_linecap":{"fallback":style.linecap},"stroke_linejoin":{"fallback":style.linejoin},"stroke_miterlimit":{"fallback":style.miter_limit}});
    val["opacity"] = json!(style.opacity);
    val["visible"] = json!(!style.visibility_hidden && !style.display_none);
    if let Some(reference) = node
        .attribute("clip-path")
        .and_then(|value| value.strip_prefix("url(#"))
        .and_then(|value| value.strip_suffix(')'))
    {
        let compatible = tag == "rect"
            && style_context.allow_rect_clip
            && node.attribute("transform").is_none()
            && node.attribute("rx").is_none()
            && node.attribute("ry").is_none()
            && style.stroke == "none"
            && !style.fill.starts_with("url(");
        if let (true, Some(clip)) = (compatible, style_context.clip_rects.get(reference)) {
            let x0 = x_or(&val["x"]);
            let y0 = x_or(&val["y"]);
            let x1 = x0 + x_or(&val["width"]);
            let y1 = y0 + x_or(&val["height"]);
            let (clip_x, clip_y, clip_width, clip_height) = if clip.object_bounding_box {
                let target_width = x1 - x0;
                let target_height = y1 - y0;
                (
                    x0 + clip.x * target_width,
                    y0 + clip.y * target_height,
                    clip.width * target_width,
                    clip.height * target_height,
                )
            } else {
                (clip.x, clip.y, clip.width, clip.height)
            };
            let left = x0.max(clip_x);
            let top = y0.max(clip_y);
            let right = x1.min(clip_x + clip_width);
            let bottom = y1.min(clip_y + clip_height);
            val["x"] = json!(left);
            val["y"] = json!(top);
            val["width"] = json!((right - left).max(0.0));
            val["height"] = json!((bottom - top).max(0.0));
            report.record(source_id, "clip_path", FidelityStatus::Exact,
                "Single rectangular clip path intersected with an untransformed, unrounded, fill-only rectangle", None);
        } else {
            report.record(
                source_id,
                "clip_path",
                FidelityStatus::Dropped,
                "Clip path cannot be represented by this shape and was not applied",
                Some("Use a single untransformed rectangular clip on a fill-only rectangle".into()),
            );
        }
    } else if node.attribute("clip-path").is_some() {
        report.record(
            source_id,
            "clip_path",
            FidelityStatus::Dropped,
            "Clip path reference is malformed or external and was not applied",
            Some("Use a local url(#id) reference to a supported clip rectangle".into()),
        );
    }
    if let Some(mask_value) = node.attribute("mask") {
        let reference = mask_value
            .strip_prefix("url(#")
            .and_then(|value| value.strip_suffix(')'));
        let compatible = tag == "rect"
            && style_context.allow_rect_clip
            && node.attribute("transform").is_none()
            && node.attribute("rx").is_none()
            && node.attribute("ry").is_none();
        if let (true, Some(mask)) = (
            compatible,
            reference.and_then(|id| style_context.masks.get(id)),
        ) {
            val["mask"] = json!({"resource":mask.digest,"invert":false,"linked":false,
                "density":1.0,"feather":0.0,"transform":[1,0,0,1,0,0]});
            report.record(
                source_id,
                "mask",
                FidelityStatus::Exact,
                "Single-rectangle alpha or luminance mask applied as a reusable vector mask",
                None,
            );
        } else {
            report.record(source_id, "mask", FidelityStatus::Dropped,
                "SVG mask is malformed, unsupported, or attached to a shape outside the supported subset",
                Some("Use a local mask reference on a top-level untransformed rectangle and the supported static mask definition".into()));
        }
    }
    Ok(val)
}

fn x_or(value: &Value) -> f64 {
    value.as_f64().unwrap_or_default()
}

fn imported_paint(
    value: &str,
    opacity: f64,
    gradients: &BTreeMap<String, SvgGradient>,
    source_id: &str,
    slot: &str,
    report: &mut FidelityReportBuilder,
) -> Result<Value> {
    let Some(reference) = value
        .strip_prefix("url(#")
        .and_then(|reference| reference.strip_suffix(')'))
    else {
        return apply_paint_opacity(json!(value), opacity);
    };
    let gradient = gradients.get(reference).with_context(|| {
        format!("[missing-resource] SVG {slot} references missing gradient #{reference}")
    })?;
    if let Some(paint) = &gradient.paint {
        report.record(
            source_id,
            format!("{slot}_gradient"),
            FidelityStatus::Exact,
            format!("SVG {slot} linear/radial gradient imported as native v12 vector paint"),
            None,
        );
        Ok(apply_paint_opacity(paint.clone(), opacity)?)
    } else {
        report.record(
            source_id,
            format!("{slot}_gradient"),
            FidelityStatus::Dropped,
            format!("unsupported SVG {slot} gradient was replaced by its first stop color"),
            Some("use a supported objectBoundingBox linear/radial gradient".into()),
        );
        Ok(apply_paint_opacity(json!(gradient.fallback), opacity)?)
    }
}

fn apply_paint_opacity(mut paint: Value, opacity: f64) -> Result<Value> {
    if opacity >= 1.0 || paint == "none" {
        return Ok(paint);
    }
    fn scale_color(value: &mut Value, opacity: f64) -> Result<()> {
        if let Some(color) = value.as_str() {
            *value = json!(svg_color(color, opacity)?);
        }
        Ok(())
    }
    if paint.is_string() {
        scale_color(&mut paint, opacity)?;
    } else if let Some(stops) = paint.get_mut("stops").and_then(Value::as_array_mut) {
        for stop in stops {
            if let Some(color) = stop.get_mut("color") {
                scale_color(color, opacity)?;
            }
        }
    }
    Ok(paint)
}

#[derive(Clone)]
struct Style {
    fill: String,
    stroke: String,
    stroke_width: f64,
    opacity: f64,
    fill_opacity: f64,
    stroke_opacity: f64,
    visibility_hidden: bool,
    display_none: bool,
    linecap: String,
    linejoin: String,
    miter_limit: f64,
    font_family: String,
    font_size: f64,
    font_weight: u16,
    italic: bool,
    anchor: String,
    letter_spacing: f64,
}
impl Default for Style {
    fn default() -> Self {
        Self {
            fill: "black".into(),
            stroke: "none".into(),
            stroke_width: 1.0,
            opacity: 1.0,
            visibility_hidden: false,
            display_none: false,
            fill_opacity: 1.0,
            stroke_opacity: 1.0,
            linecap: "butt".into(),
            linejoin: "miter".into(),
            miter_limit: 4.0,
            font_family: "sans-serif".into(),
            font_size: 16.0,
            font_weight: 400,
            italic: false,
            anchor: "start".into(),
            letter_spacing: 0.0,
        }
    }
}
fn computed_style(node: Node<'_, '_>, mut s: Style, css_rules: &[CssRule]) -> Result<Style> {
    // SVG opacity is a compositing property on this element, not an inherited
    // paint property. Group opacity is emitted on the group itself.
    s.opacity = 1.0;
    s.display_none = false;
    for (key, slot) in [
        ("fill", 0),
        ("stroke", 1),
        ("stroke-width", 2),
        ("opacity", 3),
        ("stroke-linecap", 4),
        ("stroke-linejoin", 5),
        ("stroke-miterlimit", 16),
        ("font-family", 6),
        ("font-size", 7),
        ("font-weight", 8),
        ("font-style", 9),
        ("text-anchor", 10),
        ("letter-spacing", 11),
        ("fill-opacity", 12),
        ("stroke-opacity", 13),
        ("display", 14),
        ("visibility", 15),
    ] {
        if let Some(v) = node.attribute(key) {
            assign_style(&mut s, slot, v)?;
        }
    }
    let mut matched = css_rules
        .iter()
        .filter(|rule| selector_matches(node, &rule.selector))
        .collect::<Vec<_>>();
    matched.sort_by_key(|rule| (rule.specificity, rule.order));
    for rule in matched {
        for (property, value) in &rule.declarations {
            assign_style(&mut s, style_slot(property), value)?;
        }
    }
    if let Some(style) = node.attribute("style") {
        for item in style.split(';').filter(|s| !s.trim().is_empty()) {
            let (k, v) = item
                .split_once(':')
                .context("[invalid-input] malformed SVG inline style")?;
            ensure!(
                !v.to_ascii_lowercase().contains("url(") || internal_svg_paint_reference(v),
                "[unsafe-input] external SVG paint references are rejected"
            );
            if let Some(slot) = style_slot_opt(k.trim()) {
                if v.to_ascii_lowercase().contains("!important") {
                    continue;
                }
                let value = v.trim();
                assign_style(&mut s, slot, value)?;
            }
        }
    }
    Ok(s)
}

fn report_unsupported_declarations(
    node: Node<'_, '_>,
    source_id: &str,
    report: &mut FidelityReportBuilder,
) {
    const UNSUPPORTED_PRESENTATION: &[&str] = &[
        "color",
        "fill-rule",
        "clip-rule",
        "stroke-dasharray",
        "stroke-dashoffset",
        "paint-order",
        "vector-effect",
        "shape-rendering",
        "text-decoration",
        "dominant-baseline",
        "baseline-shift",
        "word-spacing",
        "font-variant",
        "font-stretch",
        "marker-start",
        "marker-mid",
        "marker-end",
    ];
    for attr in node.attributes() {
        let property = attr.name().to_ascii_lowercase();
        if property == "style" {
            for declaration in attr.value().split(';') {
                let Some((name, value)) = declaration.split_once(':') else {
                    continue;
                };
                let name = name.trim().to_ascii_lowercase();
                let supported_stop_property = node.tag_name().name() == "stop"
                    && matches!(name.as_str(), "stop-color" | "stop-opacity");
                if !name.is_empty()
                    && !supported_stop_property
                    && (style_slot_opt(&name).is_none()
                        || value.to_ascii_lowercase().contains("!important"))
                {
                    report.record(
                        source_id,
                        format!("inline_style:{name}"),
                        FidelityStatus::Dropped,
                        format!("Unsupported SVG inline style `{name}` was not applied"),
                        Some(
                            "convert this property to supported geometry or paint before import"
                                .into(),
                        ),
                    );
                }
            }
        } else if UNSUPPORTED_PRESENTATION.contains(&property.as_str()) {
            report.record(
                source_id,
                format!("presentation:{property}"),
                FidelityStatus::Dropped,
                format!("Unsupported SVG presentation property `{property}` was not applied"),
                Some("convert this property to supported geometry or paint before import".into()),
            );
        }
    }
}
fn style_slot(property: &str) -> usize {
    style_slot_opt(property).expect("declarations were filtered to supported properties")
}
fn style_slot_opt(property: &str) -> Option<usize> {
    Some(match property {
        "fill" => 0,
        "stroke" => 1,
        "stroke-width" => 2,
        "opacity" => 3,
        "stroke-linecap" => 4,
        "stroke-linejoin" => 5,
        "stroke-miterlimit" => 16,
        "font-family" => 6,
        "font-size" => 7,
        "font-weight" => 8,
        "font-style" => 9,
        "text-anchor" => 10,
        "letter-spacing" => 11,
        "fill-opacity" => 12,
        "stroke-opacity" => 13,
        "display" => 14,
        "visibility" => 15,
        _ => return None,
    })
}
fn assign_style(s: &mut Style, k: usize, v: &str) -> Result<()> {
    match k {
        0 => s.fill = v.trim().to_owned(),
        1 => s.stroke = v.trim().to_owned(),
        2 => s.stroke_width = length(v)?,
        3 => {
            s.opacity = v
                .parse()
                .context("[invalid-input] SVG opacity must be numeric")?;
            ensure!(
                s.opacity.is_finite() && (0.0..=1.0).contains(&s.opacity),
                "[invalid-input] SVG opacity must be in 0..=1"
            )
        }
        12 | 13 => {
            let opacity = v
                .parse::<f64>()
                .context("[invalid-input] SVG paint opacity must be numeric")?;
            ensure!(
                opacity.is_finite() && (0.0..=1.0).contains(&opacity),
                "[invalid-input] SVG paint opacity must be in 0..=1"
            );
            if k == 12 {
                s.fill_opacity = opacity;
            } else {
                s.stroke_opacity = opacity;
            }
        }
        14 => {
            let display = v.trim();
            ensure!(
                matches!(
                    display,
                    "none" | "inline" | "block" | "contents" | "inherit" | "initial" | "unset"
                ),
                "[unsupported-capability] unsupported SVG display value {display:?}"
            );
            s.display_none = display == "none";
        }
        15 => {
            let visibility = v.trim();
            ensure!(
                matches!(
                    visibility,
                    "visible" | "hidden" | "collapse" | "inherit" | "initial" | "unset"
                ),
                "[unsupported-capability] unsupported SVG visibility value {visibility:?}"
            );
            s.visibility_hidden = matches!(visibility, "hidden" | "collapse");
        }
        4 => {
            ensure!(
                matches!(v, "butt" | "round" | "square"),
                "[unsupported-capability] unsupported SVG stroke-linecap"
            );
            s.linecap = v.into()
        }
        5 => {
            ensure!(
                matches!(v, "miter" | "round" | "bevel"),
                "[unsupported-capability] unsupported SVG stroke-linejoin"
            );
            s.linejoin = v.into()
        }
        16 => {
            s.miter_limit = v
                .parse()
                .context("[invalid-input] SVG stroke-miterlimit must be numeric")?;
            ensure!(
                s.miter_limit.is_finite() && (1.0..=1000.0).contains(&s.miter_limit),
                "[invalid-input] SVG stroke-miterlimit must be in 1..=1000"
            );
        }
        6 => {
            s.font_family = v
                .trim_matches(['\'', '"'])
                .split(',')
                .next()
                .unwrap_or("sans-serif")
                .trim()
                .to_owned();
            ensure!(
                !s.font_family.is_empty(),
                "[invalid-input] SVG font-family must not be empty"
            );
        }
        7 => {
            s.font_size = length(v)?;
            ensure!(
                s.font_size > 0.0,
                "[invalid-input] SVG font-size must be positive"
            );
        }
        8 => {
            s.font_weight = match v {
                "normal" => 400,
                "bold" | "bolder" => 700,
                "lighter" => 300,
                _ => v
                    .parse::<u16>()
                    .context("[invalid-input] SVG font-weight must be normal, bold, or 1..=1000")?,
            };
            ensure!(
                (1..=1000).contains(&s.font_weight),
                "[invalid-input] SVG font-weight must be in 1..=1000"
            );
        }
        9 => {
            ensure!(
                matches!(v, "normal" | "italic" | "oblique"),
                "[unsupported-capability] unsupported SVG font-style"
            );
            s.italic = v != "normal";
        }
        10 => {
            ensure!(
                matches!(v, "start" | "middle" | "end"),
                "[unsupported-capability] unsupported SVG text-anchor"
            );
            s.anchor = v.into();
        }
        11 => s.letter_spacing = length(v)?,
        _ => {}
    }
    ensure!(
        (!s.fill.to_ascii_lowercase().contains("url(") || internal_svg_paint_reference(&s.fill))
            && (!s.stroke.to_ascii_lowercase().contains("url(")
                || internal_svg_paint_reference(&s.stroke)),
        "[unsafe-input] SVG paint reference must be a local url(#id)"
    );
    Ok(())
}

fn internal_svg_paint_reference(value: &str) -> bool {
    let value = value.trim();
    value
        .strip_prefix("url(#")
        .and_then(|value| value.strip_suffix(')'))
        .is_some_and(|id| !id.is_empty() && !id.chars().any(char::is_whitespace))
}

fn import_text(
    node: Node<'_, '_>,
    id: String,
    inherited: Style,
    css_rules: &[CssRule],
    gradients: &BTreeMap<String, SvgGradient>,
    source_id: &str,
    report: &mut FidelityReportBuilder,
) -> Result<(Value, bool)> {
    let mut style = computed_style(node, inherited, css_rules)?;
    for (slot, paint) in [("fill", &mut style.fill), ("stroke", &mut style.stroke)] {
        if let Some(reference) = paint
            .strip_prefix("url(#")
            .and_then(|value| value.strip_suffix(')'))
        {
            let gradient = gradients.get(reference).with_context(|| {
                format!(
                    "[missing-resource] SVG text {slot} references missing gradient #{reference}"
                )
            })?;
            *paint = gradient.fallback.clone();
            report.record(
                source_id,
                format!("text_{slot}_gradient"),
                FidelityStatus::Dropped,
                "SVG gradient text paint is not represented by the editable text node and was replaced by the first stop color",
                Some("outline or rasterize gradient text before import".into()),
            );
        }
    }
    let preserve = node
        .ancestors()
        .filter(Node::is_element)
        .find_map(|ancestor| ancestor.attribute(("http://www.w3.org/XML/1998/namespace", "space")))
        == Some("preserve");
    let mut content = String::new();
    for descendant in node.descendants().filter(Node::is_text) {
        if let Some(text) = descendant.text() {
            content.push_str(text);
        }
    }
    if !preserve {
        content = content.split_whitespace().collect::<Vec<_>>().join(" ");
    }
    let x = node
        .attribute("x")
        .map(parse_numbers)
        .transpose()?
        .unwrap_or_default();
    let y = node
        .attribute("y")
        .map(parse_numbers)
        .transpose()?
        .unwrap_or_default();
    let text_fill = if style.fill_opacity < 1.0 {
        svg_color(&style.fill, style.fill_opacity)?
    } else {
        style.fill.clone()
    };
    let mut value = json!({
        "kind":"text", "id":id, "content":content,
        "x":x.first().copied().unwrap_or(0.0),
        "y":y.first().copied().unwrap_or(0.0),
        "font_family":style.font_family, "font_size":style.font_size,
        "font_weight":style.font_weight, "italic":style.italic,
        "align":match style.anchor.as_str() { "middle" => "center", "end" => "right", _ => "left" },
        "letter_spacing":style.letter_spacing,
        "opacity":style.opacity,
        "visible":!style.visibility_hidden && !style.display_none,
        "style":{"fill":{"fallback":text_fill}}
    });
    value["transform"] = json!(affine_array(parse_transform(node.attribute("transform"))?));
    let has_stroke = style.stroke != "none";
    let advanced_positioning = x.len() > 1
        || y.len() > 1
        || node.attribute("dx").is_some()
        || node.attribute("dy").is_some()
        || node.attribute("rotate").is_some()
        || node.attribute("textLength").is_some();
    if has_stroke {
        value["import_source"] =
            json!({"stroke_dropped":true,"stroke":style.stroke,"stroke_width":style.stroke_width});
    }
    Ok((value, has_stroke || advanced_positioning))
}

fn attr_num(node: Node<'_, '_>, key: &str, default: f64) -> Result<f64> {
    node.attribute(key)
        .map(length)
        .transpose()
        .map(|v| v.unwrap_or(default))
}
fn length(s: &str) -> Result<f64> {
    let value = s
        .trim()
        .trim_end_matches("px")
        .parse::<f64>()
        .context("[invalid-input] SVG length must be a unitless or px number")?;
    ensure!(
        value.is_finite(),
        "[invalid-input] SVG length must be finite"
    );
    Ok(value)
}
fn parse_numbers(s: &str) -> Result<Vec<f64>> {
    let mut out = Vec::new();
    for part in s
        .split(|c: char| c.is_ascii_whitespace() || c == ',')
        .filter(|p| !p.is_empty())
    {
        let v = part
            .parse::<f64>()
            .context("[invalid-input] malformed SVG number")?;
        ensure!(v.is_finite(), "[invalid-input] SVG numbers must be finite");
        out.push(v);
    }
    Ok(out)
}
fn parse_transform(input: Option<&str>) -> Result<Affine> {
    let Some(s) = input else {
        return Ok(Affine::IDENTITY);
    };
    let mut out = Affine::IDENTITY;
    let mut rest = s.trim();
    while !rest.is_empty() {
        let open = rest
            .find('(')
            .context("[invalid-input] malformed SVG transform")?;
        let close = rest[open + 1..]
            .find(')')
            .context("[invalid-input] malformed SVG transform")?
            + open
            + 1;
        let name = rest[..open].trim();
        let p = parse_numbers(&rest[open + 1..close])?;
        let t = match name {
            "matrix" if p.len() == 6 => Affine::new([p[0], p[1], p[2], p[3], p[4], p[5]]),
            "translate" if (1..=2).contains(&p.len()) => {
                Affine::translate((p[0], *p.get(1).unwrap_or(&0.0)))
            }
            "scale" if (1..=2).contains(&p.len()) => {
                Affine::scale_non_uniform(p[0], *p.get(1).unwrap_or(&p[0]))
            }
            "rotate" if p.len() == 1 => Affine::rotate(p[0].to_radians()),
            "rotate" if p.len() == 3 => {
                Affine::translate((p[1], p[2]))
                    * Affine::rotate(p[0].to_radians())
                    * Affine::translate((-p[1], -p[2]))
            }
            "skewX" if p.len() == 1 => {
                Affine::new([1.0, 0.0, p[0].to_radians().tan(), 1.0, 0.0, 0.0])
            }
            "skewY" if p.len() == 1 => {
                Affine::new([1.0, p[0].to_radians().tan(), 0.0, 1.0, 0.0, 0.0])
            }
            _ => bail!("[unsupported-capability] unsupported SVG transform {name}"),
        };
        out *= t;
        rest = rest[close + 1..].trim_start_matches(|c: char| c.is_ascii_whitespace() || c == ',');
    }
    Ok(out)
}
fn affine_array(a: Affine) -> [f64; 6] {
    let c = a.as_coeffs();
    [c[0], c[1], c[2], c[3], c[4], c[5]]
}
fn unique_id(source: &str, used: &mut HashSet<String>) -> String {
    let mut base = String::from("svg-");
    for c in source.chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
            base.push(c)
        } else {
            base.push('-')
        }
    }
    if base == "svg-" {
        base.push_str("node")
    }
    let mut id = base.clone();
    let mut n = 2;
    while !used.insert(id.clone()) {
        id = format!("{base}-{n}");
        n += 1;
    }
    id
}

fn root_references_use(defs: Node<'_, '_>, id: &str) -> bool {
    defs.document()
        .root_element()
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "use")
        .any(|node| {
            node.attribute("href")
                .or_else(|| node.attribute(("http://www.w3.org/1999/xlink", "href")))
                == Some(format!("#{id}").as_str())
        })
}

fn is_expanded_use_definition(node: Node<'_, '_>) -> bool {
    let mut current = Some(node);
    while let Some(candidate) = current {
        let tag = candidate.tag_name().name();
        if matches!(
            tag,
            "g" | "symbol"
                | "rect"
                | "circle"
                | "ellipse"
                | "line"
                | "polyline"
                | "polygon"
                | "path"
        ) && candidate
            .attribute("id")
            .is_some_and(|id| root_references_use(node, id))
        {
            return true;
        }
        if tag == "defs" {
            break;
        }
        current = candidate.parent().filter(Node::is_element);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn expands_local_symbol_use_with_viewbox_and_preserve_aspect_ratio() {
        let imported = import_svg(
            br##"<svg xmlns="http://www.w3.org/2000/svg" width="30" height="20"><defs><symbol id="icon" viewBox="0 0 10 10"><rect x="1" y="1" width="2" height="2" fill="#ff0000"/></symbol></defs><use id="instance" href="#icon" x="2" y="1" width="20" height="10"/></svg>"##,
            "symbol-use.svg", ImportLimits::default()).unwrap();
        scene::validate(&imported.document).unwrap();
        assert!(imported
            .report
            .items
            .iter()
            .any(|i| i.source_id == "instance"
                && i.feature == "use_reference"
                && i.status == FidelityStatus::Exact));
        assert!(!imported
            .report
            .items
            .iter()
            .any(|i| i.source_id == "icon" && i.status == FidelityStatus::Dropped));
        let doc = scene::migrate_to_v5(imported.document).unwrap();
        let svg = crate::image::to_svg(&doc, std::path::Path::new("symbol-use.svg"), None).unwrap();
        let png = crate::render::scene_to_png(&svg, 1.0).unwrap();
        let pixels = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(pixels.get_pixel(8, 2).0, [255, 0, 0, 255]);
        assert_eq!(pixels.get_pixel(3, 2).0, [255, 255, 255, 255]);
    }

    #[test]
    fn expands_local_use_references_to_editable_geometry_with_unique_ids() {
        let imported = import_svg(
            br##"<svg xmlns="http://www.w3.org/2000/svg" width="30" height="20"><defs><rect id="tile" x="1" y="2" width="5" height="6" fill="#ff0000"/></defs><use id="first" href="#tile" x="10"/><use id="second" href="#tile" x="20"/></svg>"##,
            "use.svg",
            ImportLimits::default(),
        )
        .unwrap();
        crate::scene::validate(&imported.document).unwrap();
        let nodes = &imported.document["pages"][0]["layers"][0]["nodes"][0]["children"];
        assert_eq!(nodes.as_array().unwrap().len(), 2);
        assert_eq!(nodes[0]["children"][0]["kind"], "rect");
        assert_eq!(nodes[0]["children"][0]["transform"][4], 10.0);
        assert_eq!(nodes[1]["children"][0]["transform"][4], 20.0);
        assert_ne!(nodes[0]["children"][0]["id"], nodes[1]["children"][0]["id"]);
        assert!(imported
            .report
            .items
            .iter()
            .filter(|item| item.feature == "use_reference")
            .all(|item| item.status == FidelityStatus::Exact));
        let render_document = crate::scene::migrate_to_v5(imported.document.clone()).unwrap();
        let svg =
            crate::image::to_svg(&render_document, std::path::Path::new("use.svg"), None).unwrap();
        let png = crate::render::scene_to_png(&svg, 1.0).unwrap();
        let pixels = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(pixels.get_pixel(12, 3).0, [255, 0, 0, 255]);
        assert_eq!(pixels.get_pixel(17, 3).0, [255, 255, 255, 255]);
        assert_eq!(pixels.get_pixel(22, 3).0, [255, 0, 0, 255]);
    }

    #[test]
    fn expands_local_group_use_with_nested_transforms_and_bounds_cycles() {
        let imported = import_svg(
            br##"<svg xmlns="http://www.w3.org/2000/svg" width="30" height="20"><defs><g id="set" transform="translate(2 0)" clip-path="url(#clip)"><rect id="tile" x="1" y="2" width="5" height="6" fill="#ff0000"/></g></defs><use id="instance" href="#set" x="10"/></svg>"##,
            "group-use.svg",
            ImportLimits::default(),
        )
        .unwrap();
        scene::validate(&imported.document).unwrap();
        assert!(imported.report.items.iter().any(|item| {
            item.source_id == "instance"
                && item.feature == "use_reference"
                && item.status == FidelityStatus::Exact
        }));
        assert!(imported.report.items.iter().any(|item| {
            item.source_id == "set"
                && item.feature == "clip_path"
                && item.status == FidelityStatus::Dropped
        }));
        let document = scene::migrate_to_v5(imported.document).unwrap();
        let svg =
            crate::image::to_svg(&document, std::path::Path::new("group-use.svg"), None).unwrap();
        let png = crate::render::scene_to_png(&svg, 1.0).unwrap();
        let pixels = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(pixels.get_pixel(14, 3).0, [255, 0, 0, 255]);
        assert_eq!(pixels.get_pixel(20, 3).0, [255, 255, 255, 255]);

        let cyclic = br##"<svg xmlns="http://www.w3.org/2000/svg"><defs><g id="loop"><use href="#loop"/></g></defs><use href="#loop"/></svg>"##;
        assert!(import_svg(cyclic, "cycle.svg", ImportLimits::default()).is_err());
    }

    #[test]
    fn bounds_expansion_of_reused_svg_definitions() {
        let repeated = br##"<svg xmlns="http://www.w3.org/2000/svg"><defs><rect id="tile" width="1" height="1"/></defs><use href="#tile"/><use href="#tile"/><use href="#tile"/></svg>"##;
        let limits = ImportLimits {
            max_nodes: 6,
            ..ImportLimits::default()
        };
        let error = import_svg(repeated, "reused.svg", limits).unwrap_err();
        assert!(format!("{error:#}").contains("expanded SVG use instances"));
    }

    #[test]
    fn imports_geometry_and_stable_ids() {
        let s=br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50" viewBox="0 0 200 100"><g id="layer" transform="translate(3 4)"><rect id="box" x="2" y="3" width="10" height="20" fill="#123456"/><path id="curve" d="M0 0 C1 2 3 4 5 6"/></g></svg>"##;
        let a = import_svg(s, "a.svg", ImportLimits::default()).unwrap();
        let b = import_svg(s, "a.svg", ImportLimits::default()).unwrap();
        assert_eq!(a.document, b.document);
        assert_eq!(a.document["version"], 4);
        assert_eq!(a.report.counts.exact, 3);
        scene::validate(&a.document).unwrap();
    }
    #[test]
    fn refuses_active_external_and_doctype_input() {
        for s in [
            r#"<!DOCTYPE svg><svg/>"#,
            r#"<svg><script>alert(1)</script></svg>"#,
            r#"<svg><image href="https://example.invalid/x.png"/></svg>"#,
        ] {
            assert!(import_svg(s.as_bytes(), "bad.svg", ImportLimits::default()).is_err());
        }
    }

    #[test]
    fn rejects_oversized_hostile_input_before_xml_parsing() {
        let limits = ImportLimits {
            max_input_bytes: 16,
            ..ImportLimits::default()
        };
        // Deliberately malformed XML: the byte limit must win before the XML
        // parser examines or allocates a DOM for hostile oversized input.
        let hostile = b"<svg><g><g>malformed-but-over-limit";
        let error = import_svg(hostile, "oversized.svg", limits).unwrap_err();
        assert!(
            format!("{error:#}").contains("[limit-exceeded]"),
            "oversized input reached XML parsing instead of the input bound: {error:#}"
        );
    }

    #[test]
    fn bounds_depth_and_path_size() {
        let limits = ImportLimits {
            max_nesting: 2,
            ..ImportLimits::default()
        };
        assert!(import_svg(br#"<svg><g><g><g/></g></g></svg>"#, "deep.svg", limits).is_err());
        let s = format!(
            "<svg><path d=\"{}\"/></svg>",
            "M0 0 ".repeat(MAX_PATH_BYTES / 5 + 1)
        );
        assert!(
            import_svg(s.as_bytes(), "large.svg", ImportLimits::default())
                .unwrap_err()
                .to_string()
                .contains("limit-exceeded")
        );
    }

    #[test]
    fn bounds_xml_dom_node_allocation_during_parsing() {
        let limits = ImportLimits {
            max_nodes: 2,
            ..ImportLimits::default()
        };
        let hostile = format!("<svg>{}</svg>", "<rect/>".repeat(64));
        let error = import_svg(hostile.as_bytes(), "node-bomb.svg", limits).unwrap_err();
        assert!(
            format!("{error:#}").contains("[limit-exceeded] SVG XML DOM"),
            "hostile XML was not rejected by the parser allocation bound: {error:#}"
        );
    }

    #[test]
    fn viewport_slice_and_skew_transforms_follow_svg_geometry() {
        let (_, _, slice) = viewport(
            Document::parse(r#"<svg viewBox="0 0 100 100" width="100" height="50" preserveAspectRatio="xMidYMid slice"/>"#)
                .unwrap()
                .root_element(),
        )
        .unwrap();
        assert_eq!(slice.as_coeffs(), [1.0, 0.0, 0.0, 1.0, 0.0, -25.0]);

        let skew = parse_transform(Some("skewX(45) skewY(45)")).unwrap();
        let coeffs = skew.as_coeffs();
        assert!((coeffs[1] - 1.0).abs() < 1e-12);
        assert!((coeffs[2] - 1.0).abs() < 1e-12);
    }

    #[test]
    fn rejects_malformed_preserve_aspect_ratio_instead_of_silently_meeting() {
        let xml = Document::parse(
            r#"<svg viewBox="0 0 100 100" width="100" height="50" preserveAspectRatio="xBogusYMid slice"/>"#,
        )
        .unwrap();
        assert!(viewport(xml.root_element())
            .unwrap_err()
            .to_string()
            .contains("unsupported SVG preserveAspectRatio alignment"));
    }

    #[test]
    fn rejects_stylesheet_matching_work_above_budget() {
        let mut svg = String::from("<svg><style>");
        for _ in 0..4_500 {
            svg.push_str("rect{fill:#123456}");
        }
        svg.push_str("</style>");
        for _ in 0..4_500 {
            svg.push_str("<rect width='1' height='1'/>");
        }
        svg.push_str("</svg>");
        let error = import_svg(svg.as_bytes(), "many-rules.svg", ImportLimits::default())
            .unwrap_err()
            .to_string();
        assert!(error.contains("stylesheet evaluation work"), "{error}");
    }

    #[test]
    fn reports_unsupported_static_features() {
        let a = import_svg(
            br##"<svg><defs><linearGradient id="g"><stop offset="0" stop-color="#000"/></linearGradient><clipPath id="c"><path d="M0 0L4 0L4 4Z"/></clipPath></defs><text id="title">Hello</text><filter id="f"/><rect id="clipped" width="4" height="4" clip-path="url(#c)"/></svg>"##,
            "x.svg",
            ImportLimits::default(),
        )
        .unwrap();
        assert!(a.report.counts.dropped >= 4);
        assert!(a.report.items.iter().any(|item| {
            item.source_id == "title"
                && item.feature == "text"
                && item.status == FidelityStatus::Exact
        }));
        assert!(a
            .report
            .items
            .iter()
            .any(|item| item.source_id == "clipped" && item.feature == "clip_path"));
        assert!(a
            .report
            .items
            .iter()
            .any(|item| item.source_id == "g" && item.feature == "linearGradient"));
    }

    #[test]
    fn imports_linear_and_radial_gradients_as_native_v12_paints() {
        for (definition, paint_kind) in [
            (
                r##"<linearGradient id="paint"><stop offset="0" stop-color="#f00"/><stop offset="1" stop-color="#00f"/></linearGradient>"##,
                "linearGradient",
            ),
            (
                r##"<radialGradient id="paint"><stop offset="0" stop-color="#f00"/><stop offset="1" stop-color="#00f"/></radialGradient>"##,
                "radialGradient",
            ),
        ] {
            let svg = format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg"><defs>{definition}</defs><rect id="target" width="10" height="10" fill="url(#paint)"/></svg>"#
            );
            let imported = import_svg(svg.as_bytes(), "paint-server.svg", ImportLimits::default())
                .unwrap_or_else(|error| panic!("{paint_kind} import failed: {error:#}"));
            assert_eq!(imported.document["version"], 12);
            crate::scene::validate(&imported.document).unwrap();
            let shape = &imported.document["pages"][0]["layers"][0]["nodes"][0]["children"][0];
            assert_eq!(
                shape["style"]["fill"]["fallback"]["kind"],
                if paint_kind == "linearGradient" {
                    "linear"
                } else {
                    "radial"
                }
            );
            assert!(imported.report.items.iter().any(|item| {
                item.source_id == "target"
                    && item.feature == "fill_gradient"
                    && item.status == FidelityStatus::Exact
            }));

            let exported = crate::image::to_svg(
                &imported.document,
                std::path::Path::new("paint-server.svg"),
                None,
            )
            .unwrap();
            assert!(
                exported.svg.contains("<linearGradient")
                    || exported.svg.contains("<radialGradient")
            );
            let png = crate::render::scene_to_png(&exported, 1.0).unwrap();
            let pixels = image::load_from_memory(&png).unwrap().to_rgba8();
            assert_ne!(pixels.get_pixel(1, 5).0, pixels.get_pixel(9, 5).0);
        }
    }

    #[test]
    fn inherits_svg_gradient_attributes_and_stops_with_cycle_guard() {
        let imported = import_svg(
            include_bytes!("../docs/fixtures/svg-conformance/gradient-inheritance.svg"),
            "gradient-inheritance.svg",
            ImportLimits::default(),
        )
        .unwrap();
        let shapes = &imported.document["pages"][0]["layers"][0]["nodes"][0]["children"];
        let paint = &shapes[0]["style"]["fill"]["fallback"];
        assert_eq!(paint["kind"], "linear");
        assert_eq!(paint["x1"], 0.0);
        assert_eq!(paint["x2"], 0.5);
        assert_eq!(paint["spread"], "reflect");
        assert_eq!(paint["stops"].as_array().unwrap().len(), 2);
        assert!(imported.report.items.iter().any(|item| {
            item.source_id == "inherited-gradient"
                && item.feature == "fill_gradient"
                && item.status == FidelityStatus::Exact
        }));
        crate::scene::validate(&imported.document).unwrap();

        let cycle = br##"<svg><defs><linearGradient id="a" href="#b"/><linearGradient id="b" href="#a"/></defs><rect id="target" width="5" height="5" fill="url(#a)"/></svg>"##;
        let imported = import_svg(cycle, "gradient-cycle.svg", ImportLimits::default()).unwrap();
        assert!(imported.report.items.iter().any(|item| {
            item.source_id == "target"
                && item.feature == "fill_gradient"
                && item.status == FidelityStatus::Dropped
        }));
    }

    #[test]
    fn rectangular_clip_paths_are_applied_and_unsupported_targets_are_reported() {
        let imported = import_svg(
            br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><defs><clipPath id="half"><rect width="5" height="10"/></clipPath></defs><rect id="target" width="10" height="10" fill="#ff0000" clip-path="url(#half)"/><ellipse id="other" cx="8" cy="8" rx="2" ry="2" fill="#00ff00" clip-path="url(#half)"/></svg>"##,
            "clip.svg",
            ImportLimits::default(),
        )
        .unwrap();
        let target = &imported.document["pages"][0]["layers"][0]["nodes"][0]["children"][0];
        assert_eq!(target["width"], 5.0);
        assert!(imported.report.items.iter().any(|item| {
            item.source_id == "target"
                && item.feature == "clip_path"
                && item.status == FidelityStatus::Exact
        }));
        assert!(imported.report.items.iter().any(|item| {
            item.source_id == "other"
                && item.feature == "clip_path"
                && item.status == FidelityStatus::Dropped
        }));
        let document = crate::scene::migrate_to_v5(imported.document).unwrap();
        let scene =
            crate::image::to_svg(&document, std::path::Path::new("clip.svg"), None).unwrap();
        let png = crate::render::scene_to_png(&scene, 1.0).unwrap();
        let pixels = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(pixels.get_pixel(2, 5).0, [255, 0, 0, 255]);
        assert_eq!(pixels.get_pixel(7, 2).0, [255, 255, 255, 255]);
    }

    #[test]
    fn object_bounding_box_rectangular_clips_follow_target_bounds() {
        let imported = import_svg(
            br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50"><defs><clipPath id="center" clipPathUnits="objectBoundingBox"><rect x="0.25" y="0.25" width="0.5" height="0.5"/></clipPath></defs><rect id="target" x="10" y="5" width="80" height="40" fill="#ff0000" clip-path="url(#center)"/></svg>"##,
            "object-bbox-clip.svg",
            ImportLimits::default(),
        )
        .unwrap();
        let target = &imported.document["pages"][0]["layers"][0]["nodes"][0]["children"][0];
        assert_eq!(target["x"], 30.0);
        assert_eq!(target["y"], 15.0);
        assert_eq!(target["width"], 40.0);
        assert_eq!(target["height"], 20.0);
        assert!(imported.report.items.iter().any(|item| {
            item.source_id == "target"
                && item.feature == "clip_path"
                && item.status == FidelityStatus::Exact
        }));

        let document = crate::scene::migrate_to_v5(imported.document).unwrap();
        let scene = crate::image::to_svg(
            &document,
            std::path::Path::new("object-bbox-clip.svg"),
            None,
        )
        .unwrap();
        let png = crate::render::scene_to_png(&scene, 1.0).unwrap();
        let pixels = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(pixels.get_pixel(20, 25).0, [255, 255, 255, 255]);
        assert_eq!(pixels.get_pixel(40, 25).0, [255, 0, 0, 255]);
        assert_eq!(pixels.get_pixel(75, 25).0, [255, 255, 255, 255]);
    }

    #[test]
    fn imports_bounded_rectangular_alpha_and_linear_luminance_masks() {
        let imported = import_svg(
            br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><defs>
              <mask id="alpha-mask" maskUnits="userSpaceOnUse" maskContentUnits="userSpaceOnUse" x="0" y="0" width="10" height="10" mask-type="alpha"><rect x="0" y="0" width="10" height="10" fill="#808080"/></mask>
              <mask id="luma-mask" maskUnits="userSpaceOnUse" maskContentUnits="userSpaceOnUse" x="10" y="0" width="10" height="10" mask-type="luminance"><rect x="10" y="0" width="10" height="10" fill="#808080"/></mask>
            </defs><rect id="alpha-target" x="0" y="0" width="10" height="10" fill="#ff0000" mask="url(#alpha-mask)"/><rect id="luma-target" x="10" y="0" width="10" height="10" fill="#ff0000" mask="url(#luma-mask)"/></svg>"##,
            "mask.svg",
            ImportLimits::default(),
        )
        .unwrap();
        assert_eq!(imported.document["version"], 12);
        crate::scene::validate(&imported.document).unwrap();
        assert!(imported.report.items.iter().any(|item| {
            item.source_id == "alpha-target"
                && item.feature == "mask"
                && item.status == FidelityStatus::Exact
        }));
        assert!(imported.report.items.iter().any(|item| {
            item.source_id == "luma-mask"
                && item.feature == "mask_definition"
                && item.status == FidelityStatus::Exact
        }));
        let png = crate::composite::png(
            &imported.document,
            std::path::Path::new("mask.svg"),
            None,
            1.0,
        )
        .unwrap();
        let pixels = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(pixels.get_pixel(5, 5).0, [255, 0, 0, 255]);
        // Gamma-correct sRGB gray #808080 has linear-light luminance about .216.
        assert!(pixels.get_pixel(15, 5)[1] >= 198 && pixels.get_pixel(15, 5)[1] <= 201);
    }

    #[test]
    fn editable_text_keeps_inherited_typography_and_reports_run_approximations() {
        let imported = import_svg(
            br##"<svg xmlns="http://www.w3.org/2000/svg"><g font-family="Georgia" font-size="18" font-weight="bold" fill="#123456"><text id="caption" x="8 20" y="24" text-anchor="middle">  Hello <tspan fill="#ff0000">world</tspan>  </text></g></svg>"##,
            "text.svg",
            ImportLimits::default(),
        )
        .unwrap();
        let text =
            &imported.document["pages"][0]["layers"][0]["nodes"][0]["children"][0]["children"][0];
        assert_eq!(text["kind"], "text");
        assert_eq!(text["content"], "Hello world");
        assert_eq!(text["font_family"], "Georgia");
        assert_eq!(text["font_size"], 18.0);
        assert_eq!(text["font_weight"], 700);
        assert_eq!(text["align"], "center");
        assert_eq!(text["style"]["fill"]["fallback"], "#123456");
        assert!(imported.report.items.iter().any(|item| {
            item.source_id == "caption" && item.status == FidelityStatus::Approximated
        }));
        scene::validate(&imported.document).unwrap();
    }

    #[test]
    fn applies_static_css_cascade_to_shapes_and_text() {
        let imported = import_svg(
            br##"<svg xmlns="http://www.w3.org/2000/svg"><style>
                rect { fill: #111111; stroke: #eeeeee; stroke-width: 2 }
                .accent { fill: #222222 }
                #hero { fill: #333333 }
                text.caption { font-family: Georgia; font-size: 18px; font-weight: bold; text-anchor: middle }
            </style>
            <rect id="hero" class="accent" width="10" height="8"/>
            <rect id="inline" class="accent" width="10" height="8" style="fill:#abcdef"/>
            <text id="caption" class="caption" x="20" y="30">Hello</text></svg>"##,
            "css.svg",
            ImportLimits::default(),
        )
        .unwrap();
        let nodes = &imported.document["pages"][0]["layers"][0]["nodes"][0]["children"];
        assert_eq!(nodes[0]["style"]["fill"]["fallback"], "#333333");
        assert_eq!(nodes[0]["style"]["stroke"]["fallback"], "#eeeeee");
        assert_eq!(nodes[0]["style"]["stroke_width"]["fallback"], 2.0);
        assert_eq!(nodes[1]["style"]["fill"]["fallback"], "#abcdef");
        assert_eq!(nodes[2]["font_family"], "Georgia");
        assert_eq!(nodes[2]["font_size"], 18.0);
        assert_eq!(nodes[2]["font_weight"], 700);
        assert_eq!(nodes[2]["align"], "center");
        assert!(imported
            .report
            .items
            .iter()
            .any(|item| { item.feature == "stylesheet" && item.status == FidelityStatus::Exact }));
        scene::validate(&imported.document).unwrap();
    }

    #[test]
    fn fill_and_stroke_opacity_are_applied_to_solid_and_gradient_paints() {
        let imported = import_svg(
            br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10">
              <defs><linearGradient id="g"><stop offset="0" stop-color="#ff0000"/><stop offset="1" stop-color="#0000ff"/></linearGradient></defs>
              <rect id="solid" width="5" height="10" fill="#00ff00" fill-opacity="0.5" stroke="#000000" stroke-opacity="0.25"/>
              <rect id="gradient" x="10" width="10" height="10" fill="url(#g)" fill-opacity="0.5"/>
            </svg>"##,
            "paint-opacity.svg",
            ImportLimits::default(),
        )
        .unwrap();
        let nodes = &imported.document["pages"][0]["layers"][0]["nodes"][0]["children"];
        assert_eq!(nodes[0]["style"]["fill"]["fallback"], "#00ff0080");
        assert_eq!(nodes[0]["style"]["stroke"]["fallback"], "#00000040");
        assert_eq!(
            nodes[1]["style"]["fill"]["fallback"]["stops"][0]["color"],
            "#ff000080"
        );
        assert_eq!(
            nodes[1]["style"]["fill"]["fallback"]["stops"][1]["color"],
            "#0000ff80"
        );
        assert!(!imported.report.items.iter().any(|item| {
            item.feature == "presentation:fill-opacity"
                || item.feature == "presentation:stroke-opacity"
        }));
        scene::validate(&imported.document).unwrap();
    }

    #[test]
    fn imports_svg_stroke_miterlimit_from_attributes_css_and_inheritance() {
        let imported = import_svg(
            br##"<svg xmlns="http://www.w3.org/2000/svg"><style>.css{stroke-miterlimit:8}</style><g stroke-miterlimit="6"><path id="inherited" d="M0 0L10 0"/><path id="css" class="css" d="M0 1L10 1"/><path id="inline" style="stroke-miterlimit:12" d="M0 2L10 2"/></g></svg>"##,
            "miterlimit.svg",
            ImportLimits::default(),
        )
        .unwrap();
        let group = &imported.document["pages"][0]["layers"][0]["nodes"][0]["children"][0];
        let children = group["children"].as_array().unwrap();
        assert_eq!(children[0]["style"]["stroke_miterlimit"]["fallback"], 6.0);
        assert_eq!(children[1]["style"]["stroke_miterlimit"]["fallback"], 8.0);
        assert_eq!(children[2]["style"]["stroke_miterlimit"]["fallback"], 12.0);
        scene::validate(&imported.document).unwrap();
    }

    #[test]
    fn rejects_out_of_range_svg_stroke_miterlimit() {
        let error = import_svg(
            br#"<svg><path d="M0 0L10 0" stroke-miterlimit="0.5"/></svg>"#,
            "invalid-miterlimit.svg",
            ImportLimits::default(),
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("stroke-miterlimit must be in 1..=1000"));
    }

    #[test]
    fn group_opacity_is_not_inherited_by_child_shapes() {
        let imported = import_svg(
            br##"<svg xmlns="http://www.w3.org/2000/svg"><g id="fade" opacity="0.5"><rect id="box" width="10" height="10"/></g></svg>"##,
            "opacity.svg",
            ImportLimits::default(),
        )
        .unwrap();
        let wrapper = &imported.document["pages"][0]["layers"][0]["nodes"][0];
        let group = &wrapper["children"][0];
        let rect = &group["children"][0];
        assert_eq!(group["opacity"], 0.5);
        assert_eq!(rect["opacity"], 1.0);
        scene::validate(&imported.document).unwrap();
    }

    #[test]
    fn display_and_visibility_are_applied_without_losing_hidden_nodes() {
        let imported = import_svg(
            br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
              <g id="hidden-group" visibility="hidden"><rect id="inherited" width="2" height="2"/><rect id="shown" x="3" width="2" height="2" visibility="visible"/></g>
              <rect id="display-none" x="6" width="2" height="2" style="display:none"/>
            </svg>"##,
            "visibility.svg",
            ImportLimits::default(),
        )
        .unwrap();
        let root = &imported.document["pages"][0]["layers"][0]["nodes"][0];
        let group = &root["children"][0];
        assert_eq!(group["visible"], true);
        assert_eq!(group["children"][0]["visible"], false);
        assert_eq!(group["children"][1]["visible"], true);
        assert_eq!(root["children"][1]["visible"], false);
        assert!(!imported.report.items.iter().any(|item| {
            item.feature == "presentation:display" || item.feature == "presentation:visibility"
        }));
        scene::validate(&imported.document).unwrap();
    }

    #[test]
    fn reports_unsupported_inline_and_presentation_properties() {
        let imported = import_svg(
            br##"<svg xmlns="http://www.w3.org/2000/svg"><rect id="box" width="10" height="10" fill-rule="evenodd" style="stroke-dasharray:2 2"/></svg>"##,
            "unsupported-style.svg",
            ImportLimits::default(),
        )
        .unwrap();
        assert!(imported.report.items.iter().any(|item| {
            item.source_id == "box"
                && item.feature == "inline_style:stroke-dasharray"
                && item.status == FidelityStatus::Dropped
        }));
        assert!(imported.report.items.iter().any(|item| {
            item.source_id == "box"
                && item.feature == "presentation:fill-rule"
                && item.status == FidelityStatus::Dropped
        }));
    }
}
