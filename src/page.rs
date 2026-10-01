use crate::document::{Canvas, Document, Layer, Page};
use anyhow::{bail, Context, Result};
use clap::Subcommand;

#[derive(Debug, Clone, Subcommand)]
pub enum PageAction {
    /// List pages as compact JSON.
    List,
    /// Add an empty page.
    Add {
        id: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long, default_value_t = 1200)]
        width: u32,
        #[arg(long, default_value_t = 800)]
        height: u32,
        #[arg(long, default_value = "#ffffff")]
        background: String,
    },
    /// Rename a page.
    Rename { id: String, name: String },
    /// Duplicate a page and all of its editable content.
    Duplicate { id: String, new_id: String },
    /// Move a page to a zero-based index.
    Move { id: String, index: usize },
    /// Remove a page.
    Remove { id: String },
}

pub fn apply(doc: &mut Document, action: PageAction) -> Result<serde_json::Value> {
    let mut pages = doc.pages();
    match action {
        PageAction::List => Ok(list(doc)),
        PageAction::Add {
            id,
            name,
            width,
            height,
            background,
        } => {
            ensure_new_id(&pages, &id)?;
            pages.push(Page {
                id: id.clone(),
                name: name.unwrap_or_else(|| id.clone()),
                canvas: Canvas {
                    width,
                    height,
                    background,
                },
                layers: vec![Layer {
                    id: "layer-1".into(),
                    name: "Layer 1".into(),
                    visible: true,
                    locked: false,
                    paths: vec![],
                    texts: vec![],
                }],
            });
            doc.replace_pages(pages, &id).map_err(anyhow::Error::msg)?;
            Ok(serde_json::json!({"operation":"add","page":id}))
        }
        PageAction::Rename { id, name } => {
            let page = pages
                .iter_mut()
                .find(|p| p.id == id)
                .context("page not found")?;
            page.name = name;
            doc.replace_pages(pages, &id).map_err(anyhow::Error::msg)?;
            Ok(serde_json::json!({"operation":"rename","page":id}))
        }
        PageAction::Duplicate { id, new_id } => {
            ensure_new_id(&pages, &new_id)?;
            let index = pages
                .iter()
                .position(|p| p.id == id)
                .context("page not found")?;
            let mut copy = pages[index].clone();
            copy.id = new_id.clone();
            copy.name = format!("{} copy", copy.name);
            pages.insert(index + 1, copy);
            doc.replace_pages(pages, &new_id)
                .map_err(anyhow::Error::msg)?;
            Ok(serde_json::json!({"operation":"duplicate","page":id,"new_id":new_id}))
        }
        PageAction::Move { id, index } => {
            if index >= pages.len() {
                bail!("page index out of range");
            }
            let old = pages
                .iter()
                .position(|p| p.id == id)
                .context("page not found")?;
            let page = pages.remove(old);
            pages.insert(index, page);
            doc.replace_pages(pages, &id).map_err(anyhow::Error::msg)?;
            Ok(serde_json::json!({"operation":"move","page":id,"index":index}))
        }
        PageAction::Remove { id } => {
            if pages.len() == 1 {
                bail!("cannot remove the last page");
            }
            let index = pages
                .iter()
                .position(|p| p.id == id)
                .context("page not found")?;
            pages.remove(index);
            let active = pages[index.min(pages.len() - 1)].id.clone();
            doc.replace_pages(pages, &active)
                .map_err(anyhow::Error::msg)?;
            Ok(serde_json::json!({"operation":"remove","page":id,"active_page":active}))
        }
    }
}

pub fn list(doc: &Document) -> serde_json::Value {
    let pages = doc.pages();
    serde_json::json!({
        "pages": pages.iter().enumerate().map(|(index, page)| serde_json::json!({
            "id": page.id,
            "name": page.name,
            "index": index,
            "width": page.canvas.width,
            "height": page.canvas.height,
            "layers": page.layers.len(),
            "objects": page.layers.iter().map(|l| l.paths.len() + l.texts.len()).sum::<usize>()
        })).collect::<Vec<_>>()
    })
}

fn ensure_new_id(pages: &[Page], id: &str) -> Result<()> {
    if id.trim().is_empty() {
        bail!("page ID cannot be empty");
    }
    if pages.iter().any(|p| p.id == id) {
        bail!("page ID already exists");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moving_a_page_preserves_its_canvas_and_objects() {
        let mut doc = Document::new(640, 480);
        apply(
            &mut doc,
            PageAction::Add {
                id: "mobile".into(),
                name: Some("Mobile".into()),
                width: 390,
                height: 844,
                background: "#101010".into(),
            },
        )
        .unwrap();
        doc.layers[0].paths.push(crate::document::Path {
            id: "mark".into(),
            d: "M 0 0 L 10 0 L 0 10 Z".into(),
            stroke: "none".into(),
            stroke_width: 0.0,
            stroke_linecap: crate::document::StrokeCap::Butt,
            stroke_linejoin: crate::document::StrokeJoin::Miter,
            stroke_miterlimit: 4.0,
            fill: "red".into(),
            closed: true,
        });
        apply(
            &mut doc,
            PageAction::Duplicate {
                id: "mobile".into(),
                new_id: "mobile-copy".into(),
            },
        )
        .unwrap();
        apply(
            &mut doc,
            PageAction::Move {
                id: "mobile-copy".into(),
                index: 0,
            },
        )
        .unwrap();
        let pages = doc.pages();
        assert_eq!(pages[0].canvas.width, 390);
        assert_eq!(pages[0].canvas.height, 844);
        assert_eq!(pages[0].layers[0].paths[0].id, "mark");
    }
}
