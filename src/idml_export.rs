//! IDML export (v0.14.0 task 14).
//!
//! Writes a version 9 document as an InDesign Markup package. The mapping
//! contract is `docs/idml-mapping-v1.md`: every supported feature is written as
//! the IDML element it was read from, carried `extension.idml` blocks return
//! where their owner still exists (and are listed as `carried-dropped` when it
//! does not), and a feature without an IDML equivalent fails the export unless
//! `--allow-approximation` is given. Output is deterministic: IDs are reused or
//! derived in document order, entries are sorted and carry a fixed timestamp.
use crate::idml::{check_entry_name, parse, round, swatch_hex, MAX_ARCHIVE_BYTES, MIMETYPE};
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Cursor, Write};

const PACKAGING: &str = "http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging";
const XML_HEAD: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n";
const MAX_PAGES: usize = 10_000;
const MAX_DEPTH: usize = 64;
const KAPPA: f64 = 0.552_284_749_8;

pub struct Exported {
    pub bytes: Vec<u8>,
    pub report: Value,
}

// ------------------------------------------------------------ helpers

fn esc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => {}
            c => out.push(c),
        }
    }
    out
}

fn fmt(v: f64) -> String {
    let v = round(v);
    if v == v.trunc() && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

fn number(v: &Value, key: &str) -> Option<f64> {
    v.get(key).and_then(Value::as_f64).filter(|n| n.is_finite())
}

fn paint(v: Option<&Value>) -> Option<&str> {
    let v = v?;
    v.as_str()
        .or_else(|| v.get("fallback").and_then(Value::as_str))
        .filter(|s| !s.is_empty() && *s != "none")
}

#[derive(Default)]
struct Report {
    items: Vec<Value>,
    counts: [usize; 5],
}

impl Report {
    fn add(&mut self, status: &str, part: &str, element: &str, reference: &str, message: &str) {
        let slot = match status {
            "supported" => 0,
            "approximated" => 1,
            "carried" => 2,
            "unsupported" => 3,
            _ => 4,
        };
        self.counts[slot] += 1;
        if status != "supported" && self.items.len() < 5000 {
            self.items.push(json!({
                "status": status, "part": part, "element": element,
                "ref": reference, "message": message,
            }));
        }
    }

    fn gating(&self) -> Vec<&Value> {
        self.items
            .iter()
            .filter(|i| matches!(i["status"].as_str(), Some("approximated" | "unsupported")))
            .collect()
    }
}

struct Carried {
    part: String,
    owner: String,
    element: String,
    xml: String,
    used: bool,
}

#[derive(Clone, Copy)]
struct Anchor {
    l: (f64, f64),
    p: (f64, f64),
    r: (f64, f64),
}

fn straight(p: (f64, f64)) -> Anchor {
    Anchor { l: p, p, r: p }
}

/// `M`/`L`/`C`/`Z` path data (absolute, as written by IDML import).
fn parse_d(d: &str) -> Option<Vec<(Vec<Anchor>, bool)>> {
    let tokens: Vec<&str> = d.split_whitespace().collect();
    let mut subs: Vec<(Vec<Anchor>, bool)> = Vec::new();
    let mut cur: Vec<Anchor> = Vec::new();
    let mut i = 0;
    let pt = |i: usize| -> Option<(f64, f64)> {
        Some((
            tokens
                .get(i)?
                .parse()
                .ok()
                .filter(|v: &f64| v.is_finite())?,
            tokens
                .get(i + 1)?
                .parse()
                .ok()
                .filter(|v: &f64| v.is_finite())?,
        ))
    };
    let mut last_curve = false;
    while i < tokens.len() {
        match tokens[i] {
            "M" => {
                if !cur.is_empty() {
                    subs.push((std::mem::take(&mut cur), true));
                }
                cur.push(straight(pt(i + 1)?));
                last_curve = false;
                i += 3;
            }
            "L" => {
                let p = pt(i + 1)?;
                cur.last()?;
                cur.push(straight(p));
                last_curve = false;
                i += 3;
            }
            "C" => {
                let (a, b, c) = (pt(i + 1)?, pt(i + 3)?, pt(i + 5)?);
                cur.last_mut()?.r = a;
                cur.push(Anchor { l: b, p: c, r: c });
                last_curve = true;
                i += 7;
            }
            "Z" => {
                if cur.len() > 1 && last_curve {
                    let tail = *cur.last()?;
                    if tail.p == cur[0].p {
                        cur[0].l = tail.l;
                        cur.pop();
                    }
                } else if cur.len() > 1 && cur.last()?.p == cur[0].p {
                    cur.pop();
                }
                subs.push((std::mem::take(&mut cur), false));
                i += 1;
            }
            _ => return None,
        }
    }
    if !cur.is_empty() {
        subs.push((cur, true));
    }
    (!subs.is_empty() && subs.iter().all(|(a, _)| !a.is_empty())).then_some(subs)
}

fn geometry(subs: &[(Vec<Anchor>, bool)]) -> String {
    let mut out = String::from("<Properties><PathGeometry>");
    for (anchors, open) in subs {
        out.push_str(&format!(
            "<GeometryPathType PathOpen=\"{open}\"><PathPointArray>"
        ));
        for a in anchors {
            out.push_str(&format!(
                "<PathPointType Anchor=\"{} {}\" LeftDirection=\"{} {}\" RightDirection=\"{} {}\"/>",
                fmt(a.p.0),
                fmt(a.p.1),
                fmt(a.l.0),
                fmt(a.l.1),
                fmt(a.r.0),
                fmt(a.r.1)
            ));
        }
        out.push_str("</PathPointArray></GeometryPathType>");
    }
    out.push_str("</PathGeometry></Properties>");
    out
}

fn rect_path(w: f64, h: f64) -> Vec<(Vec<Anchor>, bool)> {
    vec![(
        vec![
            straight((0.0, 0.0)),
            straight((0.0, h)),
            straight((w, h)),
            straight((w, 0.0)),
        ],
        false,
    )]
}

fn oval_path(rx: f64, ry: f64) -> Vec<(Vec<Anchor>, bool)> {
    let (kx, ky) = (rx * KAPPA, ry * KAPPA);
    let (w, h) = (rx * 2.0, ry * 2.0);
    vec![(
        vec![
            Anchor {
                l: (rx - kx, 0.0),
                p: (rx, 0.0),
                r: (rx + kx, 0.0),
            },
            Anchor {
                l: (w, ry - ky),
                p: (w, ry),
                r: (w, ry + ky),
            },
            Anchor {
                l: (rx + kx, h),
                p: (rx, h),
                r: (rx - kx, h),
            },
            Anchor {
                l: (0.0, ry + ky),
                p: (0.0, ry),
                r: (0.0, ry - ky),
            },
        ],
        false,
    )]
}

fn justification(align: &str) -> Option<&'static str> {
    Some(match align {
        "left" => "LeftAlign",
        "center" => "CenterAlign",
        "right" => "RightAlign",
        "justify" => "FullyJustified",
        "justify-last-left" => "LeftJustified",
        "justify-last-center" => "CenterJustified",
        "justify-last-right" => "RightJustified",
        "towards-spine" => "ToBindingSide",
        "away-from-spine" => "AwayFromBindingSide",
        _ => return None,
    })
}

