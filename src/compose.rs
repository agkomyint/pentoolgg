//! v0.14.0 task 5: the paragraph composer.
//!
//! A total-fit (Knuth–Plass family) line breaker and a first-fit single-line
//! composer, Liang hyphenation from bundled patterns, justification across
//! word spacing, letter spacing and glyph scaling, indents, drop caps, optical
//! margin alignment, hanging punctuation, balanced ragged lines and keep
//! (widow/orphan) arithmetic. All decisions use integer layout units (LU =
//! 1/1024 pt); no float takes part in a break decision.
use crate::shaping::{self, Kerning, ResolvedFont, ShapeParams};
use anyhow::{bail, Context, Result};
use flate2::read::GzDecoder;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::io::Read;
use std::sync::{Arc, Mutex, OnceLock};

/// Recorded in `layout.engine`; bump on any behavior change.
pub const ENGINE: u32 = 1;

const INF: i64 = 1 << 40;
const LINE_PENALTY: i64 = 10;
const HYPHEN_PENALTY: i32 = 50;
const DOUBLE_HYPHEN_DEMERITS: i64 = 10_000;
const FINAL_HYPHEN_DEMERITS: i64 = 5_000;
const ADJ_DEMERITS: i64 = 10_000;
const OVERFULL_DEMERITS: i64 = 1_000_000_000_000;
const FORCED: i32 = -10_000;
const MAX_ACTIVE: usize = 20_000;
pub const MAX_PARAGRAPH_CHARS: usize = 100_000;

// ---------------------------------------------------------------- hyphenation

struct Patterns {
    table: HashMap<Vec<char>, Vec<u8>>,
    max_len: usize,
    exceptions: HashMap<String, Vec<usize>>,
}

const SETS: [(&str, &[u8]); 17] = [
    (
        "en-us",
        include_bytes!("../assets/hyphenation/en-us.pat.gz"),
    ),
    (
        "en-gb",
        include_bytes!("../assets/hyphenation/en-gb.pat.gz"),
    ),
    (
        "de-1996",
        include_bytes!("../assets/hyphenation/de-1996.pat.gz"),
    ),
    ("fr", include_bytes!("../assets/hyphenation/fr.pat.gz")),
    ("es", include_bytes!("../assets/hyphenation/es.pat.gz")),
    ("it", include_bytes!("../assets/hyphenation/it.pat.gz")),
    ("pt", include_bytes!("../assets/hyphenation/pt.pat.gz")),
    ("nl", include_bytes!("../assets/hyphenation/nl.pat.gz")),
    ("sv", include_bytes!("../assets/hyphenation/sv.pat.gz")),
    ("da", include_bytes!("../assets/hyphenation/da.pat.gz")),
    ("nb", include_bytes!("../assets/hyphenation/nb.pat.gz")),
    ("fi", include_bytes!("../assets/hyphenation/fi.pat.gz")),
    ("pl", include_bytes!("../assets/hyphenation/pl.pat.gz")),
    ("ru", include_bytes!("../assets/hyphenation/ru.pat.gz")),
    ("uk", include_bytes!("../assets/hyphenation/uk.pat.gz")),
    (
        "el-monoton",
        include_bytes!("../assets/hyphenation/el-monoton.pat.gz"),
    ),
    ("tr", include_bytes!("../assets/hyphenation/tr.pat.gz")),
];
const EN_US_EXCEPTIONS: &[u8] = include_bytes!("../assets/hyphenation/en-us.hyp.gz");

/// Map a BCP 47 tag onto a bundled pattern set.
pub fn pattern_set(language: &str) -> Option<&'static str> {
    let lower = language.to_ascii_lowercase().replace('_', "-");
    let primary = lower.split('-').next().unwrap_or("");
    Some(match (lower.as_str(), primary) {
        ("en-gb" | "en-au" | "en-nz" | "en-ie" | "en-za", _) => "en-gb",
        (_, "en") => "en-us",
        (_, "de") => "de-1996",
        (_, "el") => "el-monoton",
        (_, "nb" | "no" | "nn") => "nb",
        (_, p) => SETS.iter().map(|(n, _)| *n).find(|n| *n == p)?,
    })
}

fn gunzip(bytes: &[u8]) -> String {
    let mut out = String::new();
    let _ = GzDecoder::new(bytes).read_to_string(&mut out);
    out
}

fn patterns(set: &str) -> Option<Arc<Patterns>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<Patterns>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(found) = cache.lock().expect("hyphenation cache").get(set) {
        return Some(found.clone());
    }
    let (_, bytes) = SETS.iter().find(|(name, _)| *name == set)?;
    let mut table = HashMap::new();
    let mut max_len = 0;
    for line in gunzip(bytes).lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('%') {
            continue;
        }
        let mut letters = Vec::new();
        let mut digits = vec![0u8];
        for ch in line.chars() {
            if let Some(d) = ch.to_digit(10) {
                *digits.last_mut().expect("digits start nonempty") = d as u8;
            } else {
                letters.push(ch);
                digits.push(0);
            }
        }
        max_len = max_len.max(letters.len());
        table.insert(letters, digits);
    }
    let mut exceptions = HashMap::new();
    if set == "en-us" {
        for line in gunzip(EN_US_EXCEPTIONS).lines() {
            if let Some((word, points)) = parse_exception(line) {
                exceptions.insert(word, points);
            }
        }
    }
    let built = Arc::new(Patterns {
        table,
        max_len,
        exceptions,
    });
    cache
        .lock()
        .expect("hyphenation cache")
        .insert(set.to_string(), built.clone());
    Some(built)
}

/// `as-so-ciate` -> ("associate", [2, 4]) (hyphen positions in characters).
fn parse_exception(line: &str) -> Option<(String, Vec<usize>)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('%') {
        return None;
    }
    let mut word = String::new();
    let mut points = Vec::new();
    let mut count = 0;
    for ch in line.chars() {
        if ch == '-' {
            points.push(count);
        } else {
            word.extend(ch.to_lowercase());
            count += 1;
        }
    }
    Some((word, points))
}

/// Hyphenation parameters of a paragraph.
#[derive(Clone)]
pub struct Hyphenation {
    pub enabled: bool,
    pub min_word: usize,
    pub min_before: usize,
    pub min_after: usize,
    pub max_consecutive: usize,
    pub capitalized: bool,
    pub last_word: bool,
    pub exceptions: HashMap<String, Vec<usize>>,
}

impl Default for Hyphenation {
    fn default() -> Self {
        Self {
            enabled: false,
            min_word: 5,
            min_before: 2,
            min_after: 2,
            max_consecutive: 3,
            capitalized: true,
            last_word: true,
            exceptions: HashMap::new(),
        }
    }
}

