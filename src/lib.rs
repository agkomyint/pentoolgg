//! Pentool: the command-line, server and networked-collaboration layer on top of
//! [`pentool_core`]. Every core module is re-exported here under its original
//! path, so `pentool::scene`, `pentool::render` and the rest keep working for the
//! binary, the integration tests, fuzz targets and the browser build.
//! See `docs/architecture.md`.

pub use pentool_core::*;

#[cfg(not(target_arch = "wasm32"))]
pub mod ai;
pub mod collab;
#[cfg(not(target_arch = "wasm32"))]
pub mod extension;
#[cfg(not(target_arch = "wasm32"))]
pub mod server;
