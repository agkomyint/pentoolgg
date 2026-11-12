//! v0.14.0 task 6: evaluation of nested, line and GREP character styles and of
//! lists (`docs/layout-v1.md` sections 5.3 and 5.4).
//!
//! Nothing here writes the document. Spans and list labels are *computed* from
//! the stories and styles on demand; they are never stored in the text.
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Map, Value};

/// Maximum length of a GREP pattern, in characters.
pub const MAX_PATTERN: usize = 512;
/// Step budget of one match attempt.
pub const STEP_BUDGET: u32 = 100_000;
const MAX_SPANS_PER_PARAGRAPH: usize = 4096;
const MAX_PARAGRAPH_CHARS: usize = 1_000_000;
const MAX_REPEAT: u32 = 1000;

// ---------------------------------------------------------------- regex

#[derive(Debug, Clone)]
enum Item {
    Ch(char),
    Range(char, char),
    Digit(bool),
    Word(bool),
    Space(bool),
    Cat(String, bool),
}

#[derive(Debug, Clone)]
enum Node {
    Ch(char),
    Any,
    Class(bool, Vec<Item>),
    Start,
    End,
    WordB(bool),
    Group(Box<Node>),
    Alt(Vec<Node>),
    Seq(Vec<Node>),
    Repeat {
        node: Box<Node>,
        min: u32,
        max: u32,
        lazy: bool,
    },
    Look(Box<Node>, bool),
}

/// A compiled pattern in the bounded dialect.
#[derive(Debug, Clone)]
pub struct Regex {
    root: Node,
}

struct Parser<'a> {
    chars: Vec<char>,
    at: usize,
    pattern: &'a str,
    depth: usize,
}

const SCRIPTS: &[&str] = &[
    "Latin",
    "Greek",
    "Cyrillic",
    "Han",
    "Hiragana",
    "Katakana",
    "Arabic",
    "Hebrew",
    "Devanagari",
    "Thai",
];
const CATEGORIES: &[&str] = &[
    "L", "Lu", "Ll", "N", "Nd", "P", "S", "Z", "Letter", "Number",
];

