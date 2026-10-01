use crate::document::{Document, FontAsset};
use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use resvg::usvg::fontdb::{Database, Family, Query, Style, Weight};
use serde_json::{json, Value};
use std::sync::OnceLock;

pub const DEFAULT_FAMILY: &str = "Atkinson Hyperlegible";
pub const BUNDLED: [(&str, u16, bool, &[u8]); 4] = [
    (
        "regular",
        400,
        false,
        include_bytes!("../assets/fonts/AtkinsonHyperlegible-Regular.ttf"),
    ),
    (
        "bold",
        700,
        false,
        include_bytes!("../assets/fonts/AtkinsonHyperlegible-Bold.ttf"),
    ),
    (
        "italic",
        400,
        true,
        include_bytes!("../assets/fonts/AtkinsonHyperlegible-Italic.ttf"),
    ),
    (
        "bold-italic",
        700,
        true,
        include_bytes!("../assets/fonts/AtkinsonHyperlegible-BoldItalic.ttf"),
    ),
];

pub fn decode(asset: &FontAsset) -> Result<Vec<u8>> {
    if asset.data.len() > 8 * 1024 * 1024 {
        bail!("embedded font is too large");
    }
    let data = STANDARD
        .decode(&asset.data)
        .context("invalid base64 font data")?;
    let mut db = Database::new();
    db.load_font_data(data.clone());
    if db.faces().next().is_none() {
        bail!("invalid or unsupported font file");
    }
    Ok(data)
}

pub fn database(doc: &Document, system: bool) -> Result<Database> {
    let mut db = Database::new();
    // Embedded fonts take priority over bundled and locally installed fonts.
    for asset in &doc.fonts {
        db.load_font_data(decode(asset)?);
    }
    for (_, _, _, bytes) in BUNDLED {
        db.load_font_data(bytes.to_vec());
    }
    if system {
        static SYSTEM: OnceLock<Database> = OnceLock::new();
        let installed = SYSTEM.get_or_init(|| {
            let mut db = Database::new();
            db.load_system_fonts();
            db
        });
        for face in installed.faces() {
            db.push_face_info(face.clone());
        }
    }
    db.set_sans_serif_family(DEFAULT_FAMILY);
    db.set_serif_family(DEFAULT_FAMILY);
    db.set_monospace_family(DEFAULT_FAMILY);
    db.set_cursive_family(DEFAULT_FAMILY);
    db.set_fantasy_family(DEFAULT_FAMILY);
    Ok(db)
}

pub fn ensure_family(db: &Database, family: &str, weight: u16, italic: bool) -> Result<()> {
    let family = match family {
        "sans-serif" => Family::SansSerif,
        "serif" => Family::Serif,
        "monospace" => Family::Monospace,
        "cursive" => Family::Cursive,
        "fantasy" => Family::Fantasy,
        other => Family::Name(other),
    };
    if db
        .query(&Query {
            families: &[family],
            weight: Weight(weight),
            style: if italic { Style::Italic } else { Style::Normal },
            ..Query::default()
        })
        .is_none()
    {
        bail!("font family is unavailable; embed its TTF/OTF with 'pentool font <file.pen> add <id> --file <font.ttf>'");
    }
    Ok(())
}

pub fn list(doc: &Document) -> Result<Value> {
    let db = database(doc, true)?;
    let faces:Vec<_>=db.faces().map(|f|json!({"family":f.families.first().map(|(name,_)|name),"weight":f.weight.0,"italic":matches!(f.style,Style::Italic|Style::Oblique)})).collect();
    Ok(json!({"default_family":DEFAULT_FAMILY,"faces":faces}))
}

pub fn asset(id: String, bytes: Vec<u8>) -> Result<FontAsset> {
    let asset = FontAsset {
        id,
        data: STANDARD.encode(bytes),
    };
    decode(&asset)?;
    Ok(asset)
}
pub fn css_string(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\'' => "\\27 ".into(),
            '\\' => "\\5c ".into(),
            '&' => "\\26 ".into(),
            '<' => "\\3c ".into(),
            '>' => "\\3e ".into(),
            '\n' | '\r' => " ".into(),
            c => c.to_string(),
        })
        .collect()
}
pub fn svg_faces(doc: &Document) -> Result<String> {
    let mut css = String::from("<style>");
    if doc.layers.iter().all(|l| l.texts.is_empty()) {
        return Ok(String::new());
    }
    for (_, weight, italic, bytes) in BUNDLED {
        css.push_str(&format!("@font-face{{font-family:'{}';font-weight:{};font-style:{};src:url(data:font/ttf;base64,{})}}",DEFAULT_FAMILY,weight,if italic{"italic"}else{"normal"},STANDARD.encode(bytes)));
    }
    for asset in &doc.fonts {
        let bytes = decode(asset)?;
        let mut db = Database::new();
        db.load_font_data(bytes);
        for face in db.faces() {
            if let Some((family, _)) = face.families.first() {
                css.push_str(&format!("@font-face{{font-family:'{}';font-weight:{};font-style:{};src:url(data:font/ttf;base64,{})}}",css_string(family),face.weight.0,if face.style==Style::Normal{"normal"}else{"italic"},asset.data));
            }
        }
    }
    css.push_str("</style>");
    Ok(css)
}
