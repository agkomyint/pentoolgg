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