fn category(name: &str, c: char) -> bool {
    match name {
        "L" | "Letter" => c.is_alphabetic(),
        "Lu" => c.is_uppercase(),
        "Ll" => c.is_lowercase(),
        "N" | "Nd" | "Number" => c.is_numeric(),
        "P" => {
            c.is_ascii_punctuation() && !"$+<=>^`|~".contains(c) || "\u{2010}\u{2026}".contains(c)
        }
        "S" => "$+<=>^`|~".contains(c),
        "Z" => c.is_whitespace(),
        "Latin" => c.is_alphabetic() && (c.is_ascii() || ('\u{C0}'..='\u{24F}').contains(&c)),
        "Greek" => ('\u{370}'..='\u{3FF}').contains(&c),
        "Cyrillic" => ('\u{400}'..='\u{52F}').contains(&c),
        "Han" => ('\u{4E00}'..='\u{9FFF}').contains(&c) || ('\u{3400}'..='\u{4DBF}').contains(&c),
        "Hiragana" => ('\u{3040}'..='\u{309F}').contains(&c),
        "Katakana" => ('\u{30A0}'..='\u{30FF}').contains(&c),
        "Arabic" => ('\u{600}'..='\u{6FF}').contains(&c),
        "Hebrew" => ('\u{590}'..='\u{5FF}').contains(&c),
        "Devanagari" => ('\u{900}'..='\u{97F}').contains(&c),
        "Thai" => ('\u{E00}'..='\u{E7F}').contains(&c),
        _ => false,
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl<'a> Parser<'a> {
    fn err(&self, why: &str) -> anyhow::Error {
        anyhow::anyhow!(
            "[invalid-input] GREP pattern `{}` {why} at character {}",
            self.pattern,
            self.at
        )
    }
    fn peek(&self) -> Option<char> {
        self.chars.get(self.at).copied()
    }
    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn alt(&mut self) -> Result<Node> {
        let mut branches = vec![self.seq()?];
        while self.eat('|') {
            branches.push(self.seq()?);
        }
        Ok(if branches.len() == 1 {
            branches.pop().unwrap()
        } else {
            Node::Alt(branches)
        })
    }

    fn seq(&mut self) -> Result<Node> {
        let mut items = Vec::new();
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            let atom = self.atom()?;
            items.push(self.quantified(atom)?);
        }
        Ok(Node::Seq(items))
    }

    fn quantified(&mut self, atom: Node) -> Result<Node> {
        let (min, max) = match self.peek() {
            Some('*') => (0, u32::MAX),
            Some('+') => (1, u32::MAX),
            Some('?') => (0, 1),
            Some('{') => {
                let close = self.chars[self.at..]
                    .iter()
                    .position(|&c| c == '}')
                    .ok_or_else(|| self.err("has an unterminated `{`"))?;
                let body: String = self.chars[self.at + 1..self.at + close].iter().collect();
                let parse = |s: &str| s.trim().parse::<u32>().ok();
                let (lo, hi) = match body.split_once(',') {
                    Some((a, b)) if b.trim().is_empty() => (parse(a), Some(u32::MAX)),
                    Some((a, b)) => (parse(a), parse(b)),
                    None => (parse(&body), parse(&body)),
                };
                let (lo, hi) = lo
                    .zip(hi)
                    .ok_or_else(|| self.err("has a malformed `{m,n}` repeat"))?;
                if hi != u32::MAX {
                    ensure!(
                        hi <= MAX_REPEAT && lo <= hi,
                        "[invalid-input] GREP repeat {{{lo},{hi}}} must satisfy m <= n <= {MAX_REPEAT}"
                    );
                } else {
                    ensure!(
                        lo <= MAX_REPEAT,
                        "[invalid-input] GREP repeat minimum must be at most {MAX_REPEAT}"
                    );
                }
                self.at += close;
                (lo, hi)
            }
            _ => return Ok(atom),
        };
        self.at += 1;
        let lazy = self.eat('?');
        ensure!(
            !matches!(
                atom,
                Node::Start | Node::End | Node::WordB(_) | Node::Look(..)
            ),
            "[invalid-input] GREP pattern `{}` repeats an assertion",
            self.pattern
        );
        if self.peek().is_some_and(|c| matches!(c, '*' | '+' | '{')) {
            return Err(self.err("stacks quantifiers"));
        }
        Ok(Node::Repeat {
            node: Box::new(atom),
            min,
            max,
            lazy,
        })
    }

    fn escape(&mut self) -> Result<Item> {
        let c = self
            .peek()
            .ok_or_else(|| self.err("ends with a lone `\\`"))?;
        self.at += 1;
        Ok(match c {
            'd' => Item::Digit(true),
            'D' => Item::Digit(false),
            'w' => Item::Word(true),
            'W' => Item::Word(false),
            's' => Item::Space(true),
            'S' => Item::Space(false),
            'n' => Item::Ch('\n'),
            't' => Item::Ch('\t'),
            'r' => Item::Ch('\r'),
            'p' | 'P' => {
                ensure!(
                    self.eat('{'),
                    "[invalid-input] GREP `\\{c}` needs `{{Name}}`"
                );
                let close = self.chars[self.at..]
                    .iter()
                    .position(|&c| c == '}')
                    .ok_or_else(|| self.err("has an unterminated `\\p{`"))?;
                let name: String = self.chars[self.at..self.at + close].iter().collect();
                self.at += close + 1;
                ensure!(
                    CATEGORIES.contains(&name.as_str()) || SCRIPTS.contains(&name.as_str()),
                    "[invalid-input] GREP property `{name}` is not supported; use one of {} or {}",
                    CATEGORIES.join(", "),
                    SCRIPTS.join(", ")
                );
                Item::Cat(name, c == 'p')
            }
            '1'..='9' => bail!(
                "[invalid-input] GREP back-references are not supported (`\\{c}` in `{}`)",
                self.pattern
            ),
            c if c.is_ascii_alphabetic() => bail!(
                "[invalid-input] GREP escape `\\{c}` is not supported in `{}`",
                self.pattern
            ),
            c => Item::Ch(c),
        })
    }

    fn class(&mut self) -> Result<Node> {
        let negate = self.eat('^');
        let mut items = Vec::new();
        loop {
            let c = self
                .peek()
                .ok_or_else(|| self.err("has an unterminated `[`"))?;
            self.at += 1;
            if c == ']' && !items.is_empty() {
                break;
            }
            let item = if c == '\\' {
                self.escape()?
            } else {
                Item::Ch(c)
            };
            if let Item::Ch(lo) = item {
                if self.peek() == Some('-')
                    && self.chars.get(self.at + 1).is_some_and(|&n| n != ']')
                {
                    self.at += 1;
                    let mut hi = self.peek().unwrap();
                    self.at += 1;
                    if hi == '\\' {
                        match self.escape()? {
                            Item::Ch(h) => hi = h,
                            _ => return Err(self.err("has a class escape as a range end")),
                        }
                    }
                    ensure!(
                        lo <= hi,
                        "[invalid-input] GREP range `{lo}-{hi}` is reversed"
                    );
                    items.push(Item::Range(lo, hi));
                    continue;
                }
            }
            items.push(item);
        }
        Ok(Node::Class(negate, items))
    }

    fn atom(&mut self) -> Result<Node> {
        let c = self.peek().unwrap();
        self.at += 1;
        Ok(match c {
            '.' => Node::Any,
            '^' => Node::Start,
            '$' => Node::End,
            '[' => self.class()?,
            '(' => {
                self.depth += 1;
                ensure!(
                    self.depth <= 16,
                    "[limit-exceeded] GREP pattern nests groups deeper than 16"
                );
                let node = if self.eat('?') {
                    if self.eat(':') {
                        Node::Group(Box::new(self.alt()?))
                    } else if self.eat('=') {
                        Node::Look(Box::new(self.alt()?), false)
                    } else if self.eat('!') {
                        Node::Look(Box::new(self.alt()?), true)
                    } else {
                        bail!(
                            "[invalid-input] GREP look-behind, named groups and inline flags are \
                             not supported in `{}`",
                            self.pattern
                        );
                    }
                } else {
                    Node::Group(Box::new(self.alt()?))
                };
                ensure!(
                    self.eat(')'),
                    "[invalid-input] GREP pattern `{}` has an unclosed `(`",
                    self.pattern
                );
                self.depth -= 1;
                node
            }
            '\\' => match self.peek() {
                Some('b') => {
                    self.at += 1;
                    Node::WordB(true)
                }
                Some('B') => {
                    self.at += 1;
                    Node::WordB(false)
                }
                _ => match self.escape()? {
                    Item::Ch(c) => Node::Ch(c),
                    item => Node::Class(false, vec![item]),
                },
            },
            '*' | '+' | '?' | '{' => {
                self.at -= 1;
                return Err(self.err("has a quantifier with nothing to repeat"));
            }
            c => Node::Ch(c),
        })
    }
}

fn unbounded(node: &Node) -> bool {
    match node {
        Node::Repeat { max, node, .. } => *max == u32::MAX || unbounded(node),
        Node::Group(n) | Node::Look(n, _) => unbounded(n),
        Node::Alt(v) | Node::Seq(v) => v.iter().any(unbounded),
        _ => false,
    }
}

/// Reject `(…*)*`-style nesting, the shape that explodes under backtracking.
fn nested_unbounded(node: &Node) -> bool {
    match node {
        Node::Repeat { node, max, .. } => {
            (*max == u32::MAX && unbounded(node)) || nested_unbounded(node)
        }
        Node::Group(n) | Node::Look(n, _) => nested_unbounded(n),
        Node::Alt(v) | Node::Seq(v) => v.iter().any(nested_unbounded),
        _ => false,
    }
}

impl Regex {
    /// Compile `pattern`, refusing anything outside the documented dialect.
    pub fn new(pattern: &str) -> Result<Regex> {
        ensure!(
            !pattern.is_empty(),
            "[invalid-input] a GREP pattern cannot be empty"
        );
        ensure!(
            pattern.chars().count() <= MAX_PATTERN,
            "[limit-exceeded] a GREP pattern is limited to {MAX_PATTERN} characters"
        );
        let mut p = Parser {
            chars: pattern.chars().collect(),
            at: 0,
            pattern,
            depth: 0,
        };
        let root = p.alt()?;
        if p.at < p.chars.len() {
            return Err(p.err("has an unmatched `)`"));
        }
        ensure!(
            !nested_unbounded(&root),
            "[grep-budget-exceeded] GREP pattern `{pattern}` nests unbounded repeats and could \
             exceed the {STEP_BUDGET}-step budget; use a bounded `{{m,n}}`"
        );
        Ok(Regex { root })
    }

    /// Non-overlapping matches as `[start, end)` character offsets.
    pub fn find_all(&self, text: &[char]) -> Result<Vec<(usize, usize)>> {
        let mut out = Vec::new();
        let mut from = 0;
        while from <= text.len() {
            let mut m = Matcher {
                text,
                steps: 0,
                depth: 0,
                over: false,
            };
            let mut end = None;
            let ok = m.node(&self.root, from, &mut |_, p| {
                end = Some(p);
                true
            });
            if m.over {
                bail!(
                    "[grep-budget-exceeded] a GREP match attempt at character {from} exceeded \
                     {STEP_BUDGET} steps; no partial styling was applied"
                );
            }
            match (ok, end) {
                (true, Some(e)) if e > from => {
                    out.push((from, e));
                    from = e;
                }
                _ => from += 1,
            }
            ensure!(
                out.len() <= MAX_SPANS_PER_PARAGRAPH,
                "[limit-exceeded] a GREP style matched more than {MAX_SPANS_PER_PARAGRAPH} times in one paragraph"
            );
        }
        Ok(out)
    }
}

/// Nested match frames allowed before an attempt is abandoned; keeps recursion
/// well inside the stack of any thread, including test threads.
const MAX_DEPTH: u32 = 1500;

struct Matcher<'t> {
    text: &'t [char],
    steps: u32,
    depth: u32,
    over: bool,
}

