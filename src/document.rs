use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    pub format: String,
    pub version: u32,
    pub name: String,
    pub canvas: Canvas,
    pub layers: Vec<Layer>,
    #[serde(default)]
    pub fonts: Vec<FontAsset>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Canvas {
    pub width: u32,
    pub height: u32,
    pub background: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Layer {
    pub id: String,
    pub name: String,
    #[serde(default = "yes")]
    pub visible: bool,
    #[serde(default)]
    pub locked: bool,
    #[serde(default)]
    pub paths: Vec<Path>,
    #[serde(default)]
    pub texts: Vec<Text>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FontAsset {
    pub id: String,
    pub data: String,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}
impl TextAlign {
    pub fn anchor(self) -> &'static str {
        match self {
            Self::Left => "start",
            Self::Center => "middle",
            Self::Right => "end",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Text {
    pub id: String,
    pub content: String,
    pub x: f64,
    pub y: f64,
    #[serde(default = "default_font")]
    pub font_family: String,
    #[serde(default = "default_size")]
    pub font_size: f64,
    #[serde(default = "default_weight")]
    pub font_weight: u16,
    #[serde(default)]
    pub italic: bool,
    #[serde(default = "default_stroke")]
    pub fill: String,
    #[serde(default)]
    pub align: TextAlign,
    #[serde(default)]
    pub letter_spacing: f64,
    #[serde(default = "default_line_height")]
    pub line_height: f64,
    #[serde(default = "identity")]
    pub transform: [f64; 6],
}
pub fn default_font() -> String {
    crate::fonts::DEFAULT_FAMILY.into()
}
pub fn default_size() -> f64 {
    48.0
}
pub fn default_weight() -> u16 {
    400
}
pub fn default_line_height() -> f64 {
    1.2
}
pub fn identity() -> [f64; 6] {
    [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]
}
impl Text {
    pub fn validate(&self) -> Result<(), String> {
        if self.id.is_empty() || self.font_family.trim().is_empty() || self.font_family.len() > 128
        {
            return Err(
                "text ID and font family must be nonempty; family is limited to 128 bytes".into(),
            );
        }
        if ![
            self.x,
            self.y,
            self.font_size,
            self.letter_spacing,
            self.line_height,
        ]
        .iter()
        .chain(self.transform.iter())
        .all(|v| v.is_finite())
        {
            return Err("text coordinates and typography must be finite".into());
        }
        if !(0.1..=4096.0).contains(&self.font_size)
            || !(0.1..=10.0).contains(&self.line_height)
            || !(100..=900).contains(&self.font_weight)
        {
            return Err(
                "font size must be 0.1–4096, weight 100–900, and line height 0.1–10".into(),
            );
        }
        if self.content.len() > 1_000_000
            || self.content.chars().any(|c| {
                c < ' ' && !matches!(c, '\n' | '\r' | '\t') || c == '\u{fffe}' || c == '\u{ffff}'
            })
        {
            return Err("text exceeds size limit or contains invalid XML characters".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Path {
    pub id: String,
    pub d: String,
    #[serde(default = "default_stroke")]
    pub stroke: String,
    #[serde(default = "default_width")]
    pub stroke_width: f32,
    #[serde(default = "default_fill")]
    pub fill: String,
    #[serde(default)]
    pub closed: bool,
}

fn yes() -> bool {
    true
}
fn default_stroke() -> String {
    "#111827".into()
}
fn default_fill() -> String {
    "none".into()
}
fn default_width() -> f32 {
    3.0
}

impl Document {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            format: "pentool".into(),
            version: 2,
            name: "Untitled".into(),
            canvas: Canvas {
                width,
                height,
                background: "#ffffff".into(),
            },
            layers: vec![Layer {
                id: "layer-1".into(),
                name: "Layer 1".into(),
                visible: true,
                locked: false,
                paths: vec![],
                texts: vec![],
            }],
            fonts: vec![],
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.format != "pentool" || !matches!(self.version, 1 | 2) {
            return Err("unsupported .pen format or version".into());
        }
        if self.version == 1
            && (!self.fonts.is_empty() || self.layers.iter().any(|l| !l.texts.is_empty()))
        {
            return Err("text and embedded fonts require document version 2".into());
        }
        if self.layers.is_empty() {
            return Err("document must contain at least one layer".into());
        }
        if self.canvas.width == 0
            || self.canvas.height == 0
            || self.canvas.width > 16384
            || self.canvas.height > 16384
        {
            return Err("canvas dimensions must be between 1 and 16384".into());
        }
        if self.layers.len() > 1000 {
            return Err("document has too many layers".into());
        }
        if self.layers.iter().flat_map(|l| &l.paths).count() > 100_000 {
            return Err("document has too many paths".into());
        }
        if self.layers.iter().flat_map(|l| &l.texts).count() > 100_000 {
            return Err("document has too many text objects".into());
        }
        for layer in &self.layers {
            let mut ids = std::collections::HashSet::new();
            for id in layer
                .paths
                .iter()
                .map(|p| &p.id)
                .chain(layer.texts.iter().map(|t| &t.id))
            {
                if !ids.insert(id) {
                    return Err(format!("duplicate object ID in layer {}: {}", layer.id, id));
                }
            }
            for text in &layer.texts {
                text.validate()?;
            }
        }
        let mut ids = std::collections::HashSet::new();
        let mut bytes = 0;
        for font in &self.fonts {
            crate::fonts::decode(font).map_err(|e| e.to_string())?;
            if !ids.insert(&font.id) || font.id.is_empty() {
                return Err("embedded font IDs must be unique and nonempty".into());
            }
            bytes += font.data.len();
        }
        if self.fonts.len() > 64 || bytes > 16 * 1024 * 1024 {
            return Err("embedded fonts exceed document limits".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn new_document_round_trips() {
        let d = Document::new(800, 600);
        let encoded = serde_json::to_string(&d).unwrap();
        let decoded: Document = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.canvas.width, 800);
        assert_eq!(decoded.layers.len(), 1);
    }
}
