// Grouped integration suite 12 of 12. Each original tests/*.rs file is
// mounted unchanged as a module below; add new test files to a suite here
// (and keep autotests disabled in Cargo.toml) instead of adding a new
// top-level tests/*.rs binary. Generated grouping: see git history.

#[path = "../raster_clone.rs"]
mod raster_clone;

#[path = "../raster_layers.rs"]
mod raster_layers;

#[path = "../render_regression.rs"]
mod render_regression;

#[path = "../v0110_variants.rs"]
mod v0110_variants;

#[path = "../v0120_gamut.rs"]
mod v0120_gamut;

#[path = "../v0120_lut.rs"]
mod v0120_lut;

#[path = "../v0120_preflight.rs"]
mod v0120_preflight;

#[path = "../v0120_print_ui.rs"]
mod v0120_print_ui;

#[path = "../v0120_trap.rs"]
mod v0120_trap;

#[path = "../v0130_fields.rs"]
mod v0130_fields;

#[path = "../v080_hardening.rs"]
mod v080_hardening;

#[path = "../v0140_layout_api.rs"]
mod v0140_layout_api;
