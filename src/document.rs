use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone)]
pub struct Document {
    pub format: String,
    pub version: u32,
    pub name: String,
    pub canvas: Canvas,
    pub layers: Vec<Layer>,
    pub fonts: Vec<FontAsset>,
    pages: Vec<Page>,
    active_page: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page {
    pub id: String,
    pub name: String,
    pub canvas: Canvas,
    pub layers: Vec<Layer>,
}

#[derive(Deserialize)]
struct DocumentWire {
    format: String,
    version: u32,
    name: String,
    #[serde(default)]
    canvas: Option<Canvas>,
    #[serde(default)]
    layers: Option<Vec<Layer>>,
    #[serde(default)]
    pages: Vec<Page>,
    #[serde(default)]
    fonts: Vec<FontAsset>,
}

impl<'de> Deserialize<'de> for Document {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = DocumentWire::deserialize(deserializer)?;
        let (canvas, layers, pages) = if wire.version == 3 {
            let first = wire
                .pages
                .first()
                .ok_or_else(|| serde::de::Error::custom("version 3 document has no pages"))?;
            (first.canvas.clone(), first.layers.clone(), wire.pages)
        } else {
            (
                wire.canvas
                    .ok_or_else(|| serde::de::Error::custom("document has no canvas"))?,
                wire.layers
                    .ok_or_else(|| serde::de::Error::custom("document has no layers"))?,
                vec![],
            )
        };
        Ok(Self {
            format: wire.format,
            version: wire.version,
            name: wire.name,
            canvas,
            layers,
            fonts: wire.fonts,
            pages,
            active_page: 0,
        })
    }
}

