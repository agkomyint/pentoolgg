//! Professional color engine. See `docs/professional-color-v1.md`.
//!
//! This module consumes the photography engine's deterministic math; document
//! integration must use the existing scene and transaction paths.
pub mod bundle;
pub mod catalog;
pub mod editing;
pub(crate) mod gradient;
pub mod icc;
pub mod ink;
pub mod output;
pub mod pcs;
pub mod pdfx;
pub mod preflight;
pub mod production;
pub mod profile;
pub mod proof;
pub mod proofimage;
pub mod recipe;
pub mod roles;
pub mod scene;
pub mod separation;
pub mod spot;
pub mod value;

pub const VERSION: u64 = 8;

/// True for a document at the color format version or any later one, which
/// carries every v8 construct unchanged (version 9 only adds layout).
pub fn has_v8(raw: &serde_json::Value) -> bool {
    raw.get("version")
        .and_then(serde_json::Value::as_u64)
        .is_some_and(|v| v >= VERSION)
}