type Cont<'k> = &'k mut dyn FnMut(&mut Matcher, usize) -> bool;

impl Matcher<'_> {
    fn word_at(&self, i: usize) -> bool {
        self.text.get(i).copied().is_some_and(is_word)
    }

    fn item(item: &Item, c: char) -> bool {
        match item {
            Item::Ch(x) => *x == c,
            Item::Range(a, b) => (*a..=*b).contains(&c),
            Item::Digit(yes) => c.is_ascii_digit() == *yes,
            Item::Word(yes) => is_word(c) == *yes,
            Item::Space(yes) => c.is_whitespace() == *yes,
            Item::Cat(name, yes) => category(name, c) == *yes,
        }
    }

    fn seq(&mut self, nodes: &[Node], i: usize, pos: usize, k: Cont) -> bool {
        if i == nodes.len() {
            return k(self, pos);
        }
        self.node(&nodes[i], pos, &mut |m, p| m.seq(nodes, i + 1, p, k))
    }

    #[allow(clippy::too_many_arguments)]
    fn rep(
        &mut self,
        node: &Node,
        min: u32,
        max: u32,
        lazy: bool,
        count: u32,
        pos: usize,
        k: Cont,
    ) -> bool {
        if self.over {
            return false;
        }
        let more = |m: &mut Matcher, k: Cont| {
            count < max
                && m.node(node, pos, &mut |m, p| {
                    if p == pos && count >= min {
                        return false;
                    }
                    m.rep(node, min, max, lazy, count + 1, p, k)
                })
        };
        if lazy {
            if count >= min && k(self, pos) {
                return true;
            }
            more(self, k)
        } else {
            if more(self, k) {
                return true;
            }
            count >= min && k(self, pos)
        }
    }

    /// Whether `node` is a single-character matcher and matches `c`.
    fn single(&self, node: &Node, c: char) -> Option<bool> {
        match node {
            Node::Ch(x) => Some(*x == c),
            Node::Any => Some(c != '\n'),
            Node::Class(neg, items) => Some(items.iter().any(|i| Self::item(i, c)) != *neg),
            _ => None,
        }
    }

    fn node(&mut self, node: &Node, pos: usize, k: Cont) -> bool {
        if self.over {
            return false;
        }
        self.steps += 1;
        self.depth += 1;
        if self.steps > STEP_BUDGET || self.depth > MAX_DEPTH {
            self.over = true;
            self.depth -= 1;
            return false;
        }
        let hit = self.node_inner(node, pos, k);
        self.depth -= 1;
        hit
    }

    fn node_inner(&mut self, node: &Node, pos: usize, k: Cont) -> bool {
        match node {
            Node::Ch(c) => self.text.get(pos) == Some(c) && k(self, pos + 1),
            Node::Any => pos < self.text.len() && self.text[pos] != '\n' && k(self, pos + 1),
            Node::Class(neg, items) => match self.text.get(pos) {
                Some(&c) => items.iter().any(|i| Self::item(i, c)) != *neg && k(self, pos + 1),
                None => false,
            },
            Node::Start => (pos == 0 || self.text[pos - 1] == '\n') && k(self, pos),
            Node::End => (pos == self.text.len() || self.text[pos] == '\n') && k(self, pos),
            Node::WordB(want) => {
                let at = (pos > 0 && self.word_at(pos - 1)) != self.word_at(pos);
                at == *want && k(self, pos)
            }
            Node::Group(n) => self.node(n, pos, k),
            Node::Alt(branches) => branches.iter().any(|b| self.node(b, pos, k)),
            Node::Seq(nodes) => self.seq(nodes, 0, pos, k),
            Node::Repeat {
                node,
                min,
                max,
                lazy,
            } => {
                // Single-character bodies run iteratively: no recursion per character.
                if self.single(node, ' ').is_some() {
                    let mut n = 0usize;
                    let cap = (*max as usize).min(self.text.len() - pos.min(self.text.len()));
                    while n < cap && self.single(node, self.text[pos + n]) == Some(true) {
                        n += 1;
                    }
                    self.steps = self.steps.saturating_add(n as u32);
                    if self.steps > STEP_BUDGET {
                        self.over = true;
                        return false;
                    }
                    if n < *min as usize {
                        return false;
                    }
                    if *lazy {
                        (*min as usize..=n).any(|c| k(self, pos + c))
                    } else {
                        (*min as usize..=n).rev().any(|c| k(self, pos + c))
                    }
                } else {
                    self.rep(node, *min, *max, *lazy, 0, pos, k)
                }
            }
            Node::Look(inner, negate) => {
                let hit = self.node(inner, pos, &mut |_, _| true);
                if self.over {
                    return false;
                }
                hit != *negate && k(self, pos)
            }
        }
    }
}

