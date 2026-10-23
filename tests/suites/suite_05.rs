// Grouped integration suite 5 of 12. Each original tests/*.rs file is
// mounted unchanged as a module below; add new test files to a suite here
// (and keep autotests disabled in Cargo.toml) instead of adding a new
// top-level tests/*.rs binary. Generated grouping: see git history.

#[path = "../stabilization.rs"]
mod stabilization;

#[path = "../cli_help.rs"]
mod cli_help;

#[path = "../raster_batch.rs"]
mod raster_batch;

#[path = "../raster_fill.rs"]
mod raster_fill;

#[path = "../raster_presets.rs"]
mod raster_presets;

#[path = "../v0130_palette.rs"]
mod v0130_palette;

#[path = "../v0140_idml.rs"]
mod v0140_idml;

#[path = "../v0140_longdoc.rs"]
mod v0140_longdoc;

#[path = "../v0140_stories.rs"]
mod v0140_stories;

#[path = "../v070_images.rs"]
mod v070_images;

#[path = "../v072_fixes.rs"]
mod v072_fixes;

#[path = "../v080_selections_inspection.rs"]
mod v080_selections_inspection;