/// Hyphen positions (in characters from the word start) for `word`.
fn hyphenate_word(set: &Patterns, word: &[char], opts: &Hyphenation) -> Vec<usize> {
    let n = word.len();
    if n < opts.min_word || n < opts.min_before + opts.min_after {
        return Vec::new();
    }
    let lower: String = word.iter().flat_map(|c| c.to_lowercase()).collect();
    if lower.chars().count() != n {
        return Vec::new();
    }
    if let Some(points) = opts
        .exceptions
        .get(&lower)
        .or_else(|| set.exceptions.get(&lower))
    {
        return points
            .iter()
            .copied()
            .filter(|p| *p >= opts.min_before && *p + opts.min_after <= n)
            .collect();
    }
    let mut dotted: Vec<char> = Vec::with_capacity(n + 2);
    dotted.push('.');
    dotted.extend(lower.chars());
    dotted.push('.');
    let mut values = vec![0u8; dotted.len() + 1];
    for start in 0..dotted.len() {
        for len in 1..=set.max_len.min(dotted.len() - start) {
            if let Some(digits) = set.table.get(&dotted[start..start + len]) {
                for (k, d) in digits.iter().enumerate() {
                    let slot = &mut values[start + k];
                    *slot = (*slot).max(*d);
                }
            }
        }
    }
    // values[m] sits before dotted[m]; a break between letters j-1 and j is
    // before dotted index j+1.
    (opts.min_before..=n - opts.min_after)
        .filter(|j| values[j + 1] % 2 == 1)
        .collect()
}

// ------------------------------------------------------------ paragraph model

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Align {
    Left,
    Center,
    Right,
    Justify,
    JustifyLastLeft,
    JustifyLastCenter,
    JustifyLastRight,
    JustifyLastAll,
}

impl Align {
    pub fn parse(text: &str) -> Result<Self> {
        Ok(match text {
            "left" | "towards-spine" => Self::Left,
            "right" | "away-from-spine" => Self::Right,
            "center" => Self::Center,
            "justify" | "justify-last-left" => Self::JustifyLastLeft,
            "justify-last-center" => Self::JustifyLastCenter,
            "justify-last-right" => Self::JustifyLastRight,
            "justify-last-all" => Self::JustifyLastAll,
            other => bail!("[invalid-input] alignment `{other}` is not supported"),
        })
    }
    fn justified(self) -> bool {
        !matches!(self, Self::Left | Self::Center | Self::Right)
    }
}

/// `min / desired / max` triple in percent.
#[derive(Clone, Copy)]
pub struct Range3 {
    pub min: i64,
    pub desired: i64,
    pub max: i64,
}

#[derive(Clone)]
pub struct DropCap {
    pub lines: usize,
    pub chars: usize,
}

#[derive(Clone)]
pub struct Params {
    pub width_lu: i64,
    pub align: Align,
    pub single_line: bool,
    pub left_indent: i64,
    pub right_indent: i64,
    pub first_indent: i64,
    pub last_indent: i64,
    pub word_spacing: Range3,
    /// Percent of an em.
    pub letter_spacing: Range3,
    pub glyph_scaling: Range3,
    pub hyphenation: Hyphenation,
    pub language: String,
    pub optical_margin: bool,
    pub hanging_punctuation: bool,
    pub balance: bool,
    pub drop_cap: Option<DropCap>,
    pub leading_lu: i64,
    pub size_lu: i64,
    pub tolerance: i64,
}

impl Params {
    pub fn new(width_lu: i64, size_lu: i64) -> Self {
        Self {
            width_lu,
            align: Align::Left,
            single_line: false,
            left_indent: 0,
            right_indent: 0,
            first_indent: 0,
            last_indent: 0,
            word_spacing: Range3 {
                min: 80,
                desired: 100,
                max: 133,
            },
            letter_spacing: Range3 {
                min: 0,
                desired: 0,
                max: 0,
            },
            glyph_scaling: Range3 {
                min: 100,
                desired: 100,
                max: 100,
            },
            hyphenation: Hyphenation::default(),
            language: "en-US".into(),
            optical_margin: false,
            hanging_punctuation: false,
            balance: false,
            drop_cap: None,
            leading_lu: size_lu * 12 / 10,
            size_lu,
            tolerance: 1000,
        }
    }
}

#[derive(Clone, Debug)]
enum Item {
    Box {
        w: i64,
        start: usize,
        end: usize,
        letters: i64,
    },
    Glue {
        w: i64,
        stretch: i64,
        shrink: i64,
        pos: usize,
    },
    Penalty {
        w: i64,
        p: i32,
        flagged: bool,
        pos: usize,
    },
}

/// One composed line. Offsets are characters of the paragraph text.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub start: usize,
    pub end: usize,
    /// Left edge of the line content relative to the column, in LU.
    pub x: i64,
    /// Content width after justification, in LU.
    pub width: i64,
    pub natural: i64,
    /// Adjustment ratio in thousandths (negative shrinks).
    pub ratio_milli: i64,
    pub hyphenated: bool,
    pub overfull: bool,
    pub word_spacing_pct: i64,
    pub letter_spacing_lu: i64,
    pub glyph_scale_pct: i64,
    pub left_hang: i64,
    pub right_hang: i64,
}

#[derive(Debug)]
pub struct Composed {
    pub lines: Vec<Line>,
    pub passes: u8,
    pub demerits: i64,
    pub hyphens: usize,
    pub overfull: usize,
    pub hyphenation_unavailable: bool,
    pub drop_cap_width: i64,
}

fn is_space(ch: char) -> bool {
    ch == ' '
}

fn is_letter(ch: char) -> bool {
    ch.is_alphabetic()
}

/// Per-character widths from a whole-paragraph shaping.
pub fn char_widths(font: &ResolvedFont, text: &str, shape: &ShapeParams) -> Result<Vec<i64>> {
    let shaped = shaping::shape(font, text, shape)?;
    let mut widths = vec![0i64; text.chars().count()];
    for run in &shaped.runs {
        for g in &run.glyphs {
            if let Some(slot) = widths.get_mut(g.cluster) {
                *slot += g.x_advance;
            }
        }
    }
    Ok(widths)
}