impl Serialize for Document {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("Document", 6)?;
        state.serialize_field("format", &self.format)?;
        state.serialize_field("version", &self.version)?;
        state.serialize_field("name", &self.name)?;
        if self.version == 3 {
            let pages = self.current_pages();
            state.serialize_field("pages", &pages)?;
        } else {
            state.serialize_field("canvas", &self.canvas)?;
            state.serialize_field("layers", &self.layers)?;
        }
        state.serialize_field("fonts", &self.fonts)?;
        state.end()
    }
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
    /// Optional text-box width; centred and right-aligned lines anchor inside it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
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
    #[serde(default)]
    pub stroke_linecap: StrokeCap,
    #[serde(default)]
    pub stroke_linejoin: StrokeJoin,
    #[serde(default = "default_miter_limit")]
    pub stroke_miterlimit: f32,
    #[serde(default = "default_fill")]
    pub fill: String,
    #[serde(default)]
    pub closed: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum StrokeCap {
    Butt,
    #[default]
    Round,
    Square,
}
impl StrokeCap {
    pub fn svg(self) -> &'static str {
        match self {
            Self::Butt => "butt",
            Self::Round => "round",
            Self::Square => "square",
        }
    }
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum StrokeJoin {
    Miter,
    #[default]
    Round,
    Bevel,
}
impl StrokeJoin {
    pub fn svg(self) -> &'static str {
        match self {
            Self::Miter => "miter",
            Self::Round => "round",
            Self::Bevel => "bevel",
        }
    }
}
pub fn default_miter_limit() -> f32 {
    4.0
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
        let canvas = Canvas {
            width,
            height,
            background: "#ffffff".into(),
        };
        let layers = vec![Layer {
            id: "layer-1".into(),
            name: "Layer 1".into(),
            visible: true,
            locked: false,
            paths: vec![],
            texts: vec![],
        }];
        Self {
            format: "pentool".into(),
            version: 3,
            name: "Untitled".into(),
            canvas: canvas.clone(),
            layers: layers.clone(),
            fonts: vec![],
            pages: vec![Page {
                id: "page-1".into(),
                name: "Page 1".into(),
                canvas,
                layers,
            }],
            active_page: 0,
        }
    }

    fn current_pages(&self) -> Vec<Page> {
        if self.version != 3 {
            return vec![];
        }
        let mut pages = self.pages.clone();
        if pages.is_empty() {
            pages.push(Page {
                id: "page-1".into(),
                name: "Page 1".into(),
                canvas: self.canvas.clone(),
                layers: self.layers.clone(),
            });
        } else if let Some(page) = pages.get_mut(self.active_page) {
            page.canvas = self.canvas.clone();
            page.layers = self.layers.clone();
        }
        pages
    }

    pub fn pages(&self) -> Vec<Page> {
        if self.version == 3 {
            self.current_pages()
        } else {
            vec![Page {
                id: "page-1".into(),
                name: "Page 1".into(),
                canvas: self.canvas.clone(),
                layers: self.layers.clone(),
            }]
        }
    }

    pub fn active_page_id(&self) -> &str {
        if self.version == 3 {
            self.pages
                .get(self.active_page)
                .map(|p| p.id.as_str())
                .unwrap_or("page-1")
        } else {
            "page-1"
        }
    }

    pub fn select_page(&mut self, id: &str) -> Result<(), String> {
        if self.version != 3 {
            if id == "page-1" {
                return Ok(());
            }
            return Err("legacy documents only expose page-1; upgrade before adding pages".into());
        }
        if let Some(page) = self.pages.get_mut(self.active_page) {
            page.canvas = self.canvas.clone();
            page.layers = self.layers.clone();
        }
        let index = self
            .pages
            .iter()
            .position(|p| p.id == id)
            .ok_or_else(|| format!("page not found: {id}"))?;
        self.active_page = index;
        self.canvas = self.pages[index].canvas.clone();
        self.layers = self.pages[index].layers.clone();
        Ok(())
    }

    pub fn replace_pages(&mut self, pages: Vec<Page>, active_id: &str) -> Result<(), String> {
        let index = pages
            .iter()
            .position(|p| p.id == active_id)
            .ok_or_else(|| format!("page not found: {active_id}"))?;
        self.version = 3;
        self.pages = pages;
        self.active_page = index;
        self.canvas = self.pages[index].canvas.clone();
        self.layers = self.pages[index].layers.clone();
        Ok(())
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.format != "pentool" || !matches!(self.version, 1..=3) {
            return Err("unsupported .pen format or version".into());
        }
        if self.version == 1
            && (!self.fonts.is_empty() || self.layers.iter().any(|l| !l.texts.is_empty()))
        {
            return Err("text and embedded fonts require document version 2".into());
        }
        let pages = self.pages();
        if pages.is_empty() || pages.len() > 1000 {
            return Err("document must contain between 1 and 1000 pages".into());
        }
        let mut page_ids = std::collections::HashSet::new();
        if pages
            .iter()
            .any(|p| p.id.is_empty() || !page_ids.insert(&p.id))
        {
            return Err("page IDs must be unique and nonempty".into());
        }
        if pages.iter().any(|p| p.layers.is_empty()) {
            return Err("each page must contain at least one layer".into());
        }
        if pages.iter().any(|p| {
            p.canvas.width == 0
                || p.canvas.height == 0
                || p.canvas.width > 16384
                || p.canvas.height > 16384
        }) {
            return Err("canvas dimensions must be between 1 and 16384".into());
        }
        if pages.iter().any(|p| p.layers.len() > 1000) {
            return Err("a page has too many layers".into());
        }
        for page in &pages {
            let mut layer_ids = std::collections::HashSet::new();
            if page
                .layers
                .iter()
                .any(|layer| layer.id.is_empty() || !layer_ids.insert(&layer.id))
            {
                return Err(format!(
                    "layer IDs must be unique and nonempty within page {}",
                    page.id
                ));
            }
        }
        if pages
            .iter()
            .flat_map(|p| &p.layers)
            .flat_map(|l| &l.paths)
            .count()
            > 100_000
        {
            return Err("document has too many paths".into());
        }
        if pages
            .iter()
            .flat_map(|p| &p.layers)
            .flat_map(|l| &l.texts)
            .count()
            > 100_000
        {
            return Err("document has too many text objects".into());
        }
        for layer in pages.iter().flat_map(|p| &p.layers) {
            for path in &layer.paths {
                if !path.stroke_width.is_finite()
                    || path.stroke_width < 0.0
                    || !path.stroke_miterlimit.is_finite()
                    || !(1.0..=1000.0).contains(&path.stroke_miterlimit)
                {
                    return Err(
                        "stroke width must be finite/nonnegative and miter limit must be 1–1000"
                            .into(),
                    );
                }
                if self.version == 1
                    && (path.stroke_linecap != StrokeCap::Round
                        || path.stroke_linejoin != StrokeJoin::Round)
                {
                    return Err("custom stroke caps and joins require document version 2".into());
                }
            }
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

    #[test]
    fn legacy_document_loads_as_one_virtual_page() {
        let raw = r##"{"format":"pentool","version":1,"name":"Old","canvas":{"width":10,"height":20,"background":"#fff"},"layers":[{"id":"ink","name":"Ink","paths":[]}]}"##;
        let doc: Document = serde_json::from_str(raw).unwrap();
        doc.validate().unwrap();
        assert_eq!(doc.pages().len(), 1);
        assert_eq!(doc.pages()[0].id, "page-1");
        assert_eq!(doc.layers[0].id, "ink");
    }

    #[test]
    fn page_selection_serializes_edits_to_only_the_selected_page() {
        let mut doc = Document::new(100, 100);
        crate::page::apply(
            &mut doc,
            crate::page::PageAction::Add {
                id: "mobile".into(),
                name: Some("Mobile".into()),
                width: 390,
                height: 844,
                background: "#000000".into(),
            },
        )
        .unwrap();
        doc.layers[0].name = "Mobile art".into();
        doc.select_page("page-1").unwrap();
        assert_eq!(doc.layers[0].name, "Layer 1");
        doc.select_page("mobile").unwrap();
        assert_eq!(doc.layers[0].name, "Mobile art");
        let raw = serde_json::to_value(&doc).unwrap();
        assert!(raw.get("canvas").is_none());
        assert_eq!(raw["pages"][1]["canvas"]["width"], 390);
        assert_eq!(raw["pages"][1]["layers"][0]["name"], "Mobile art");
    }
}
