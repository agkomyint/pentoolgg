//! Reproduce the linked-photograph campaign without modifying its sources.
#[path = "../tests/support/v080_campaign.rs"]
mod campaign;
use anyhow::{bail, Result};
use std::path::Path;
fn main() -> Result<()> {
    let output = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/v080-campaign".into());
    let output = Path::new(&output);
    if output.exists() {
        bail!("choose a new output directory")
    }
    std::fs::create_dir_all(output.join("sources"))?;
    for file in ["earth.jpg", "sunrise.jpg", "horizon.jpg"] {
        std::fs::copy(
            Path::new("examples/v080-campaign/sources").join(file),
            output.join("sources").join(file),
        )?;
    }
    let document = output.join("campaign.pen");
    let raw = campaign::build(&document)?;
    std::fs::write(&document, serde_json::to_vec_pretty(&raw)?)?;
    for page in ["poster", "screen"] {
        std::fs::write(
            output.join(format!("{page}.png")),
            pentool::composite::png(&raw, &document, Some(page), 1.0)?,
        )?;
    }
    let report = pentool::inspect::analyze(&raw, &document, Some("poster"), "page", None, &[])?;
    std::fs::write(
        output.join("analysis.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    println!(
        "{}",
        serde_json::json!({"document":document,"pages":2,"sources":3,"source_pixels":"unchanged"})
    );
    Ok(())
}