fn build_items(
    chars: &[char],
    widths: &[i64],
    hyphen_w: i64,
    params: &Params,
    hyphenate: bool,
) -> (Vec<Item>, bool) {
    let n = chars.len();
    let text: String = chars.iter().collect();
    let mut opportunity: Vec<Option<bool>> = vec![None; n + 1];
    let mut byte_to_char: HashMap<usize, usize> = HashMap::new();
    for (index, (byte, _)) in text.char_indices().enumerate() {
        byte_to_char.insert(byte, index);
    }
    byte_to_char.insert(text.len(), n);
    for (byte, kind) in unicode_linebreak::linebreaks(&text) {
        if let Some(&at) = byte_to_char.get(&byte) {
            if at > 0 && at < n {
                opportunity[at] = Some(kind == unicode_linebreak::BreakOpportunity::Mandatory);
            }
        }
    }
    // Hyphenation opportunities.
    let mut hyphen_at = vec![false; n + 1];
    let mut unavailable = false;
    if hyphenate {
        match pattern_set(&params.language).and_then(patterns) {
            None => unavailable = true,
            Some(set) => {
                let mut i = 0;
                while i < n {
                    if !is_letter(chars[i]) {
                        i += 1;
                        continue;
                    }
                    let start = i;
                    while i < n && is_letter(chars[i]) {
                        i += 1;
                    }
                    let word = &chars[start..i];
                    let capitalized = word[0].is_uppercase();
                    let last_word = chars[i..].iter().all(|c| !is_letter(*c));
                    if (!capitalized || params.hyphenation.capitalized)
                        && (!last_word || params.hyphenation.last_word)
                    {
                        for p in hyphenate_word(&set, word, &params.hyphenation) {
                            hyphen_at[start + p] = true;
                        }
                    }
                }
            }
        }
    }
    let ws = params.word_spacing;
    let mut items: Vec<Item> = Vec::new();
    let mut box_start = 0usize;
    let mut box_w = 0i64;
    let mut box_letters = 0i64;
    let flush = |items: &mut Vec<Item>, start: usize, end: usize, w: i64, letters: i64| {
        if end > start {
            items.push(Item::Box {
                w,
                start,
                end,
                letters,
            });
        }
    };
    for i in 0..n {
        let ch = chars[i];
        // Break opportunities *before* char i.
        if i > 0 {
            let prev_space = is_space(chars[i - 1]);
            if let Some(mandatory) = opportunity[i] {
                flush(&mut items, box_start, i, box_w, box_letters);
                box_start = i;
                box_w = 0;
                box_letters = 0;
                if mandatory {
                    items.push(Item::Penalty {
                        w: 0,
                        p: FORCED,
                        flagged: false,
                        pos: i,
                    });
                } else if !prev_space {
                    items.push(Item::Penalty {
                        w: 0,
                        p: 0,
                        flagged: false,
                        pos: i,
                    });
                }
            } else if hyphen_at[i] || chars[i - 1] == '\u{ad}' {
                flush(&mut items, box_start, i, box_w, box_letters);
                box_start = i;
                box_w = 0;
                box_letters = 0;
                items.push(Item::Penalty {
                    w: hyphen_w,
                    p: HYPHEN_PENALTY,
                    flagged: true,
                    pos: i,
                });
            }
        }
        if is_space(ch) {
            flush(&mut items, box_start, i, box_w, box_letters);
            box_w = 0;
            box_letters = 0;
            let w = widths[i];
            items.push(Item::Glue {
                w: w * ws.desired / 100,
                stretch: w * (ws.max - ws.desired).max(0) / 100,
                shrink: w * (ws.desired - ws.min).max(0) / 100,
                pos: i,
            });
            box_start = i + 1;
        } else {
            box_w += widths[i];
            if is_letter(ch) {
                box_letters += 1;
            }
        }
    }
    flush(&mut items, box_start, n, box_w, box_letters);
    items.push(Item::Glue {
        w: 0,
        stretch: INF,
        shrink: 0,
        pos: n,
    });
    items.push(Item::Penalty {
        w: 0,
        p: FORCED,
        flagged: false,
        pos: n,
    });
    (items, unavailable)
}

#[derive(Clone)]
struct Node {
    /// Index of the break item that ends the previous line (items.len() = start).
    item: usize,
    line: usize,
    class: u8,
    demerits: i64,
    prev: Option<usize>,
    flagged: bool,
    hyphens: usize,
}

struct Totals {
    w: Vec<i64>,
    y: Vec<i64>,
    z: Vec<i64>,
    t: Vec<i64>,
    l: Vec<i64>,
}

fn totals(items: &[Item]) -> Totals {
    let mut t = Totals {
        w: vec![0],
        y: vec![0],
        z: vec![0],
        t: vec![0],
        l: vec![0],
    };
    for item in items {
        let (w, y, z, text, letters) = match item {
            Item::Box { w, letters, .. } => (*w, 0, 0, *w, *letters),
            Item::Glue {
                w, stretch, shrink, ..
            } => (*w, *stretch, *shrink, 0, 0),
            Item::Penalty { .. } => (0, 0, 0, 0, 0),
        };
        t.w.push(t.w.last().unwrap() + w);
        t.y.push((t.y.last().unwrap() + y).min(INF));
        t.z.push(t.z.last().unwrap() + z);
        t.t.push(t.t.last().unwrap() + text);
        t.l.push(t.l.last().unwrap() + letters);
    }
    t
}

/// A rational adjustment ratio.
#[derive(Clone, Copy)]
struct Ratio {
    num: i128,
    den: i128,
}

struct Fit {
    ratio: Ratio,
    class: u8,
    badness: i64,
    infeasible: bool,
}

fn badness_of(r: Ratio) -> i64 {
    let n = r.num.abs();
    if r.den == 0 {
        return 10_000;
    }
    let cube = 100 * n * n * n;
    let d = r.den * r.den * r.den;
    ((cube / d).min(10_000)) as i64
}

fn class_of(r: Ratio) -> u8 {
    // thresholds -1/2, 1/2, 1 in rational form
    if r.num * 2 < -r.den {
        0
    } else if r.num * 2 <= r.den {
        1
    } else if r.num <= r.den {
        2
    } else {
        3
    }
}

/// Fit a line of natural width `w` with flexibility into `target`.
fn fit(natural: i64, stretch: i64, shrink: i64, target: i64, emergency: bool) -> Fit {
    let (ratio, infeasible) = if natural < target {
        if stretch >= INF / 2 {
            (Ratio { num: 0, den: 1 }, false)
        } else if stretch > 0 {
            (
                Ratio {
                    num: i128::from(target - natural),
                    den: i128::from(stretch),
                },
                false,
            )
        } else {
            (
                Ratio {
                    num: 1_000_000,
                    den: 1,
                },
                false,
            )
        }
    } else if natural > target {
        if shrink > 0 {
            let r = Ratio {
                num: -i128::from(natural - target),
                den: i128::from(shrink),
            };
            (r, -r.num > r.den && !emergency)
        } else {
            (
                Ratio {
                    num: -1_000_000,
                    den: 1,
                },
                !emergency,
            )
        }
    } else {
        (Ratio { num: 0, den: 1 }, false)
    };
    let overshrunk = -ratio.num > ratio.den;
    let badness = if overshrunk {
        10_000
    } else {
        badness_of(ratio)
    };
    Fit {
        ratio,
        class: class_of(ratio),
        badness,
        infeasible,
    }
}

struct Plan<'a> {
    items: &'a [Item],
    tot: &'a Totals,
    widths: &'a [i64],
    params: &'a Params,
}

