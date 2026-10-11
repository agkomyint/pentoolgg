//! `pentool paint-media`: list material paint modes and canonicalise a media spec
//! (v0.13.0 task 7). Read-only; needs no document.
use crate::raster::media::{Media, ALGORITHM, KINDS};
use anyhow::{Context, Result};
use serde_json::{json, Value};

#[derive(clap::Subcommand, Debug)]
pub enum MediaAction {
    /// List the media kinds and the parameters each one reads.
    List,
    /// Validate a media object and print its canonical form and digest.
    /// Parameters the kind does not read are rejected, never ignored.
    Describe {
        /// Media JSON or @file.json, e.g. '{"kind":"impasto","light":45}'.
        media: String,
    },
}

/// The media kinds and their defaults.
pub fn list_value() -> Value {
    let kinds: Vec<Value> = KINDS
        .iter()
        .map(|(name, _)| {
            let media = Media::parse(&json!({"kind": name})).expect("default media");
            json!({"kind": name, "defaults": media.to_json()})
        })
        .collect();
    json!({"algorithm": ALGORITHM, "kinds": kinds, "blend": "normal"})
}

/// Canonical form and digest of a media object.
pub fn describe_value(value: &Value) -> Result<Value> {
    let parsed = Media::parse(value)?;
    Ok(json!({
        "algorithm": ALGORITHM,
        "media": parsed.to_json(),
        "digest": parsed.digest(),
    }))
}

pub fn run(action: MediaAction) -> Result<()> {
    let out = match action {
        MediaAction::List => list_value(),
        MediaAction::Describe { media } => {
            let text = match media.strip_prefix('@') {
                Some(path) => std::fs::read_to_string(path)
                    .with_context(|| format!("[missing-resource] cannot read {path}"))?,
                None => media,
            };
            let value: Value =
                serde_json::from_str(&text).context("[invalid-request] media must be JSON")?;
            describe_value(&value)?
        }
    };
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}