fn font_style(weight: Option<i64>, italic: bool) -> Option<String> {
    let name = match weight {
        None | Some(400) => None,
        Some(100) => Some("Thin"),
        Some(200) => Some("ExtraLight"),
        Some(300) => Some("Light"),
        Some(500) => Some("Medium"),
        Some(600) => Some("SemiBold"),
        Some(700) => Some("Bold"),
        Some(800) => Some("ExtraBold"),
        Some(900) => Some("Black"),
        Some(_) => None,
    };
    match (name, italic) {
        (None, false) => weight.map(|_| "Regular".to_string()),
        (None, true) => Some("Italic".into()),
        (Some(n), false) => Some(n.into()),
        (Some(n), true) => Some(format!("{n} Italic")),
    }
}

// ------------------------------------------------------------ exporter

struct Ex<'a> {
    doc: &'a Value,
    rep: Report,
    ids: HashSet<String>,
    frames: HashMap<String, String>,
    frame_prev: HashMap<String, String>,
    stories: HashMap<String, String>,
    swatches: BTreeMap<String, String>,
    swatch_xml: Vec<String>,
    layers: Vec<(String, String, bool, bool)>,
    carried: Vec<Carried>,
    by_owner: HashMap<String, Vec<usize>>,
    item_ext: &'a Map<String, Value>,
}

