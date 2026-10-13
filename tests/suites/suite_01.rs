// Grouped integration suite 1 of 12. Each original tests/*.rs file is
// mounted unchanged as a module below; add new test files to a suite here
// (and keep autotests disabled in Cargo.toml) instead of adding a new
// top-level tests/*.rs binary. Generated grouping: see git history.

#[path = "../raster_compose.rs"]
mod raster_compose;

#[path = "../v0110_local.rs"]
mod v0110_local;

#[path = "../v0110_organize.rs"]
mod v0110_organize;

#[path = "../v0120_catalog.rs"]
mod v0120_catalog;

#[path = "../v0120_color.rs"]
mod v0120_color;

#[path = "../v0120_spot.rs"]
mod v0120_spot;

#[path = "../v0130_bundle.rs"]
mod v0130_bundle;

#[path = "../v0130_constraints.rs"]
mod v0130_constraints;

#[path = "../v080_effects.rs"]
mod v080_effects;

#[path = "../v080_masks.rs"]
mod v080_masks;
