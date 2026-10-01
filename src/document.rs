use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    pub format: String,
    pub version: u32,
    pub name: String,
    pub canvas: Canvas,
    pub layers: Vec<Layer>,
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
            version: 1,
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
            }],
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.format != "pentool" || self.version != 1 {
            return Err("unsupported .pen format or version".into());
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