impl Plan<'_> {
    fn target(&self, line: usize) -> i64 {
        self.widths[line.min(self.widths.len() - 1)]
    }

    /// Flexibility of the line made of items[start..end] (a break at `end`).
    fn flex(&self, start: usize, end: usize, line: usize) -> (i64, i64, i64) {
        let t = self.tot;
        let mut natural = t.w[end] - t.w[start];
        if let Item::Penalty { w, .. } = self.items[end] {
            natural += w;
        }
        let letters = t.l[end] - t.l[start];
        let text = t.t[end] - t.t[start];
        let p = self.params;
        let em = p.size_lu;
        let ls_stretch =
            letters * em * (p.letter_spacing.max - p.letter_spacing.desired).max(0) / 100;
        let ls_shrink =
            letters * em * (p.letter_spacing.desired - p.letter_spacing.min).max(0) / 100;
        let gs_stretch = text * (p.glyph_scaling.max - p.glyph_scaling.desired).max(0) / 100;
        let gs_shrink = text * (p.glyph_scaling.desired - p.glyph_scaling.min).max(0) / 100;
        let stretch = (t.y[end] - t.y[start]).min(INF);
        let stretch = if stretch >= INF / 2 {
            INF
        } else {
            stretch + ls_stretch + gs_stretch
        };
        let shrink = t.z[end] - t.z[start] + ls_shrink + gs_shrink;
        // Ragged alignments get slack so short lines are not scored as defects.
        let stretch = if stretch < INF / 2 && !p.align.justified() {
            stretch + self.target(line) * 3 / 10
        } else {
            stretch
        };
        (natural, stretch, shrink)
    }

    /// Start item of the line after a break at `b`: skip discardable items.
    fn next_start(&self, b: usize) -> usize {
        let mut s = b + 1;
        while s < self.items.len() && !matches!(self.items[s], Item::Box { .. }) {
            s += 1;
        }
        s
    }
}

/// Total-fit breaking. Returns break item indices in order, or None.
fn total_fit(
    plan: &Plan,
    tolerance: i64,
    emergency: bool,
    hyphen_limit: usize,
) -> Option<(Vec<usize>, i64, usize)> {
    let items = plan.items;
    let mut arena: Vec<Node> = vec![Node {
        item: usize::MAX,
        line: 0,
        class: 1,
        demerits: 0,
        prev: None,
        flagged: false,
        hyphens: 0,
    }];
    let mut active: Vec<usize> = vec![0];
    let mut last_best: Option<usize> = None;
    for (i, item) in items.iter().enumerate() {
        let (pen, is_break) = match item {
            Item::Glue { .. } => (
                0,
                i > 0 && i + 2 < items.len() && matches!(items[i - 1], Item::Box { .. }),
            ),
            Item::Penalty { p, .. } => (*p, *p < 10_000),
            Item::Box { .. } => (0, false),
        };
        if !is_break {
            continue;
        }
        let (flagged_break, pen_w) = match item {
            Item::Penalty { flagged, w, .. } => (*flagged, *w),
            _ => (false, 0),
        };
        let _ = pen_w;
        let forced = pen <= FORCED;
        // best candidate per (class, line-bucket)
        let mut best: HashMap<(u8, usize), (i64, usize)> = HashMap::new();
        let mut deactivate: Vec<usize> = Vec::new();
        for &a in &active {
            let node = &arena[a];
            let start = if node.item == usize::MAX {
                0
            } else {
                plan.next_start(node.item)
            };
            if start >= i {
                continue;
            }
            let (natural, stretch, shrink) = plan.flex(start, i, node.line);
            let target = plan.target(node.line);
            let f = fit(natural, stretch, shrink, target, emergency);
            if f.infeasible && natural > target {
                deactivate.push(a);
            }
            if f.infeasible || (f.badness > tolerance && !emergency && !forced) {
                continue;
            }
            if flagged_break && node.flagged && node.hyphens >= hyphen_limit {
                continue;
            }
            let mut d = LINE_PENALTY + f.badness;
            d *= d;
            if natural - shrink > target {
                d += OVERFULL_DEMERITS;
            }
            if pen > 0 {
                d += i64::from(pen) * i64::from(pen);
            } else if pen > FORCED {
                d -= i64::from(pen) * i64::from(pen);
            }
            if flagged_break && node.flagged {
                d += DOUBLE_HYPHEN_DEMERITS;
            }
            let last =
                matches!(items[i], Item::Penalty { p, .. } if p <= FORCED) && i == items.len() - 1;
            if last && node.flagged {
                d += FINAL_HYPHEN_DEMERITS;
            }
            if (i16::from(f.class) - i16::from(node.class)).abs() > 1 {
                d += ADJ_DEMERITS;
            }
            let total = node.demerits.saturating_add(d);
            let key = (f.class, (node.line + 1).min(plan.widths.len()));
            if best.get(&key).is_none_or(|(bd, _)| total < *bd) {
                best.insert(key, (total, a));
            }
        }
        // Deactivate nodes that can no longer fit (keep forced breaks working).
        active.retain(|a| !deactivate.contains(a));
        let mut keys: Vec<_> = best.keys().copied().collect();
        keys.sort_unstable();
        for key in keys {
            let (total, from) = best[&key];
            if arena.len() > MAX_ACTIVE * 8 {
                break;
            }
            arena.push(Node {
                item: i,
                line: arena[from].line + 1,
                class: key.0,
                demerits: total,
                prev: Some(from),
                flagged: flagged_break,
                hyphens: if flagged_break {
                    arena[from].hyphens + 1
                } else {
                    0
                },
            });
            let id = arena.len() - 1;
            last_best = Some(id);
            if !forced {
                active.push(id);
            }
        }
        if forced {
            // A forced break closes every line: only the new nodes continue.
            let fresh: Vec<usize> = arena
                .iter()
                .enumerate()
                .filter(|(_, n)| n.item == i)
                .map(|(id, _)| id)
                .collect();
            if fresh.is_empty() {
                return None;
            }
            if i == items.len() - 1 {
                let winner = fresh
                    .iter()
                    .copied()
                    .min_by_key(|&id| (arena[id].demerits, arena[id].line, id))?;
                let mut breaks = Vec::new();
                let mut at = Some(winner);
                while let Some(id) = at {
                    if arena[id].item != usize::MAX {
                        breaks.push(arena[id].item);
                    }
                    at = arena[id].prev;
                }
                breaks.reverse();
                return Some((breaks, arena[winner].demerits, arena[winner].hyphens));
            }
            active = fresh;
        }
        if active.len() > MAX_ACTIVE {
            // Bound memory: keep the best nodes, deterministically.
            active.sort_by_key(|&id| (arena[id].demerits, id));
            active.truncate(MAX_ACTIVE / 2);
        }
        if active.is_empty() {
            return None;
        }
    }
    let _ = last_best;
    None
}

/// First-fit composer: fill each line greedily.
fn first_fit(plan: &Plan) -> Vec<usize> {
    let items = plan.items;
    let mut breaks = Vec::new();
    let mut start = 0usize;
    let mut line = 0usize;
    let mut last_ok: Option<usize> = None;
    let mut i = start;
    while i < items.len() {
        let is_break = match &items[i] {
            Item::Glue { .. } => i > 0 && matches!(items[i - 1], Item::Box { .. }),
            Item::Penalty { p, .. } => *p < 10_000,
            Item::Box { .. } => false,
        };
        if is_break {
            let forced = matches!(items[i], Item::Penalty { p, .. } if p <= FORCED);
            let (natural, _, _) = plan.flex(start, i, line);
            let target = plan.target(line);
            if forced || i == items.len() - 1 {
                if natural <= target || last_ok.is_none() || forced {
                    if i == items.len() - 1 && natural > target {
                        if let Some(b) = last_ok.take() {
                            breaks.push(b);
                            start = plan.next_start(b);
                            line += 1;
                            i = start;
                            continue;
                        }
                    }
                    breaks.push(i);
                    if i == items.len() - 1 {
                        break;
                    }
                    start = plan.next_start(i);
                    line += 1;
                    last_ok = None;
                    i = start;
                    continue;
                }
            } else if natural <= target {
                last_ok = Some(i);
            } else {
                let b = last_ok.take().unwrap_or(i);
                breaks.push(b);
                start = plan.next_start(b);
                line += 1;
                i = start;
                continue;
            }
        }
        i += 1;
    }
    if breaks.last() != Some(&(items.len() - 1)) {
        breaks.push(items.len() - 1);
    }
    breaks
}