// ---------------------------------------------------------------- spans

fn paragraph_text(paragraph: &Value) -> Vec<char> {
    paragraph
        .get("runs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|r| r.get("text").and_then(Value::as_str))
        .flat_map(str::chars)
        .collect()
}

fn character_style_exists(raw: &Value, id: &str) -> bool {
    raw.pointer("/layout/styles/character")
        .and_then(Value::as_object)
        .is_some_and(|m| m.contains_key(id))
}

/// Validate the `nested`, `line_styles` and `grep` rules of one style. Called when
/// a style is written so a bad pattern is refused up front.
pub fn validate_rules(key: &str, rules: &Value, what: &str) -> Result<()> {
    let list = rules.as_array().map(Vec::as_slice).unwrap_or_default();
    for (i, rule) in list.iter().enumerate() {
        let at = format!("{what}[{i}]");
        let obj = rule
            .as_object()
            .with_context(|| format!("[invalid-input] {at} must be an object"))?;
        let style = obj.get("style").and_then(Value::as_str);
        ensure!(
            style.is_some_and(|s| !s.is_empty() && s.len() <= 128),
            "[invalid-input] {at} needs a character `style` ID"
        );
        match key {
            "grep" => {
                let pattern = obj
                    .get("pattern")
                    .and_then(Value::as_str)
                    .with_context(|| format!("[invalid-input] {at} needs a `pattern` string"))?;
                Regex::new(pattern).with_context(|| format!("{at}.pattern"))?;
            }
            "nested" => {
                let through = obj.get("through").and_then(Value::as_u64).unwrap_or(1);
                ensure!(
                    (1..=1000).contains(&through),
                    "[invalid-input] {at}.through must be between 1 and 1000"
                );
                let of = obj.get("of").and_then(Value::as_str).unwrap_or("word");
                ensure!(
                    DELIMITERS.contains(&of),
                    "[invalid-input] {at}.of `{of}` must be one of {}",
                    DELIMITERS.join(", ")
                );
            }
            "line_styles" => {
                let lines = obj.get("lines").and_then(Value::as_u64).unwrap_or(1);
                ensure!(
                    (1..=1000).contains(&lines),
                    "[invalid-input] {at}.lines must be between 1 and 1000"
                );
            }
            _ => {}
        }
    }
    Ok(())
}

const DELIMITERS: &[&str] = &[
    "character",
    "word",
    "sentence",
    "tab",
    "forced-break",
    "em-space",
    "en-space",
    "nonbreaking-space",
    "letter",
    "digit",
];

/// Offset just past the `n`th delimiter at or after `from`; the whole remainder
/// when there are fewer (InDesign applies the style to the end of the paragraph).
fn through(text: &[char], from: usize, n: u64, of: &str, inclusive: bool) -> usize {
    let mut seen = 0;
    let mut i = from;
    while i < text.len() {
        let c = text[i];
        let (hit, width) = match of {
            "character" => (true, 1),
            "letter" => (c.is_alphabetic(), 1),
            "digit" => (c.is_numeric(), 1),
            "tab" => (c == '\t', 1),
            "forced-break" => (matches!(c, '\n' | '\u{2028}' | '\u{000B}'), 1),
            "em-space" => (c == '\u{2003}', 1),
            "en-space" => (c == '\u{2002}', 1),
            "nonbreaking-space" => (c == '\u{00A0}', 1),
            // A word ends at the whitespace that follows it.
            "word" => (
                c.is_whitespace() && i > from && !text[i - 1].is_whitespace(),
                1,
            ),
            // A sentence ends at terminal punctuation followed by space or the end.
            "sentence" => (
                matches!(c, '.' | '!' | '?' | '\u{2026}' | '\u{3002}')
                    && text.get(i + 1).is_none_or(|n| n.is_whitespace()),
                1,
            ),
            _ => (false, 1),
        };
        if hit {
            seen += 1;
            if seen == n {
                return if inclusive { i + width } else { i };
            }
        }
        i += 1;
    }
    text.len()
}

/// Character-style spans for one paragraph, in application order (later spans win).
///
/// `line_starts` are the character offsets at which composed lines begin; without
/// them line styles are reported in `skipped` because lines only exist after
/// composition.
pub fn evaluate_paragraph(
    raw: &Value,
    resolved: &Map<String, Value>,
    text: &[char],
    line_starts: Option<&[usize]>,
) -> Result<(Vec<Value>, Vec<Value>)> {
    ensure!(
        text.len() <= MAX_PARAGRAPH_CHARS,
        "[limit-exceeded] a paragraph is limited to {MAX_PARAGRAPH_CHARS} characters for style evaluation"
    );
    let mut spans = Vec::new();
    let mut skipped = Vec::new();
    let known = |style: &str, source: &str, i: usize, skipped: &mut Vec<Value>| {
        if character_style_exists(raw, style) {
            true
        } else {
            skipped.push(
                json!({"source": source, "rule": i, "reason": "missing-style", "style": style}),
            );
            false
        }
    };

    // Nested styles: consecutive rules from the start, optionally cycling.
    if let Some(rules) = resolved.get("nested").and_then(Value::as_array) {
        let mut at = 0usize;
        let mut round = 0;
        'outer: loop {
            let before = at;
            for (i, rule) in rules.iter().enumerate() {
                if at >= text.len() {
                    break 'outer;
                }
                let style = rule["style"].as_str().unwrap_or_default();
                let n = rule.get("through").and_then(Value::as_u64).unwrap_or(1);
                let of = rule.get("of").and_then(Value::as_str).unwrap_or("word");
                let inclusive = rule
                    .get("inclusive")
                    .and_then(Value::as_bool)
                    .unwrap_or(true);
                let end = through(text, at, n, of, inclusive);
                if end > at && known(style, "nested", i, &mut skipped) {
                    spans
                        .push(json!({"start": at, "end": end, "style": style, "source": "nested"}));
                }
                at = end;
            }
            let cycles = rules
                .last()
                .and_then(|r| r.get("repeat"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            round += 1;
            if !cycles || at == before || at >= text.len() || round >= 1000 {
                break;
            }
        }
    }

    // Line styles: successive rules cover successive runs of composed lines.
    if let Some(rules) = resolved.get("line_styles").and_then(Value::as_array) {
        if !rules.is_empty() {
            match line_starts {
                None => skipped.push(json!({
                    "source": "line_styles",
                    "reason": "needs-composition",
                    "rules": rules.len(),
                })),
                Some(starts) => {
                    let mut line = 0usize;
                    for (i, rule) in rules.iter().enumerate() {
                        let style = rule["style"].as_str().unwrap_or_default();
                        let count = rule.get("lines").and_then(Value::as_u64).unwrap_or(1) as usize;
                        if line >= starts.len() {
                            break;
                        }
                        let from = starts[line];
                        let to = starts.get(line + count).copied().unwrap_or(text.len());
                        if to > from && known(style, "line", i, &mut skipped) {
                            spans.push(
                                json!({"start": from, "end": to, "style": style, "source": "line"}),
                            );
                        }
                        line += count;
                    }
                }
            }
        }
    }

    // GREP styles apply last, so they win over nested and line styles.
    if let Some(rules) = resolved.get("grep").and_then(Value::as_array) {
        for (i, rule) in rules.iter().enumerate() {
            let style = rule["style"].as_str().unwrap_or_default();
            let pattern = rule["pattern"].as_str().unwrap_or_default();
            let re = Regex::new(pattern)?;
            for (s, e) in re.find_all(text)? {
                if known(style, "grep", i, &mut skipped) {
                    spans.push(
                        json!({"start": s, "end": e, "style": style, "source": "grep", "rule": i}),
                    );
                }
            }
        }
    }
    Ok((spans, skipped))
}

/// Evaluate the nested, line and GREP styles of every paragraph of `story`.
pub fn evaluate_story(raw: &Value, story: &str, limit: usize, offset: usize) -> Result<Value> {
    ensure!(
        (1..=1000).contains(&limit),
        "[invalid-input] --limit must be between 1 and 1000"
    );
    let story_value = raw
        .pointer("/layout/stories")
        .and_then(Value::as_array)
        .and_then(|s| s.iter().find(|s| s["id"] == story))
        .with_context(|| format!("[not-found] story `{story}` does not exist"))?;
    let paragraphs = story_value
        .get("paragraphs")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut rows = Vec::new();
    let mut styled = 0usize;
    for (index, paragraph) in paragraphs.iter().enumerate() {
        let Some(style) = paragraph.get("style").and_then(Value::as_str) else {
            continue;
        };
        let Ok(resolved) = crate::styles::resolve(raw, "paragraph", style) else {
            continue;
        };
        if !["nested", "line_styles", "grep"].iter().any(|k| {
            resolved
                .get(*k)
                .is_some_and(|v| v.as_array().is_some_and(|a| !a.is_empty()))
        }) {
            continue;
        }
        styled += 1;
        let text = paragraph_text(paragraph);
        let (spans, skipped) = evaluate_paragraph(raw, &resolved, &text, None)
            .with_context(|| format!("story `{story}` paragraph {index} (style `{style}`)"))?;
        rows.push(json!({
            "paragraph": index,
            "style": style,
            "chars": text.len(),
            "spans": spans,
            "skipped": skipped,
        }));
    }
    let page: Vec<Value> = rows.iter().skip(offset).take(limit).cloned().collect();
    Ok(json!({
        "story": story,
        "paragraphs": paragraphs.len(),
        "styled_paragraphs": styled,
        "matches": rows.len(),
        "returned": page.len(),
        "offset": offset,
        "limit": limit,
        "has_more": offset + page.len() < rows.len(),
        "evaluated": page,
    }))
}

// ---------------------------------------------------------------- lists

const NUMBER_STYLES: &[&str] = &["1", "A", "a", "I", "i"];
const RESTARTS: &[&str] = &[
    "after-higher-level",
    "never",
    "after-story",
    "after-section",
];
const MAX_LEVELS: usize = 9;

fn alpha(n: u64, upper: bool) -> String {
    // 1 -> a, 26 -> z, 27 -> aa ...
    let mut n = n;
    let mut out = Vec::new();
    while n > 0 {
        n -= 1;
        let c = (b'a' + (n % 26) as u8) as char;
        out.push(if upper { c.to_ascii_uppercase() } else { c });
        n /= 26;
    }
    out.iter().rev().collect()
}

fn roman(mut n: u64, upper: bool) -> String {
    const T: [(u64, &str); 13] = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut out = String::new();
    for (v, s) in T {
        while n >= v {
            out.push_str(s);
            n -= v;
        }
    }
    if upper {
        out.to_uppercase()
    } else {
        out
    }
}

fn number_text(n: u64, style: &str) -> String {
    match style {
        "A" => alpha(n, true),
        "a" => alpha(n, false),
        "I" => roman(n, true),
        "i" => roman(n, false),
        _ => n.to_string(),
    }
}

/// Validate `layout.lists`: unique IDs, at most 9 levels, known restart policies
/// and number styles.
pub fn validate_lists(raw: &Value) -> Result<()> {
    let Some(lists) = raw.pointer("/layout/lists") else {
        return Ok(());
    };
    let lists = lists
        .as_array()
        .context("[invalid-input] layout.lists must be an array")?;
    ensure!(
        lists.len() <= 256,
        "[limit-exceeded] a document is limited to 256 lists"
    );
    let mut seen = std::collections::BTreeSet::new();
    for list in lists {
        let id = list
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty() && s.len() <= 128)
            .context("[invalid-input] every list needs a nonempty `id`")?;
        ensure!(seen.insert(id), "[invalid-input] duplicate list `{id}`");
        let levels = list
            .get("levels")
            .and_then(Value::as_array)
            .with_context(|| format!("[invalid-input] list `{id}` needs a `levels` array"))?;
        ensure!(
            (1..=MAX_LEVELS).contains(&levels.len()),
            "[invalid-input] list `{id}` needs between 1 and {MAX_LEVELS} levels"
        );
        for (n, level) in levels.iter().enumerate() {
            if let Some(f) = level.get("format") {
                ensure!(
                    f.as_str().is_some_and(|s| s.chars().count() <= 64),
                    "[invalid-input] list `{id}` level {} format must be a string of at most 64 characters",
                    n + 1
                );
            }
            if let Some(r) = level.get("restart").and_then(Value::as_str) {
                ensure!(
                    RESTARTS.contains(&r),
                    "[invalid-input] list `{id}` level {} restart `{r}` must be one of {}",
                    n + 1,
                    RESTARTS.join(", ")
                );
            }
            if let Some(s) = level.get("number_style").and_then(Value::as_str) {
                ensure!(
                    NUMBER_STYLES.contains(&s),
                    "[invalid-input] list `{id}` level {} number_style `{s}` must be one of 1, A, a, I, i",
                    n + 1
                );
            }
            if let Some(s) = level.get("start").and_then(Value::as_u64) {
                ensure!(
                    (1..=100_000).contains(&s),
                    "[invalid-input] list `{id}` level {} start must be between 1 and 100000",
                    n + 1
                );
            }
        }
    }
    Ok(())
}

struct ListState {
    counters: [u64; MAX_LEVELS],
    started: [bool; MAX_LEVELS],
}

fn render_label(
    format: &str,
    style: &str,
    counters: &[u64; MAX_LEVELS],
    level: usize,
    styles: &[String],
) -> String {
    let mut out = String::new();
    let chars: Vec<char> = format.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '^' && i + 1 < chars.len() {
            match chars[i + 1] {
                '#' => out.push_str(&number_text(counters[level], style)),
                't' => out.push('\t'),
                '^' => out.push('^'),
                d @ '1'..='9' => {
                    let at = (d as u8 - b'1') as usize;
                    if at <= level {
                        out.push_str(&number_text(counters[at], &styles[at]));
                    }
                }
                other => {
                    out.push('^');
                    out.push(other);
                }
            }
            i += 2;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// Labels for every list paragraph, in story order. Numbers continue across
/// stories that share a list `id`.
pub fn list_labels(
    raw: &Value,
    only_story: Option<&str>,
    limit: usize,
    offset: usize,
) -> Result<Value> {
    ensure!(
        (1..=1000).contains(&limit),
        "[invalid-input] --limit must be between 1 and 1000"
    );
    validate_lists(raw)?;
    let definitions: Vec<&Value> = raw
        .pointer("/layout/lists")
        .and_then(Value::as_array)
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    let stories = raw
        .pointer("/layout/stories")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    if let Some(s) = only_story {
        ensure!(
            stories.iter().any(|st| st["id"] == s),
            "[not-found] story `{s}` does not exist"
        );
    }
    let mut state: std::collections::BTreeMap<String, ListState> = Default::default();
    let mut rows = Vec::new();
    let mut problems = Vec::new();
    for story in stories {
        let story_id = story["id"].as_str().unwrap_or_default();
        // Restart-after-story: forget counters of lists that restart per story.
        for (id, st) in state.iter_mut() {
            let restarts = definitions
                .iter()
                .find(|d| d["id"] == id.as_str())
                .and_then(|d| d["levels"].as_array())
                .is_some_and(|l| l.iter().any(|v| v["restart"] == "after-story"));
            if restarts {
                *st = ListState {
                    counters: [0; MAX_LEVELS],
                    started: [false; MAX_LEVELS],
                };
            }
        }
        let paragraphs = story
            .get("paragraphs")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        for (index, paragraph) in paragraphs.iter().enumerate() {
            let Some(style) = paragraph.get("style").and_then(Value::as_str) else {
                continue;
            };
            let Ok(resolved) = crate::styles::resolve(raw, "paragraph", style) else {
                continue;
            };
            let Some(spec) = resolved.get("list").and_then(Value::as_object) else {
                continue;
            };
            let Some(list_id) = spec.get("list").and_then(Value::as_str) else {
                continue;
            };
            let Some(definition) = definitions.iter().find(|d| d["id"] == list_id) else {
                problems.push(json!({
                    "story": story_id, "paragraph": index, "list": list_id,
                    "problem": "undefined-list",
                    "action": "define it in layout.lists or fix the style's list.list",
                }));
                continue;
            };
            let levels = definition["levels"].as_array().expect("validated");
            let level = (spec.get("level").and_then(Value::as_u64).unwrap_or(1) as usize)
                .clamp(1, levels.len())
                - 1;
            if only_story.is_some_and(|s| s != story_id) {
                // Still advance the counters so continuation is exact.
            }
            let styles: Vec<String> = levels
                .iter()
                .map(|l| {
                    l.get("number_style")
                        .and_then(Value::as_str)
                        .unwrap_or("1")
                        .to_string()
                })
                .collect();
            let st = state.entry(list_id.to_string()).or_insert(ListState {
                counters: [0; MAX_LEVELS],
                started: [false; MAX_LEVELS],
            });
            let lvl = &levels[level];
            if !st.started[level] {
                st.counters[level] = lvl.get("start").and_then(Value::as_u64).unwrap_or(1);
                st.started[level] = true;
            } else {
                st.counters[level] += 1;
            }
            // A higher level resets every deeper level that restarts after it.
            for (deeper, deeper_level) in levels.iter().enumerate().skip(level + 1) {
                let policy = deeper_level
                    .get("restart")
                    .and_then(Value::as_str)
                    .unwrap_or("after-higher-level");
                if policy == "after-higher-level" {
                    st.started[deeper] = false;
                }
            }
            let format = lvl.get("format").and_then(Value::as_str).unwrap_or("^#.^t");
            let has_number = format.contains("^#") || format.contains('^');
            let style_of = &styles[level];
            let label = render_label(format, style_of, &st.counters, level, &styles);
            if only_story.is_none_or(|s| s == story_id) {
                rows.push(json!({
                    "story": story_id,
                    "paragraph": index,
                    "list": list_id,
                    "level": level + 1,
                    "number": if has_number { json!(st.counters[level]) } else { Value::Null },
                    "label": label,
                }));
            }
        }
    }
    let page: Vec<Value> = rows.iter().skip(offset).take(limit).cloned().collect();
    Ok(json!({
        "matches": rows.len(),
        "returned": page.len(),
        "offset": offset,
        "limit": limit,
        "has_more": offset + page.len() < rows.len(),
        "labels": page,
        "problems": problems,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(p: &str, t: &str) -> Vec<(usize, usize)> {
        let chars: Vec<char> = t.chars().collect();
        Regex::new(p).unwrap().find_all(&chars).unwrap()
    }

    #[test]
    fn literals_classes_and_quantifiers() {
        assert_eq!(all("a+", "caaab a"), vec![(1, 4), (6, 7)]);
        assert_eq!(all(r"\d{2,3}", "1 22 4444"), vec![(2, 4), (5, 8)]);
        assert_eq!(all("[A-Z][a-z]*?", "Hello World"), vec![(0, 1), (6, 7)]);
        assert_eq!(all(r"\bcat\b", "cat concat cat"), vec![(0, 3), (11, 14)]);
        assert_eq!(all("(?:ab|cd)+", "abcdx ab"), vec![(0, 4), (6, 8)]);
    }

    #[test]
    fn lookahead_and_properties() {
        assert_eq!(all("a(?=b)", "ab ac"), vec![(0, 1)]);
        assert_eq!(all("a(?!b)", "ab ac"), vec![(3, 4)]);
        assert_eq!(all(r"\p{Lu}", "aBcD"), vec![(1, 2), (3, 4)]);
        assert_eq!(all(r"\p{Greek}+", "ab αβγ c"), vec![(3, 6)]);
    }

    #[test]
    fn refuses_outside_the_dialect() {
        for bad in [r"(a)\1", "(?<=a)b", "(a*)*", "*a", "(", "a{3,1}", r"\q", ""] {
            assert!(Regex::new(bad).is_err(), "{bad} should be refused");
        }
        assert!(Regex::new(&"a".repeat(513)).is_err());
    }

    #[test]
    fn step_budget_is_enforced_without_partial_results() {
        // Quadratic-ish: bounded repeats of bounded repeats stay legal but can be
        // pushed over the budget by a long hostile input.
        let re = Regex::new("(?:a{1,1000}){1,1000}b").unwrap();
        let text: Vec<char> = "a".repeat(5000).chars().collect();
        let err = re.find_all(&text).unwrap_err().to_string();
        assert!(err.contains("grep-budget-exceeded"), "{err}");
    }

    #[test]
    fn number_formats() {
        assert_eq!(alpha(27, false), "aa");
        assert_eq!(roman(1994, true), "MCMXCIV");
    }
}