impl<'a> Ex<'a> {
    fn unique(&mut self, base: &str) -> String {
        let mut clean: String = base
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '_' | '-') {
                    c
                } else {
                    '_'
                }
            })
            .take(80)
            .collect();
        if !clean
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic())
        {
            clean.insert(0, 'u');
        }
        let mut id = clean.clone();
        let mut n = 2;
        while !self.ids.insert(id.clone()) {
            id = format!("{clean}-{n}");
            n += 1;
        }
        id
    }

    fn color(&mut self, value: Option<&str>) -> String {
        let Some(hex) = value.filter(|v| v.len() == 7 && v.starts_with('#')) else {
            return "Swatch/None".into();
        };
        let hex = hex.to_ascii_lowercase();
        if let Some(found) = self.swatches.get(&hex) {
            return found.clone();
        }
        let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(0);
        let me = format!("Color/{}", &hex[1..]);
        self.swatch_xml.push(format!(
            "<Color Self=\"{me}\" Model=\"Process\" Space=\"RGB\" ColorValue=\"{} {} {}\" Name=\"{}\"/>",
            channel(1),
            channel(3),
            channel(5),
            esc(&hex[1..])
        ));
        self.swatches.insert(hex, me.clone());
        me
    }

    /// Carried XML addressed to any of `owners`, marked as returned.
    fn take(&mut self, owners: &[&str], part_hint: &str) -> String {
        let mut out = String::new();
        for owner in owners {
            let Some(list) = self.by_owner.get(*owner).cloned() else {
                continue;
            };
            for index in list {
                if self.carried[index].used {
                    continue;
                }
                self.carried[index].used = true;
                out.push_str(&self.carried[index].xml);
                let c = &self.carried[index];
                let (element, owner) = (c.element.clone(), c.owner.clone());
                self.rep.add(
                    "carried",
                    part_hint,
                    &element,
                    &owner,
                    "carried block returned unchanged",
                );
            }
        }
        out
    }

    fn layer(&mut self, layer: &Value) -> String {
        let id = layer
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("layer")
            .to_string();
        if let Some(found) = self.layers.iter().find(|l| l.0 == id) {
            return found.0.clone();
        }
        let name = layer
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(&id)
            .to_string();
        self.layers.push((
            id.clone(),
            name,
            layer.get("visible").and_then(Value::as_bool) != Some(false),
            layer.get("locked").and_then(Value::as_bool) == Some(true),
        ));
        id
    }

    fn style_attrs(&mut self, node: &Value, part: &str, name: &str) -> String {
        let mut attrs = String::new();
        let style = node.get("style");
        let fill = paint(style.and_then(|s| s.get("fill"))).map(str::to_string);
        let stroke = paint(style.and_then(|s| s.get("stroke"))).map(str::to_string);
        let weight = style
            .and_then(|s| number(s, "stroke_width"))
            .or_else(|| {
                style
                    .and_then(|s| s.get("stroke_width"))
                    .and_then(|w| number(w, "fallback"))
            })
            .unwrap_or(0.0);
        let f = self.color(fill.as_deref());
        let s = self.color(stroke.as_deref());
        attrs.push_str(&format!(" FillColor=\"{f}\" StrokeColor=\"{s}\""));
        attrs.push_str(&format!(" StrokeWeight=\"{}\"", fmt(weight)));
        let _ = (part, name);
        attrs
    }

    fn flag_extras(&mut self, node: &Value, part: &str, id: &str) {
        const KNOWN: &[&str] = &[
            "kind",
            "id",
            "name",
            "semantic",
            "x",
            "y",
            "width",
            "height",
            "radius_x",
            "radius_y",
            "cx",
            "cy",
            "x1",
            "y1",
            "x2",
            "y2",
            "style",
            "d",
            "closed",
            "story",
            "next",
            "columns",
            "gutter",
            "inset",
            "children",
            "transform",
            "fit",
            "visible",
            "locked",
        ];
        let Some(obj) = node.as_object() else { return };
        let extra: Vec<&str> = obj
            .keys()
            .map(String::as_str)
            .filter(|k| !KNOWN.contains(k))
            .collect();
        if !extra.is_empty() {
            self.rep.add(
                "approximated",
                part,
                "PageItem",
                id,
                &format!(
                    "properties without an IDML mapping were not exported: {}",
                    extra.join(", ")
                ),
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn item(
        &mut self,
        node: &Value,
        origin: (f64, f64),
        layer: &str,
        part: &str,
        depth: usize,
        out: &mut String,
    ) -> Result<()> {
        ensure!(
            depth <= MAX_DEPTH,
            "[limit-exceeded] group nesting deeper than {MAX_DEPTH} levels"
        );
        let kind = node.get("kind").and_then(Value::as_str).unwrap_or("");
        let pen_id = node.get("id").and_then(Value::as_str).unwrap_or("item");
        let id = match (kind, self.frames.get(pen_id)) {
            ("text-frame", Some(id)) => id.clone(),
            _ => self.unique(pen_id),
        };
        if kind != "group" {
            if let Some(t) = node.get("transform").and_then(Value::as_array) {
                let identity = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
                if t.len() != 6
                    || t.iter()
                        .zip(identity)
                        .any(|(a, b)| a.as_f64().is_none_or(|a| (a - b).abs() > 1e-9))
                {
                    self.rep.add(
                        "approximated",
                        part,
                        "PageItem",
                        pen_id,
                        "node transform was not exported; the item is written untransformed",
                    );
                }
            }
        }
        self.flag_extras(node, part, pen_id);
        let original = self
            .item_ext
            .get(pen_id)
            .and_then(|v| v.get("self"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let mut owners: Vec<&str> = vec![id.as_str()];
        if let Some(o) = original.as_deref() {
            owners.push(o);
        }
        let carried = self.take(&owners, part);
        let object_style = self
            .item_ext
            .get(pen_id)
            .and_then(|v| v.get("applied_object_style"))
            .and_then(Value::as_str)
            .map(|s| format!(" AppliedObjectStyle=\"{}\"", esc(s)))
            .unwrap_or_default();
        let (ox, oy) = origin;
        let x = number(node, "x").unwrap_or(0.0);
        let y = number(node, "y").unwrap_or(0.0);
        let w = number(node, "width").unwrap_or(0.0).max(0.0);
        let h = number(node, "height").unwrap_or(0.0).max(0.0);
        let matrix = |tx: f64, ty: f64| format!("1 0 0 1 {} {}", fmt(tx), fmt(ty));
        let head = |tag: &str, extra: &str| {
            format!(
                "<{tag} Self=\"{id}\" ItemLayer=\"{}\"{extra}{object_style}",
                esc(layer)
            )
        };
        match kind {
            "rect" => {
                let style = self.style_attrs(node, part, pen_id);
                let (rx, ry) = (
                    number(node, "radius_x").unwrap_or(0.0),
                    number(node, "radius_y").unwrap_or(0.0),
                );
                let corners = if rx > 0.0 || ry > 0.0 {
                    let r = fmt(rx.max(ry));
                    format!(
                        " CornerOption=\"RoundedCorner\" TopLeftCornerRadius=\"{r}\" TopRightCornerRadius=\"{r}\" BottomLeftCornerRadius=\"{r}\" BottomRightCornerRadius=\"{r}\""
                    )
                } else {
                    String::new()
                };
                out.push_str(&format!(
                    "{} ItemTransform=\"{}\"{style}{corners}>{}{carried}</Rectangle>",
                    head("Rectangle", ""),
                    matrix(ox + x, oy + y),
                    geometry(&rect_path(w, h)),
                ));
            }
            "ellipse" => {
                let style = self.style_attrs(node, part, pen_id);
                let (cx, cy) = (
                    number(node, "cx").unwrap_or(0.0),
                    number(node, "cy").unwrap_or(0.0),
                );
                let (rx, ry) = (
                    number(node, "radius_x").unwrap_or(0.0).max(0.0),
                    number(node, "radius_y").unwrap_or(0.0).max(0.0),
                );
                out.push_str(&format!(
                    "{} ItemTransform=\"{}\"{style}>{}{carried}</Oval>",
                    head("Oval", ""),
                    matrix(ox + cx - rx, oy + cy - ry),
                    geometry(&oval_path(rx, ry)),
                ));
            }
            "line" => {
                let style = self.style_attrs(node, part, pen_id);
                let p = |k: &str| number(node, k).unwrap_or(0.0);
                let subs = vec![(
                    vec![straight((p("x1"), p("y1"))), straight((p("x2"), p("y2")))],
                    true,
                )];
                out.push_str(&format!(
                    "{} ItemTransform=\"{}\"{style}>{}{carried}</GraphicLine>",
                    head("GraphicLine", ""),
                    matrix(ox, oy),
                    geometry(&subs),
                ));
            }
            "path" => {
                let d = node.get("d").and_then(Value::as_str).unwrap_or("");
                let Some(subs) = parse_d(d) else {
                    self.rep.add(
                        "unsupported",
                        part,
                        "Polygon",
                        pen_id,
                        "path data could not be read; the path was not exported",
                    );
                    return Ok(());
                };
                let style = self.style_attrs(node, part, pen_id);
                let open = subs.iter().all(|(_, open)| *open);
                let tag = if open { "GraphicLine" } else { "Polygon" };
                out.push_str(&format!(
                    "{} ItemTransform=\"{}\"{style}>{}{carried}</{tag}>",
                    head(tag, ""),
                    matrix(ox, oy),
                    geometry(&subs),
                ));
            }
            "text-frame" => {
                let story = node
                    .get("story")
                    .and_then(Value::as_str)
                    .and_then(|s| self.stories.get(s))
                    .map(|s| format!(" ParentStory=\"{s}\""))
                    .unwrap_or_default();
                let next = node
                    .get("next")
                    .and_then(Value::as_str)
                    .and_then(|n| self.frames.get(n))
                    .map(|n| format!(" NextTextFrame=\"{n}\""))
                    .unwrap_or_else(|| " NextTextFrame=\"n\"".into());
                let prev = self
                    .frame_prev
                    .get(pen_id)
                    .and_then(|p| self.frames.get(p))
                    .map(|p| format!(" PreviousTextFrame=\"{p}\""))
                    .unwrap_or_else(|| " PreviousTextFrame=\"n\"".into());
                let columns = node
                    .get("columns")
                    .and_then(Value::as_u64)
                    .filter(|c| (1..=40).contains(c))
                    .unwrap_or(1);
                let gutter = number(node, "gutter").unwrap_or(12.0);
                let mut inset = String::new();
                if let Some(a) = node.get("inset").and_then(Value::as_array) {
                    let v: Vec<f64> = a.iter().filter_map(Value::as_f64).collect();
                    if v.len() == 4 {
                        // Pentool top,right,bottom,left -> IDML top,left,bottom,right.
                        inset = format!(
                            "<Properties><InsetSpacing type=\"list\">{}</InsetSpacing></Properties>",
                            [v[0], v[3], v[2], v[1]]
                                .iter()
                                .map(|n| format!("<ListItem type=\"unit\">{}</ListItem>", fmt(*n)))
                                .collect::<String>()
                        );
                    }
                }
                out.push_str(&format!(
                    "{}{story}{prev}{next} ItemTransform=\"{}\">{}<TextFramePreference TextColumnCount=\"{columns}\" TextColumnGutter=\"{}\">{inset}</TextFramePreference>{carried}</TextFrame>",
                    head("TextFrame", ""),
                    matrix(ox + x, oy + y),
                    geometry(&rect_path(w, h)),
                    fmt(gutter),
                ));
            }
            "graphic-frame" => {
                let link = self
                    .item_ext
                    .get(pen_id)
                    .and_then(|v| v.get("link"))
                    .and_then(Value::as_str)
                    .filter(|l| !l.is_empty())
                    .map(str::to_string);
                let image = match &link {
                    Some(l) => {
                        let image_id = self.unique(&format!("{pen_id}-image"));
                        let link_id = self.unique(&format!("{pen_id}-link"));
                        format!(
                            "<Image Self=\"{image_id}\"><Link Self=\"{link_id}\" LinkResourceURI=\"{}\"/></Image>",
                            esc(l)
                        )
                    }
                    None => {
                        self.rep.add(
                            "approximated",
                            part,
                            "Rectangle",
                            pen_id,
                            "graphic frame has no linked file; exported as an empty frame",
                        );
                        String::new()
                    }
                };
                out.push_str(&format!(
                    "{} ItemTransform=\"{}\" FillColor=\"Swatch/None\" StrokeColor=\"Swatch/None\">{}{image}{carried}</Rectangle>",
                    head("Rectangle", ""),
                    matrix(ox + x, oy + y),
                    geometry(&rect_path(w, h)),
                ));
            }
            "group" => {
                let t: Vec<f64> = node
                    .get("transform")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_f64).collect())
                    .filter(|v: &Vec<f64>| v.len() == 6)
                    .unwrap_or_else(|| vec![1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
                // Children keep their group-local coordinates; the translation
                // places the group on its page.
                let mut inner = String::new();
                if let Some(children) = node.get("children").and_then(Value::as_array) {
                    for child in children {
                        self.item(child, (0.0, 0.0), layer, part, depth + 1, &mut inner)?;
                    }
                }
                out.push_str(&format!(
                    "{} ItemTransform=\"{} {} {} {} {} {}\">{inner}{carried}</Group>",
                    head("Group", ""),
                    fmt(t[0]),
                    fmt(t[1]),
                    fmt(t[2]),
                    fmt(t[3]),
                    fmt(t[4] + ox),
                    fmt(t[5] + oy),
                ));
            }
            other => {
                self.rep.add(
                    "unsupported",
                    part,
                    other,
                    pen_id,
                    &format!("node kind `{other}` has no IDML mapping yet; it was not exported"),
                );
                return Ok(());
            }
        }
        self.rep.add("supported", part, kind, pen_id, "");
        Ok(())
    }

    fn story(&mut self, story: &Value, export_id: &str) -> String {
        let part = format!("Stories/Story_{export_id}.xml");
        let pen_id = story.get("id").and_then(Value::as_str).unwrap_or("story");
        let mut body = String::new();
        let paragraphs = story
            .get("paragraphs")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let last = paragraphs.len().saturating_sub(1);
        for (index, paragraph) in paragraphs.iter().enumerate() {
            let pstyle = paragraph
                .get("style")
                .and_then(Value::as_str)
                .map(|s| format!("ParagraphStyle/{}", esc(s)))
                .unwrap_or_else(|| "ParagraphStyle/$ID/NormalParagraphStyle".into());
            body.push_str(&format!(
                "<ParagraphStyleRange AppliedParagraphStyle=\"{pstyle}\">"
            ));
            let runs = paragraph
                .get("runs")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let mut ranges = 0;
            for run in &runs {
                let cstyle = run
                    .get("style")
                    .and_then(Value::as_str)
                    .map(|s| format!("CharacterStyle/{}", esc(s)))
                    .unwrap_or_else(|| "CharacterStyle/$ID/[No character style]".into());
                let mut inner = String::new();
                if let Some(text) = run.get("text").and_then(Value::as_str) {
                    inner.push_str(&format!(
                        "<Content>{}</Content>",
                        esc(&text.replace('\n', "\u{2028}"))
                    ));
                }
                if let Some(special) = run.get("special").and_then(Value::as_str) {
                    let content = |c: &str| format!("<Content>{c}</Content>");
                    match special {
                        "tab" => inner.push_str(&content("\t")),
                        "line-break" => inner.push_str(&content("\u{2028}")),
                        "em-space" => inner.push_str(&content("\u{2003}")),
                        "en-space" => inner.push_str(&content("\u{2002}")),
                        "thin-space" => inner.push_str(&content("\u{2009}")),
                        "hair-space" => inner.push_str(&content("\u{200A}")),
                        "nonbreaking-hyphen" => inner.push_str(&content("\u{2011}")),
                        "page-number" => inner.push_str("<?ACE 18?>"),
                        "next-page-number" => inner.push_str("<?ACE 19?>"),
                        "right-indent-tab" => inner.push_str("<?ACE 7?>"),
                        other => self.rep.add(
                            "approximated",
                            &part,
                            "Content",
                            pen_id,
                            &format!("special character `{other}` has no IDML mapping yet and was not exported"),
                        ),
                    }
                }
                if let Some(obj) = run.as_object() {
                    let extra: Vec<&str> = obj
                        .keys()
                        .map(String::as_str)
                        .filter(|k| !matches!(*k, "text" | "special" | "style"))
                        .collect();
                    if !extra.is_empty() {
                        self.rep.add(
                            "approximated",
                            &part,
                            "CharacterStyleRange",
                            pen_id,
                            &format!(
                                "run fields without an IDML mapping were not exported: {}",
                                extra.join(", ")
                            ),
                        );
                    }
                }
                if inner.is_empty() {
                    continue;
                }
                body.push_str(&format!(
                    "<CharacterStyleRange AppliedCharacterStyle=\"{cstyle}\">{inner}</CharacterStyleRange>"
                ));
                ranges += 1;
            }
            if ranges == 0 {
                body.push_str("<CharacterStyleRange AppliedCharacterStyle=\"CharacterStyle/$ID/[No character style]\"/>");
            }
            if index != last {
                // Paragraph break: IDML keeps it inside the last range.
                if body.ends_with("</CharacterStyleRange>") {
                    let cut = body.len() - "</CharacterStyleRange>".len();
                    body.truncate(cut);
                    body.push_str("<Br/></CharacterStyleRange>");
                } else if body.ends_with("[No character style]\"/>") {
                    let cut = body.len() - 2;
                    body.truncate(cut);
                    body.push_str("><Br/></CharacterStyleRange>");
                }
            }
            body.push_str("</ParagraphStyleRange>");
        }
        let carried = self.take(&[export_id, pen_id], &part);
        self.rep.add("supported", &part, "Story", pen_id, "");
        format!(
            "{XML_HEAD}<idPkg:Story xmlns:idPkg=\"{PACKAGING}\" DOMVersion=\"8.0\"><Story Self=\"{export_id}\" AppliedTOCStyle=\"n\" TrackChanges=\"false\" StoryTitle=\"\" AppliedNamedGrid=\"n\">{body}{carried}</Story></idPkg:Story>\n"
        )
    }

    fn style_element(
        &mut self,
        kind: &str,
        name: &str,
        style: &Value,
        ext: Option<&Value>,
    ) -> String {
        let (tag, prefix) = if kind == "paragraph" {
            ("ParagraphStyle", "ParagraphStyle")
        } else {
            ("CharacterStyle", "CharacterStyle")
        };
        let mut attrs = String::new();
        let mut props = String::new();
        let mut dropped: Vec<String> = Vec::new();
        let mut weight = None;
        let mut italic = false;
        if let Some(obj) = style.as_object() {
            for (key, value) in obj {
                let n = value.as_f64();
                match key.as_str() {
                    "font" => {
                        if let Some(f) = value.as_str() {
                            props.push_str(&format!(
                                "<AppliedFont type=\"string\">{}</AppliedFont>",
                                esc(f)
                            ));
                        }
                    }
                    "weight" => weight = value.as_i64(),
                    "italic" => italic = value.as_bool() == Some(true),
                    "size" if n.is_some() => {
                        attrs.push_str(&format!(" PointSize=\"{}\"", fmt(n.unwrap_or(0.0))))
                    }
                    "leading" if n.is_some() => {
                        attrs.push_str(&format!(" Leading=\"{}\"", fmt(n.unwrap_or(0.0))))
                    }
                    "tracking" if n.is_some() && kind == "character" => {
                        attrs.push_str(&format!(" Tracking=\"{}\"", fmt(n.unwrap_or(0.0))))
                    }
                    "fill" if kind == "character" => {
                        let c = self.color(paint(Some(value)));
                        attrs.push_str(&format!(" FillColor=\"{c}\""));
                    }
                    "align" if kind == "paragraph" => {
                        match value.as_str().and_then(justification) {
                            Some(j) => attrs.push_str(&format!(" Justification=\"{j}\"")),
                            None => dropped.push(key.clone()),
                        }
                    }
                    "left_indent" | "right_indent" | "first_indent" | "last_indent"
                    | "space_before" | "space_after"
                        if kind == "paragraph" && n.is_some() =>
                    {
                        let attr = match key.as_str() {
                            "left_indent" => "LeftIndent",
                            "right_indent" => "RightIndent",
                            "first_indent" => "FirstLineIndent",
                            "last_indent" => "LastLineIndent",
                            "space_before" => "SpaceBefore",
                            _ => "SpaceAfter",
                        };
                        attrs.push_str(&format!(" {attr}=\"{}\"", fmt(n.unwrap_or(0.0))));
                    }
                    "hyphenate" if kind == "paragraph" => {
                        if let Some(b) = value.as_bool() {
                            attrs.push_str(&format!(" Hyphenation=\"{b}\""));
                        }
                    }
                    "based_on" => {
                        if let Some(b) = value.as_str() {
                            props.push_str(&format!(
                                "<BasedOn type=\"object\">{prefix}/{}</BasedOn>",
                                esc(b)
                            ));
                        }
                    }
                    "next" if kind == "paragraph" => {
                        if let Some(b) = value.as_str() {
                            attrs.push_str(&format!(" NextStyle=\"{prefix}/{}\"", esc(b)));
                        }
                    }
                    _ => dropped.push(key.clone()),
                }
            }
        }
        if let Some(fs) = font_style(weight, italic) {
            attrs.push_str(&format!(" FontStyle=\"{fs}\""));
        }
        if let Some(extra) = ext.and_then(Value::as_object) {
            for (key, value) in extra {
                let valid = !key.is_empty()
                    && key.chars().all(|c| c.is_ascii_alphanumeric())
                    && !attrs.contains(&format!(" {key}=\""));
                if let (true, Some(text)) = (valid, value.as_str()) {
                    attrs.push_str(&format!(" {key}=\"{}\"", esc(text)));
                }
            }
        }
        if !dropped.is_empty() {
            self.rep.add(
                "approximated",
                "Resources/Styles.xml",
                tag,
                name,
                &format!(
                    "style properties without an IDML mapping were not exported: {}",
                    dropped.join(", ")
                ),
            );
        } else {
            self.rep
                .add("supported", "Resources/Styles.xml", tag, name, "");
        }
        let properties = if props.is_empty() {
            String::new()
        } else {
            format!("<Properties>{props}</Properties>")
        };
        format!(
            "<{tag} Self=\"{prefix}/{n}\" Name=\"{n}\"{attrs}>{properties}</{tag}>",
            n = esc(name)
        )
    }
}

fn section_style(style: &str) -> &'static str {
    match style {
        "upper-roman" => "UpperRoman",
        "lower-roman" => "LowerRoman",
        "upper-alpha" => "UpperLetters",
        "lower-alpha" => "LowerLetters",
        _ => "Arabic",
    }
}

fn page_pref(layout: Option<&Value>) -> String {
    let Some(l) = layout else {
        return String::new();
    };
    let mut attrs = String::new();
    if let Some(c) = l.get("columns").and_then(Value::as_u64) {
        attrs.push_str(&format!(" ColumnCount=\"{c}\""));
    }
    if let Some(g) = number(l, "gutter") {
        attrs.push_str(&format!(" ColumnGutter=\"{}\"", fmt(g)));
    }
    if let Some(m) = l.get("margins").and_then(Value::as_array) {
        let v: Vec<f64> = m.iter().filter_map(Value::as_f64).collect();
        if v.len() == 4 {
            attrs.push_str(&format!(
                " Top=\"{}\" Right=\"{}\" Bottom=\"{}\" Left=\"{}\"",
                fmt(v[0]),
                fmt(v[1]),
                fmt(v[2]),
                fmt(v[3])
            ));
        }
    }
    if attrs.is_empty() {
        String::new()
    } else {
        format!("<MarginPreference{attrs}/>")
    }
}

/// Export a document (any supported version; versions before 9 are migrated in
/// memory first) as an IDML package.
pub fn export(raw: &Value, source: &str, allow_approximation: bool) -> Result<Exported> {
    let doc = if crate::spreads::is_v9(raw) {
        crate::scene::validate(raw)?;
        raw.clone()
    } else {
        crate::spreads::migrate(raw.clone()).context(
            "[unsupported-capability] only scene documents (version 4 and newer) can be exported as IDML; run `pentool migrate` first",
        )?
    };
    let pages = doc["pages"].as_array().cloned().unwrap_or_default();
    ensure!(
        !pages.is_empty() && pages.len() <= MAX_PAGES,
        "[limit-exceeded] the document must have between 1 and {MAX_PAGES} pages to export"
    );
    let empty_map = Map::new();
    let idml_ext = doc.get("extension").and_then(|e| e.get("idml"));
    let item_ext = idml_ext
        .and_then(|e| e.get("items"))
        .and_then(Value::as_object)
        .unwrap_or(&empty_map);
    let mut ex = Ex {
        doc: &doc,
        rep: Report::default(),
        ids: HashSet::new(),
        frames: HashMap::new(),
        frame_prev: HashMap::new(),
        stories: HashMap::new(),
        swatches: BTreeMap::new(),
        swatch_xml: Vec::new(),
        layers: Vec::new(),
        carried: Vec::new(),
        by_owner: HashMap::new(),
        item_ext,
    };
    let layout = ex.doc.get("layout").cloned().unwrap_or(Value::Null);

    // Swatches that came from IDML return verbatim; the rest are minted on use.
    if let Some(sw) = idml_ext
        .and_then(|e| e.get("swatches"))
        .and_then(Value::as_object)
    {
        for (me, meta) in sw {
            let name = meta.get("name").and_then(Value::as_str).unwrap_or(me);
            let space = meta.get("space").and_then(Value::as_str).unwrap_or("RGB");
            let value = meta.get("value").and_then(Value::as_str).unwrap_or("");
            let model = meta
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or("Process");
            if let Some((hex, _)) = swatch_hex(space, value) {
                ex.swatches.entry(hex).or_insert_with(|| me.clone());
            }
            ex.swatch_xml.push(format!(
                "<Color Self=\"{}\" Model=\"{}\" Space=\"{}\" ColorValue=\"{}\" Name=\"{}\"/>",
                esc(me),
                esc(model),
                esc(space),
                esc(value),
                esc(name)
            ));
        }
    }

    // Carried blocks, validated before they can reach the package.
    let mut other_parts: BTreeMap<String, (String, String)> = BTreeMap::new();
    if let Some(list) = idml_ext
        .and_then(|e| e.get("carried"))
        .and_then(Value::as_array)
    {
        for entry in list {
            let part = entry.get("part").and_then(Value::as_str).unwrap_or("");
            let owner = entry.get("owner").and_then(Value::as_str).unwrap_or("");
            let element = entry.get("element").and_then(Value::as_str).unwrap_or("");
            let xml = entry.get("xml").and_then(Value::as_str).unwrap_or("");
            if xml.is_empty() || parse(xml, part).is_err() {
                ex.rep.add(
                    "carried-dropped",
                    part,
                    element,
                    owner,
                    "carried block is not well-formed XML and was not written",
                );
                continue;
            }
            if owner == "Document" {
                let name_ok =
                    !element.is_empty() && element.chars().all(|c| c.is_ascii_alphanumeric());
                let reserved = part == "designmap.xml"
                    || part.starts_with("Spreads/")
                    || part.starts_with("MasterSpreads/")
                    || part.starts_with("Stories/")
                    || matches!(
                        part,
                        "Resources/Graphic.xml" | "Resources/Fonts.xml" | "Resources/Styles.xml"
                    );
                if name_ok
                    && !reserved
                    && check_entry_name(part).is_ok()
                    && part.to_ascii_lowercase().ends_with(".xml")
                {
                    other_parts.insert(part.to_string(), (element.to_string(), xml.to_string()));
                } else {
                    ex.rep.add(
                        "carried-dropped",
                        part,
                        element,
                        owner,
                        "document part has an unsafe or reserved name and was not written",
                    );
                }
                continue;
            }
            let index = ex.carried.len();
            ex.carried.push(Carried {
                part: part.into(),
                owner: owner.into(),
                element: element.into(),
                xml: xml.into(),
                used: false,
            });
            ex.by_owner
                .entry(owner.to_string())
                .or_default()
                .push(index);
        }
    }

    // Reserve IDs: stories first, then text frames, so cross references resolve.
    let story_list = layout
        .get("stories")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for story in &story_list {
        if let Some(id) = story.get("id").and_then(Value::as_str) {
            let eid = ex.unique(id);
            ex.stories.insert(id.to_string(), eid);
        }
    }
    fn scan(nodes: &[Value], ex: &mut Ex) {
        for n in nodes {
            if n.get("kind").and_then(Value::as_str) == Some("text-frame") {
                if let Some(id) = n.get("id").and_then(Value::as_str) {
                    let eid = ex.unique(id);
                    ex.frames.insert(id.to_string(), eid);
                    if let Some(next) = n.get("next").and_then(Value::as_str) {
                        ex.frame_prev.insert(next.to_string(), id.to_string());
                    }
                }
            }
            if let Some(children) = n.get("children").and_then(Value::as_array) {
                scan(children, ex);
            }
        }
    }
    for page in &pages {
        for layer in page["layers"].as_array().into_iter().flatten() {
            if let Some(nodes) = layer["nodes"].as_array() {
                scan(nodes, &mut ex);
            }
        }
    }
    let parents = layout
        .get("parents")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for parent in &parents {
        for layer in parent["layers"].as_array().into_iter().flatten() {
            if let Some(nodes) = layer["nodes"].as_array() {
                scan(nodes, &mut ex);
            }
        }
    }

    let mut parts: BTreeMap<String, String> = BTreeMap::new();
    let mut master_refs: Vec<String> = Vec::new();
    let mut spread_refs: Vec<String> = Vec::new();
    let mut story_refs: Vec<String> = Vec::new();

    // Layers are document-wide in IDML: register the page layers first; parent
    // items sit on the first of them (a parent's own layers are flattened).
    for page in &pages {
        for layer in page["layers"].as_array().into_iter().flatten() {
            ex.layer(layer);
        }
    }
    let master_layer = ex
        .layers
        .first()
        .map(|l| l.0.clone())
        .unwrap_or_else(|| "layer".into());
    // Parents become master spreads.
    let (default_w, default_h) = (
        pages[0]["canvas"]["width"].as_f64().unwrap_or(612.0),
        pages[0]["canvas"]["height"].as_f64().unwrap_or(792.0),
    );
    let mut master_ids: HashMap<String, String> = HashMap::new();
    for parent in &parents {
        let pen = parent.get("id").and_then(Value::as_str).unwrap_or("master");
        let id = ex.unique(pen);
        master_ids.insert(pen.to_string(), id);
    }
    for (index, parent) in parents.iter().enumerate() {
        let pen = parent.get("id").and_then(Value::as_str).unwrap_or("master");
        let id = master_ids[pen].clone();
        let part = format!("MasterSpreads/MasterSpread_{id}.xml");
        let page_id = ex.unique(&format!("{id}-page"));
        let applied = parent
            .get("based_on")
            .and_then(Value::as_str)
            .and_then(|b| master_ids.get(b))
            .cloned()
            .unwrap_or_else(|| "n".into());
        let mut items = String::new();
        let origin = (-default_w / 2.0, -default_h / 2.0);
        for layer in parent["layers"].as_array().into_iter().flatten() {
            for node in layer["nodes"].as_array().into_iter().flatten() {
                ex.item(node, origin, &master_layer, &part, 0, &mut items)?;
            }
        }
        let carried = ex.take(&[id.as_str(), pen], &part);
        let letter = char::from(b'A' + (index % 26) as u8);
        parts.insert(
            part.clone(),
            format!(
                "{XML_HEAD}<idPkg:MasterSpread xmlns:idPkg=\"{PACKAGING}\" DOMVersion=\"8.0\"><MasterSpread Self=\"{id}\" Name=\"{}\" NamePrefix=\"{letter}\" BaseName=\"{}\" PageCount=\"1\" ItemTransform=\"1 0 0 1 0 0\"><Page Self=\"{page_id}\" Name=\"{letter}\" GeometricBounds=\"0 0 {} {}\" ItemTransform=\"1 0 0 1 {} {}\" AppliedMaster=\"{applied}\">{}</Page>{items}{carried}</MasterSpread></idPkg:MasterSpread>\n",
                esc(pen),
                esc(pen),
                fmt(default_h),
                fmt(default_w),
                fmt(origin.0),
                fmt(origin.1),
                page_pref(Some(parent)),
            ),
        );
        master_refs.push(part);
        ex.rep
            .add("supported", "MasterSpreads", "MasterSpread", pen, "");
    }

    // Spreads: those in the layout first, then any page not in one.
    let page_by_id: HashMap<&str, &Value> = pages
        .iter()
        .filter_map(|p| p.get("id").and_then(Value::as_str).map(|i| (i, p)))
        .collect();
    let mut groups: Vec<(String, Vec<String>, bool)> = Vec::new();
    let mut placed: HashSet<String> = HashSet::new();
    for spread in layout
        .get("spreads")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let ids: Vec<String> = spread["pages"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|p| page_by_id.contains_key(p))
            .map(str::to_string)
            .collect();
        if ids.is_empty() {
            continue;
        }
        placed.extend(ids.iter().cloned());
        groups.push((
            spread
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("spread")
                .to_string(),
            ids,
            spread.get("facing").and_then(Value::as_bool) == Some(true),
        ));
    }
    for page in &pages {
        let id = page["id"].as_str().unwrap_or("page");
        if !placed.contains(id) {
            groups.push((format!("spread-{id}"), vec![id.to_string()], false));
        }
    }
    let mut page_export: HashMap<String, String> = HashMap::new();
    for (spread_pen, ids, facing) in &groups {
        let spread_id = ex.unique(spread_pen);
        let part = format!("Spreads/Spread_{spread_id}.xml");
        let total: f64 = ids
            .iter()
            .map(|p| {
                page_by_id[p.as_str()]["canvas"]["width"]
                    .as_f64()
                    .unwrap_or(0.0)
            })
            .sum();
        let mut x = -total / 2.0;
        let mut body = String::new();
        for pid in ids {
            let page = page_by_id[pid.as_str()];
            let w = page["canvas"]["width"].as_f64().unwrap_or(612.0);
            let h = page["canvas"]["height"].as_f64().unwrap_or(792.0);
            let eid = ex.unique(pid);
            page_export.insert(pid.clone(), eid.clone());
            let origin = (x, -h / 2.0);
            let master = page
                .get("layout")
                .and_then(|l| l.get("parent"))
                .and_then(Value::as_str)
                .and_then(|m| master_ids.get(m))
                .cloned()
                .unwrap_or_else(|| "n".into());
            let name = page.get("name").and_then(Value::as_str).unwrap_or(pid);
            let carried_page = ex.take(&[eid.as_str(), pid.as_str()], &part);
            body.push_str(&format!(
                "<Page Self=\"{eid}\" Name=\"{}\" GeometricBounds=\"0 0 {} {}\" ItemTransform=\"1 0 0 1 {} {}\" AppliedMaster=\"{master}\">{}{carried_page}</Page>",
                esc(name),
                fmt(h),
                fmt(w),
                fmt(origin.0),
                fmt(origin.1),
                page_pref(page.get("layout")),
            ));
            let mut items = String::new();
            for layer in page["layers"].as_array().into_iter().flatten() {
                let lid = ex.layer(layer);
                for node in layer["nodes"].as_array().into_iter().flatten() {
                    ex.item(node, origin, &lid, &part, 0, &mut items)?;
                }
            }
            body.push_str(&items);
            x += w;
        }
        let carried = ex.take(&[spread_id.as_str(), spread_pen.as_str()], &part);
        parts.insert(
            part.clone(),
            format!(
                "{XML_HEAD}<idPkg:Spread xmlns:idPkg=\"{PACKAGING}\" DOMVersion=\"8.0\"><Spread Self=\"{spread_id}\" PageCount=\"{}\" BindingLocation=\"{}\" ItemTransform=\"1 0 0 1 0 0\">{body}{carried}</Spread></idPkg:Spread>\n",
                ids.len(),
                if *facing { 1 } else { 0 },
            ),
        );
        spread_refs.push(part);
        ex.rep.add("supported", "Spreads", "Spread", spread_pen, "");
    }

    // Stories.
    for story in &story_list {
        let Some(pen) = story.get("id").and_then(Value::as_str) else {
            continue;
        };
        let id = ex.stories[pen].clone();
        let xml = ex.story(story, &id);
        let part = format!("Stories/Story_{id}.xml");
        parts.insert(part.clone(), xml);
        story_refs.push(part);
    }

    // Styles.
    let ext_styles = idml_ext
        .and_then(|e| e.get("style_properties"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut paragraph_xml = String::new();
    let mut character_xml = String::new();
    if let Some(styles) = layout.get("styles") {
        for (kind, sink) in [
            ("paragraph", &mut paragraph_xml),
            ("character", &mut character_xml),
        ] {
            if let Some(map) = styles.get(kind).and_then(Value::as_object) {
                for (name, style) in map {
                    let extra = ext_styles.get(&format!("{kind}:{name}"));
                    sink.push_str(&ex.style_element(kind, name, style, extra));
                }
            }
        }
        if styles
            .get("object")
            .and_then(Value::as_object)
            .is_some_and(|o| !o.is_empty())
        {
            ex.rep.add(
                "approximated",
                "Resources/Styles.xml",
                "ObjectStyle",
                "object",
                "object styles are not exported yet",
            );
        }
    }
    parts.insert(
        "Resources/Styles.xml".into(),
        format!(
            "{XML_HEAD}<idPkg:Styles xmlns:idPkg=\"{PACKAGING}\" DOMVersion=\"8.0\"><RootParagraphStyleGroup Self=\"uparagraphroot\">{paragraph_xml}</RootParagraphStyleGroup><RootCharacterStyleGroup Self=\"ucharacterroot\">{character_xml}</RootCharacterStyleGroup></idPkg:Styles>\n"
        ),
    );
    parts.insert(
        "Resources/Graphic.xml".into(),
        format!(
            "{XML_HEAD}<idPkg:Graphic xmlns:idPkg=\"{PACKAGING}\" DOMVersion=\"8.0\">{}<Swatch Self=\"Swatch/None\" Name=\"None\"/></idPkg:Graphic>\n",
            ex.swatch_xml.concat()
        ),
    );
    let families: Vec<&str> = idml_ext
        .and_then(|e| e.get("font_families"))
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    parts.insert(
        "Resources/Fonts.xml".into(),
        format!(
            "{XML_HEAD}<idPkg:Fonts xmlns:idPkg=\"{PACKAGING}\" DOMVersion=\"8.0\">{}</idPkg:Fonts>\n",
            families
                .iter()
                .map(|f| format!(
                    "<FontFamily Self=\"FontFamily/{n}\" Name=\"{n}\"/>",
                    n = esc(f)
                ))
                .collect::<String>()
        ),
    );

    // Verbatim document-level parts.
    let mut extra_refs = String::new();
    for (part, (element, xml)) in &other_parts {
        parts.insert(part.clone(), xml.clone());
        extra_refs.push_str(&format!("<idPkg:{element} src=\"{}\"/>", esc(part)));
        ex.rep.add(
            "carried",
            part,
            element,
            "Document",
            "part returned unchanged",
        );
    }

    // Sections.
    let mut sections = String::new();
    for (index, section) in layout
        .get("sections")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let Some(start) = section
            .get("start_page")
            .and_then(Value::as_str)
            .and_then(|p| page_export.get(p))
        else {
            continue;
        };
        let id = ex.unique(
            section
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or(&format!("section-{}", index + 1)),
        );
        let mut attrs = format!(
            " PageStart=\"{start}\" PageNumberStyle=\"{}\" ContinueNumbering=\"{}\"",
            section_style(
                section
                    .get("style")
                    .and_then(Value::as_str)
                    .unwrap_or("arabic")
            ),
            section.get("continue").and_then(Value::as_bool) == Some(true)
        );
        if let Some(n) = section.get("start_number").and_then(Value::as_u64) {
            attrs.push_str(&format!(" PageNumberStart=\"{n}\""));
        }
        if let Some(p) = section.get("prefix").and_then(Value::as_str) {
            attrs.push_str(&format!(
                " IncludeSectionPrefix=\"true\" SectionPrefix=\"{}\"",
                esc(p)
            ));
        }
        sections.push_str(&format!("<Section Self=\"{id}\"{attrs}/>"));
        ex.rep.add("supported", "designmap.xml", "Section", &id, "");
    }

    // Anything carried that found no owner is reported, never silent.
    for c in &ex.carried {
        if !c.used {
            let (part, element, owner) = (c.part.clone(), c.element.clone(), c.owner.clone());
            ex.rep.add(
                "carried-dropped",
                &part,
                &element,
                &owner,
                "the owning element no longer exists, so the carried block was not written",
            );
        }
    }

    let layers: String = ex
        .layers
        .iter()
        .map(|(id, name, visible, locked)| {
            format!(
                "<Layer Self=\"{}\" Name=\"{}\" Visible=\"{visible}\" Locked=\"{locked}\"/>",
                esc(id),
                esc(name)
            )
        })
        .collect();
    let refs = |tag: &str, list: &[String]| -> String {
        list.iter()
            .map(|p| format!("<idPkg:{tag} src=\"{}\"/>", esc(p)))
            .collect()
    };
    parts.insert(
        "designmap.xml".into(),
        format!(
            "{XML_HEAD}<Document xmlns:idPkg=\"{PACKAGING}\" DOMVersion=\"8.0\" Self=\"d\"><idPkg:Graphic src=\"Resources/Graphic.xml\"/><idPkg:Fonts src=\"Resources/Fonts.xml\"/><idPkg:Styles src=\"Resources/Styles.xml\"/>{layers}{sections}{}{}{}{extra_refs}</Document>\n",
            refs("MasterSpread", &master_refs),
            refs("Spread", &spread_refs),
            refs("Story", &story_refs),
        ),
    );
    parts.insert(
        "META-INF/container.xml".into(),
        format!(
            "{XML_HEAD}<container version=\"1.0\" xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\"><rootfiles><rootfile full-path=\"designmap.xml\" media-type=\"text/xml\"/></rootfiles></container>\n"
        ),
    );

    let blocking = ex.rep.gating();
    if !allow_approximation && !blocking.is_empty() {
        let shown: Vec<String> = blocking
            .iter()
            .take(8)
            .map(|i| {
                format!(
                    "{} {} ({}): {}",
                    i["element"].as_str().unwrap_or(""),
                    i["ref"].as_str().unwrap_or(""),
                    i["status"].as_str().unwrap_or(""),
                    i["message"].as_str().unwrap_or("")
                )
            })
            .collect();
        bail!(
            "[unsupported-capability] {} feature(s) have no exact IDML equivalent; nothing was written. Pass --allow-approximation to export with them dropped or approximated and listed in the report. {}",
            blocking.len(),
            shown.join("; ")
        );
    }

    // Deterministic package: mimetype first and stored, then sorted entries.
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut cursor);
        let stamp = zip::DateTime::from_date_and_time(1980, 1, 1, 0, 0, 0)
            .map_err(|_| anyhow::anyhow!("[internal-error] invalid fixed timestamp"))?;
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .last_modified_time(stamp)
            .unix_permissions(0o644);
        let deflated = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .last_modified_time(stamp)
            .unix_permissions(0o644);
        zip.start_file("mimetype", stored)?;
        zip.write_all(MIMETYPE.as_bytes())?;
        for (name, text) in &parts {
            zip.start_file(name.as_str(), deflated)?;
            zip.write_all(text.as_bytes())?;
        }
        zip.finish()?;
    }
    let bytes = cursor.into_inner();
    ensure!(
        bytes.len() as u64 <= MAX_ARCHIVE_BYTES,
        "[limit-exceeded] the IDML package would be {} bytes; the limit is {MAX_ARCHIVE_BYTES}",
        bytes.len()
    );
    let report = json!({
        "source": source,
        "counts": {
            "supported": ex.rep.counts[0], "approximated": ex.rep.counts[1],
            "carried": ex.rep.counts[2], "unsupported": ex.rep.counts[3],
            "carried_dropped": ex.rep.counts[4],
        },
        "items": ex.rep.items,
        "parts": parts.len() + 1,
        "pages": pages.len(),
        "spreads": spread_refs.len(),
        "stories": story_refs.len(),
        "bytes": bytes.len(),
        "allow_approximation": allow_approximation,
    });
    Ok(Exported { bytes, report })
}