/// Hanging fraction (percent of the advance) at a left or right margin.
fn hang_percent(ch: char, left: bool, full: bool) -> i64 {
    match ch {
        '.' | ',' | '-' | '\u{2010}' | '\u{2013}' | '\u{2014}' | ':' | ';' if !left => {
            if full {
                100
            } else {
                60
            }
        }
        '"' | '\'' | '\u{201c}' | '\u{201d}' | '\u{2018}' | '\u{2019}' | '\u{ab}' | '\u{bb}' => {
            if full {
                100
            } else {
                70
            }
        }
        '(' | '[' | '{' if left => 40,
        ')' | ']' | '}' if !left => 40,
        _ => 0,
    }
}

/// Compose one paragraph. `widths` are per-character advances in LU, `hyphen_w`
/// the width of an inserted hyphen.
pub fn compose(
    text: &str,
    widths: &[i64],
    hyphen_w: i64,
    params: &Params,
    drop_cap_width: i64,
) -> Result<Composed> {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() > MAX_PARAGRAPH_CHARS {
        bail!("[limit-exceeded] a paragraph is limited to {MAX_PARAGRAPH_CHARS} characters");
    }
    if widths.len() != chars.len() {
        bail!("[invalid-input] one width is required per character");
    }
    if params.width_lu <= 0 {
        bail!("[invalid-input] column width must be positive");
    }
    let base = params.width_lu - params.left_indent - params.right_indent;
    if base <= 0 {
        bail!("[invalid-input] indents leave no room for text");
    }
    // Line target widths: line 0 gets the first-line indent; drop-cap lines
    // are indented by the cap's width.
    let drop_lines = params.drop_cap.as_ref().map_or(0, |d| d.lines);
    let mut line_widths: Vec<i64> = Vec::new();
    for line in 0..=drop_lines.max(1) {
        let mut w = base;
        if line == 0 {
            w -= params.first_indent;
        }
        if line < drop_lines {
            w -= drop_cap_width;
        }
        line_widths.push(w.max(1));
    }
    let mut composed = compose_at(
        &chars,
        widths,
        hyphen_w,
        params,
        &line_widths,
        drop_cap_width,
    )?;
    // Balanced ragged lines: narrow the measure while the line count holds.
    if params.balance && !params.align.justified() && composed.lines.len() > 1 {
        let count = composed.lines.len();
        let (mut lo, mut hi) = (base / 4, base);
        let mut best = composed.clone_lines_only();
        while lo + 16 < hi {
            let mid = lo + (hi - lo) / 2;
            let scaled: Vec<i64> = line_widths
                .iter()
                .map(|w| (w * mid / base).max(1))
                .collect();
            match compose_at(&chars, widths, hyphen_w, params, &scaled, drop_cap_width) {
                Ok(c) if c.lines.len() == count && c.overfull == 0 => {
                    best = c.clone_lines_only();
                    hi = mid;
                }
                _ => lo = mid,
            }
        }
        composed.lines = best;
    }
    // Optical margin alignment / hanging punctuation.
    if params.optical_margin || params.hanging_punctuation {
        for line in &mut composed.lines {
            if line.end <= line.start {
                continue;
            }
            let first = chars[line.start];
            let last = chars[line.end - 1];
            let full = params.hanging_punctuation;
            line.left_hang = widths[line.start] * hang_percent(first, true, full) / 100;
            if !line.hyphenated {
                line.right_hang = widths[line.end - 1] * hang_percent(last, false, full) / 100;
            }
            line.x -= line.left_hang;
            line.width += line.left_hang + line.right_hang;
        }
    }
    composed.drop_cap_width = drop_cap_width;
    Ok(composed)
}

impl Composed {
    fn clone_lines_only(&self) -> Vec<Line> {
        self.lines.clone()
    }
}

fn compose_at(
    chars: &[char],
    widths: &[i64],
    hyphen_w: i64,
    params: &Params,
    line_widths: &[i64],
    drop_cap_width: i64,
) -> Result<Composed> {
    let mut unavailable =
        params.hyphenation.enabled && pattern_set(&params.language).and_then(patterns).is_none();
    type Found = (Vec<usize>, Vec<Item>, i64, usize, u8);
    let mut result: Option<Found> = None;
    let want_hyphen = params.hyphenation.enabled && !params.single_line;
    let passes: &[(bool, bool, u8)] = if params.single_line {
        &[(false, false, 1)]
    } else if want_hyphen {
        &[(false, false, 1), (true, false, 2), (true, true, 3)]
    } else {
        &[(false, false, 1), (false, true, 3)]
    };
    let mut kept_items = None;
    for &(hyphenate, emergency, pass) in passes {
        let (items, na) = build_items(chars, widths, hyphen_w, params, hyphenate);
        unavailable |= na;
        let tot = totals(&items);
        let plan = Plan {
            items: &items,
            tot: &tot,
            widths: line_widths,
            params,
        };
        if params.single_line {
            let breaks = first_fit(&plan);
            result = Some((breaks, items, 0, 0, pass));
            break;
        }
        let tolerance = if emergency { 10_000 } else { params.tolerance };
        let limit = params.hyphenation.max_consecutive.max(1);
        if let Some((breaks, demerits, hyphens)) = total_fit(&plan, tolerance, emergency, limit) {
            result = Some((breaks, items, demerits, hyphens, pass));
            break;
        }
        kept_items = Some(items);
    }
    let Some((breaks, items, demerits, hyphens, pass)) = result else {
        drop(kept_items);
        bail!("[compose-failed] the paragraph could not be broken into lines");
    };
    let tot = totals(&items);
    let plan = Plan {
        items: &items,
        tot: &tot,
        widths: line_widths,
        params,
    };
    let mut lines = Vec::new();
    let mut start_item = 0usize;
    let mut overfull = 0;
    let mut hyphenated_lines = 0;
    for (index, &b) in breaks.iter().enumerate() {
        let start_item_real = {
            let mut s = start_item;
            while s < b && !matches!(items[s], Item::Box { .. }) {
                s += 1;
            }
            s
        };
        let (start_char, end_char, hyphen) = {
            let first = match items.get(start_item_real) {
                Some(Item::Box { start, .. }) => *start,
                _ => match &items[b] {
                    Item::Glue { pos, .. } | Item::Penalty { pos, .. } => *pos,
                    Item::Box { start, .. } => *start,
                },
            };
            let (end, hyph) = match &items[b] {
                Item::Glue { pos, .. } => (*pos, false),
                Item::Penalty { pos, flagged, .. } => (*pos, *flagged),
                Item::Box { end, .. } => (*end, false),
            };
            (first, end.max(first), hyph)
        };
        let (natural, stretch, shrink) = plan.flex(start_item_real, b, index);
        let target = plan.target(index);
        let is_last = index == breaks.len() - 1;
        let hard_end = matches!(items[b], Item::Penalty { p, .. } if p <= FORCED);
        let ends_paragraph = is_last || hard_end;
        // Letter and glyph adjustments only when justifying this line.
        let justify_line = match params.align {
            Align::Justify
            | Align::JustifyLastLeft
            | Align::JustifyLastCenter
            | Align::JustifyLastRight => !ends_paragraph,
            Align::JustifyLastAll => true,
            _ => false,
        };
        let f = fit(natural, stretch, shrink, target, true);
        let is_overfull = natural - shrink > target && start_char < end_char;
        if is_overfull {
            overfull += 1;
        }
        if hyphen {
            hyphenated_lines += 1;
        }
        let mut width = natural;
        let (mut wsp, mut lsp, mut gsp) = (params.word_spacing.desired, 0i64, 100i64);
        let ratio_milli = if f.ratio.den == 0 {
            0
        } else {
            (f.ratio.num * 1000 / f.ratio.den).clamp(-1_000_000, 1_000_000) as i64
        };
        if justify_line && natural != target {
            let excess = target - natural;
            // Word spacing first, then letter spacing, then glyph scaling.
            let glue_stretch = tot.y[b] - tot.y[start_item_real];
            let glue_shrink = tot.z[b] - tot.z[start_item_real];
            let glue_w: i64 = (start_item_real..b)
                .map(|k| match items[k] {
                    Item::Glue { w, .. } => w,
                    _ => 0,
                })
                .sum();
            let letters = tot.l[b] - tot.l[start_item_real];
            let text_w = tot.t[b] - tot.t[start_item_real];
            let em = params.size_lu;
            let mut rest = excess;
            let word_take;
            if excess > 0 {
                word_take = rest.min(glue_stretch.min(INF / 2));
                rest -= word_take;
                let l_cap = letters
                    * em
                    * (params.letter_spacing.max - params.letter_spacing.desired).max(0)
                    / 100;
                let l_take = rest.min(l_cap);
                rest -= l_take;
                if letters > 0 {
                    lsp = l_take / letters;
                }
                let g_cap =
                    text_w * (params.glyph_scaling.max - params.glyph_scaling.desired).max(0) / 100;
                let g_take = rest.min(g_cap);
                rest -= g_take;
                if text_w > 0 {
                    gsp = 100 + g_take * 100 / text_w;
                }
            } else {
                word_take = -((-rest).min(glue_shrink));
                rest -= word_take;
                let l_cap = letters
                    * em
                    * (params.letter_spacing.desired - params.letter_spacing.min).max(0)
                    / 100;
                let l_take = (-rest).min(l_cap);
                rest += l_take;
                if letters > 0 {
                    lsp = -(l_take / letters);
                }
                let g_cap =
                    text_w * (params.glyph_scaling.desired - params.glyph_scaling.min).max(0) / 100;
                let g_take = (-rest).min(g_cap);
                rest += g_take;
                if text_w > 0 {
                    gsp = 100 - g_take * 100 / text_w;
                }
            }
            if glue_w > 0 {
                wsp = params.word_spacing.desired * (glue_w + word_take) / glue_w;
            }
            width = target - rest;
        }
        // Horizontal placement within the column.
        let left = params.left_indent + if index == 0 { params.first_indent } else { 0 };
        let drop_pad = match &params.drop_cap {
            Some(d) if index < d.lines => drop_cap_width,
            _ => 0,
        };
        let avail = target;
        let align = match params.align {
            Align::Justify | Align::JustifyLastLeft => {
                if ends_paragraph {
                    Align::Left
                } else {
                    Align::Justify
                }
            }
            Align::JustifyLastCenter => {
                if ends_paragraph {
                    Align::Center
                } else {
                    Align::Justify
                }
            }
            Align::JustifyLastRight => {
                if ends_paragraph {
                    Align::Right
                } else {
                    Align::Justify
                }
            }
            other => other,
        };
        let line_left = left + drop_pad;
        let x = match align {
            Align::Right => line_left + (avail - width).max(0),
            Align::Center => line_left + (avail - width).max(0) / 2,
            _ => line_left,
        };
        // The last line's own indent shifts it right and never overflows.
        let x = if ends_paragraph && params.last_indent > 0 {
            x + params.last_indent.min((avail - width).max(0))
        } else {
            x
        };
        lines.push(Line {
            start: start_char,
            end: end_char,
            x,
            width,
            natural,
            ratio_milli,
            hyphenated: hyphen,
            overfull: is_overfull,
            word_spacing_pct: wsp,
            letter_spacing_lu: lsp,
            glyph_scale_pct: gsp,
            left_hang: 0,
            right_hang: 0,
        });
        start_item = plan.next_start(b);
        if start_item >= items.len() {
            break;
        }
    }
    Ok(Composed {
        lines,
        passes: pass,
        demerits,
        hyphens: hyphens.max(hyphenated_lines),
        overfull,
        hyphenation_unavailable: unavailable,
        drop_cap_width: 0,
    })
}

// ------------------------------------------------------------------- keeps

/// How many of a paragraph's `total` lines to place in a container that has
/// room for `available` more lines, honoring orphan/widow control and keep-
/// together. `Some(n)` places `n` lines (`n == total` means all); `None` moves
/// the whole paragraph to the next container.
pub fn split_lines(
    total: usize,
    available: usize,
    orphans: usize,
    widows: usize,
    keep_together: bool,
) -> Option<usize> {
    if total <= available {
        return Some(total);
    }
    if keep_together {
        return None;
    }
    let orphans = orphans.max(1);
    let widows = widows.max(1);
    if total < orphans + widows {
        // Cannot satisfy both: keep the paragraph whole.
        return None;
    }
    let mut take = available;
    if take < orphans {
        return None;
    }
    if total - take < widows {
        take = total - widows;
        if take < orphans {
            return None;
        }
    }
    Some(take)
}

// -------------------------------------------------------------------- driver

/// Style properties that influence composition, read from a merged style map.
pub fn params_from_style(style: &Map<String, Value>, width_lu: i64) -> Result<Params> {
    let num = |key: &str| style.get(key).and_then(Value::as_f64);
    let size = num("size").unwrap_or(12.0);
    let size_lu = shaping::to_lu(size);
    let mut p = Params::new(width_lu, size_lu);
    match style.get("leading") {
        Some(Value::Number(n)) => {
            p.leading_lu = shaping::to_lu(n.as_f64().unwrap_or(size * 1.2));
        }
        _ => p.leading_lu = size_lu * 12 / 10,
    }
    if let Some(a) = style.get("align").and_then(Value::as_str) {
        p.align = Align::parse(a)?;
        if a == "justify" {
            p.align = Align::JustifyLastLeft;
        }
    }
    p.single_line = style.get("composer").and_then(Value::as_str) == Some("single-line");
    p.left_indent = shaping::to_lu(num("left_indent").unwrap_or(0.0));
    p.right_indent = shaping::to_lu(num("right_indent").unwrap_or(0.0));
    p.first_indent = shaping::to_lu(num("first_indent").unwrap_or(0.0));
    p.last_indent = shaping::to_lu(num("last_indent").unwrap_or(0.0));
    let triple = |key: &str, default: Range3| -> Result<Range3> {
        match style.get(key) {
            None => Ok(default),
            Some(v) => {
                let a = v.as_array().filter(|a| a.len() == 3).with_context(|| {
                    format!("[invalid-input] `{key}` must be [min, desired, max]")
                })?;
                let n: Vec<i64> = a.iter().filter_map(Value::as_i64).collect();
                if n.len() != 3 || !(n[0] <= n[1] && n[1] <= n[2]) {
                    bail!(
                        "[invalid-input] `{key}` must be three integers with min <= desired <= max"
                    );
                }
                Ok(Range3 {
                    min: n[0],
                    desired: n[1],
                    max: n[2],
                })
            }
        }
    };
    p.word_spacing = triple("word_spacing", p.word_spacing)?;
    p.letter_spacing = triple("letter_spacing", p.letter_spacing)?;
    p.glyph_scaling = triple("glyph_scaling", p.glyph_scaling)?;
    if p.word_spacing.min < 1 || p.glyph_scaling.min < 1 {
        bail!("[invalid-input] word spacing and glyph scaling minimums must be at least 1 percent");
    }
    if let Some(lang) = style.get("language").and_then(Value::as_str) {
        p.language = lang.to_string();
    }
    p.hyphenation.enabled = style
        .get("hyphenate")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if let Some(h) = style.get("hyphenation").and_then(Value::as_object) {
        let get = |k: &str, d: usize| h.get(k).and_then(Value::as_u64).map_or(d, |v| v as usize);
        p.hyphenation.min_word = get("min_word", 5);
        p.hyphenation.min_before = get("min_before", 2).max(1);
        p.hyphenation.min_after = get("min_after", 2).max(1);
        p.hyphenation.max_consecutive = get("max_consecutive", 3);
        p.hyphenation.capitalized = h
            .get("capitalized")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        p.hyphenation.last_word = h.get("last_word").and_then(Value::as_bool).unwrap_or(true);
    }
    p.optical_margin = style
        .get("optical_margin")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    p.hanging_punctuation = style
        .get("hanging_punctuation")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    p.balance = style
        .get("balance")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if let Some(d) = style.get("drop_cap").and_then(Value::as_object) {
        let lines = d.get("lines").and_then(Value::as_u64).unwrap_or(0) as usize;
        let chars = d.get("chars").and_then(Value::as_u64).unwrap_or(1) as usize;
        if !(2..=20).contains(&lines) || !(1..=10).contains(&chars) {
            bail!("[invalid-input] drop_cap needs lines 2-20 and chars 1-10");
        }
        p.drop_cap = Some(DropCap { lines, chars });
    }
    Ok(p)
}

/// Cap height of `font` as a fraction of an em, in 1/1000.
fn cap_height_milli(font: &ResolvedFont) -> i64 {
    let face = rustybuzz::ttf_parser::Face::parse(&font.bytes, font.index).ok();
    face.and_then(|f| {
        f.capital_height()
            .map(|h| i64::from(h) * 1000 / i64::from(f.units_per_em()))
    })
    .unwrap_or(700)
}

pub struct Request<'a> {
    pub raw: &'a Value,
    pub text: &'a str,
    pub style: Map<String, Value>,
    pub family: String,
    pub weight: u16,
    pub italic: bool,
    pub width_lu: i64,
    pub kerning: Kerning,
    pub features: Vec<(String, u32)>,
}

pub struct Outcome {
    pub composed: Composed,
    pub font: ResolvedFont,
    pub params: Params,
}

/// Shape and compose one paragraph of uniformly styled text.
pub fn compose_paragraph(request: &Request) -> Result<Outcome> {
    let mut params = params_from_style(&request.style, request.width_lu)?;
    if let Some(dictionary) = request
        .raw
        .pointer("/layout/hyphenation/exceptions")
        .and_then(Value::as_object)
    {
        let tag = pattern_set(&params.language).unwrap_or("");
        for (lang, words) in dictionary {
            if pattern_set(lang).unwrap_or(lang.as_str()) != tag && lang != &params.language {
                continue;
            }
            for word in words
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                if let Some((w, points)) = parse_exception(word) {
                    params.hyphenation.exceptions.insert(w, points);
                }
            }
        }
    }
    let font = shaping::resolve(request.raw, &request.family, request.weight, request.italic)?;
    let shape = ShapeParams {
        size_pt: params.size_lu as f64 / shaping::LU_PER_PT as f64,
        features: request.features.clone(),
        language: Some(params.language.clone()),
        kerning: request.kerning,
        ..ShapeParams::default()
    };
    let widths = char_widths(&font, request.text, &shape)?;
    let hyphen = shaping::shape(&font, "-", &shape)?;
    let hyphen_w = hyphen
        .runs
        .iter()
        .flat_map(|r| &r.glyphs)
        .map(|g| g.x_advance)
        .sum();
    let mut drop_cap_width = 0;
    if let Some(d) = &params.drop_cap {
        let cap = cap_height_milli(&font).max(1);
        let span = (d.lines as i64 - 1) * params.leading_lu + params.size_lu * cap / 1000;
        let big = (span * 1000 / cap).max(1);
        let chars: String = request.text.chars().take(d.chars).collect();
        let big_shape = ShapeParams {
            size_pt: big as f64 / shaping::LU_PER_PT as f64,
            ..ShapeParams::default()
        };
        let shaped = shaping::shape(&font, &chars, &big_shape)?;
        drop_cap_width = shaped
            .runs
            .iter()
            .flat_map(|r| &r.glyphs)
            .map(|g| g.x_advance)
            .sum::<i64>()
            + params.size_lu / 4;
    }
    let mut body_widths = widths;
    let mut body_text: String = request.text.to_string();
    let mut offset = 0usize;
    if let Some(d) = &params.drop_cap {
        // The cap characters leave the flowed text; lines are re-based after.
        offset = d.chars.min(body_text.chars().count());
        body_text = body_text.chars().skip(offset).collect();
        body_widths = body_widths.split_off(offset);
    }
    let mut composed = compose(&body_text, &body_widths, hyphen_w, &params, drop_cap_width)?;
    if offset > 0 {
        for line in &mut composed.lines {
            line.start += offset;
            line.end += offset;
        }
    }
    Ok(Outcome {
        composed,
        font,
        params,
    })
}

pub fn line_json(line: &Line) -> Value {
    json!({
        "start": line.start,
        "end": line.end,
        "x_lu": line.x,
        "width_lu": line.width,
        "natural_lu": line.natural,
        "ratio_milli": line.ratio_milli,
        "hyphenated": line.hyphenated,
        "overfull": line.overfull,
        "word_spacing_pct": line.word_spacing_pct,
        "letter_spacing_lu": line.letter_spacing_lu,
        "glyph_scale_pct": line.glyph_scale_pct,
        "left_hang_lu": line.left_hang,
        "right_hang_lu": line.right_hang,
    })
}

/// `text compose`: read-only; works on documents of every version.
pub fn run_compose(raw: &Value, args: &crate::text::ComposeArgs) -> Result<Value> {
    let text = match (&args.text, &args.story) {
        (Some(text), None) => text.clone(),
        (None, Some(story)) => shaping::story_text(raw, story)?,
        _ => bail!("[invalid-input] give exactly one of --text or --story"),
    };
    if args.limit > 2000 {
        bail!("[invalid-input] --limit must be at most 2000");
    }
    if !(args.width.is_finite() && args.width > 0.0 && args.width <= 100_000.0) {
        bail!("[invalid-input] --width must be a positive number of points");
    }
    let mut style = Map::new();
    if let Some(name) = &args.style {
        let chain = crate::stories::paragraph_style(raw, name);
        if chain.is_empty() {
            bail!("[not-found] paragraph style `{name}` does not exist");
        }
        for layer in chain.iter().rev() {
            if let Some(map) = layer.as_object() {
                for (k, v) in map {
                    style.insert(k.clone(), v.clone());
                }
            }
        }
    }
    if let Some(v) = args.size {
        style.insert("size".into(), json!(v));
    }
    if let Some(v) = &args.align {
        style.insert("align".into(), json!(v));
    }
    if let Some(v) = args.hyphenate {
        style.insert("hyphenate".into(), json!(v));
    }
    if let Some(v) = &args.composer {
        style.insert("composer".into(), json!(v));
    }
    if let Some(v) = &args.language {
        style.insert("language".into(), json!(v));
    }
    let family = args
        .font
        .clone()
        .or_else(|| {
            style
                .get("font")
                .and_then(|f| f.as_str().map(str::to_string))
        })
        .unwrap_or_else(|| "Atkinson Hyperlegible".into());
    let width_lu = shaping::to_lu(args.width);
    let mut paragraphs = Vec::new();
    let mut rows = Vec::new();
    let (mut total_lines, mut hyphens, mut overfull) = (0usize, 0usize, 0usize);
    let mut unavailable = false;
    let mut font_report = Value::Null;
    for (index, para) in text.split('\n').enumerate() {
        let request = Request {
            raw,
            text: para,
            style: style.clone(),
            family: family.clone(),
            weight: args.weight,
            italic: args.italic,
            width_lu,
            kerning: Kerning::Metrics,
            features: Vec::new(),
        };
        let out = compose_paragraph(&request)?;
        if font_report.is_null() {
            font_report = shaping::font_json(&out.font);
        }
        let c = &out.composed;
        let hyphenated = c.lines.iter().filter(|l| l.hyphenated).count();
        total_lines += c.lines.len();
        hyphens += hyphenated;
        overfull += c.overfull;
        unavailable |= c.hyphenation_unavailable;
        paragraphs.push(json!({
            "paragraph": index,
            "lines": c.lines.len(),
            "passes": c.passes,
            "demerits": c.demerits,
            "hyphens": hyphenated,
            "overfull": c.overfull,
            "drop_cap_width_lu": c.drop_cap_width,
        }));
        for line in &c.lines {
            let mut row = line_json(line);
            row["paragraph"] = json!(index);
            rows.push(row);
        }
    }
    let returned: Vec<Value> = rows
        .iter()
        .skip(args.offset)
        .take(args.limit)
        .cloned()
        .collect();
    let single = style.get("composer").and_then(Value::as_str) == Some("single-line");
    Ok(json!({
        "engine": ENGINE,
        "composer": if single { "single-line" } else { "paragraph" },
        "width_lu": width_lu,
        "font": font_report,
        "summary": {
            "paragraphs": paragraphs.len(),
            "lines": total_lines,
            "hyphens": hyphens,
            "overfull": overfull,
            "hyphenation_unavailable": unavailable,
        },
        "paragraphs": paragraphs,
        "matches": rows.len(),
        "returned": returned.len(),
        "offset": args.offset,
        "limit": args.limit,
        "has_more": args.offset + returned.len() < rows.len(),
        "lines": returned,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn points(set: &str, word: &str) -> Vec<usize> {
        let p = patterns(set).expect("bundled set");
        let chars: Vec<char> = word.chars().collect();
        hyphenate_word(&p, &chars, &Hyphenation::default())
    }

    #[test]
    fn english_patterns_and_exceptions() {
        assert_eq!(points("en-us", "associate"), [2, 4]);
        assert!(points("en-us", "typographical").contains(&2));
        assert!(points("en-us", "cat").is_empty());
    }

    #[test]
    fn german_patterns_break_compounds() {
        assert!(!points("de-1996", "Donaudampfschiff").is_empty());
    }

    fn flat(text: &str, params: &Params, cap: i64) -> Composed {
        let widths = vec![1000i64; text.chars().count()];
        compose(text, &widths, 500, params, cap).unwrap()
    }

    #[test]
    fn indents_shift_and_narrow_lines() {
        let mut p = Params::new(20_000, 12_000);
        p.first_indent = 4_000;
        p.left_indent = 2_000;
        let c = flat("aaaa bbbb cccc dddd eeee ffff gggg hhhh", &p, 0);
        assert_eq!(c.lines[0].x, 6_000);
        assert_eq!(c.lines[1].x, 2_000);
        assert!(c.lines.iter().all(|l| !l.overfull));
    }

    #[test]
    fn drop_cap_narrows_only_its_lines() {
        let mut p = Params::new(20_000, 12_000);
        p.drop_cap = Some(DropCap { lines: 2, chars: 1 });
        let c = flat("aaaa bbbb cccc dddd eeee ffff gggg hhhh", &p, 6_000);
        assert_eq!(c.lines[0].x, 6_000);
        assert_eq!(c.lines[1].x, 6_000);
        assert_eq!(c.lines[2].x, 0);
        assert_eq!(c.drop_cap_width, 6_000);
    }

    #[test]
    fn hanging_punctuation_moves_margins() {
        let mut p = Params::new(20_000, 12_000);
        p.hanging_punctuation = true;
        let c = flat("\"aaa bbb\" ccc.", &p, 0);
        assert!(c.lines[0].left_hang > 0);
        assert!(c.lines[0].x < 0);
    }

    #[test]
    fn balanced_ragged_lines_keep_the_line_count() {
        let mut p = Params::new(30_000, 12_000);
        let plain = flat("aaaa bbbb cccc dddd eeee ffff gggg hhhh iiii jjjj", &p, 0);
        p.balance = true;
        let balanced = flat("aaaa bbbb cccc dddd eeee ffff gggg hhhh iiii jjjj", &p, 0);
        assert_eq!(plain.lines.len(), balanced.lines.len());
        let spread = |c: &Composed| {
            let w: Vec<i64> = c.lines.iter().map(|l| l.width).collect();
            w.iter().max().unwrap() - w.iter().min().unwrap()
        };
        assert!(spread(&balanced) <= spread(&plain));
    }
}
