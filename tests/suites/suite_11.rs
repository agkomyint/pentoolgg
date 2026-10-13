// Grouped integration suite 11 of 12. Each original tests/*.rs file is
// mounted unchanged as a module below; add new test files to a suite here
// (and keep autotests disabled in Cargo.toml) instead of adding a new
// top-level tests/*.rs binary. Generated grouping: see git history.

#[path = "../raster.rs"]
mod raster;

#[path = "../stroke_edges.rs"]
mod stroke_edges;

#[path = "../v0110_delivery.rs"]
mod v0110_delivery;

#[path = "../v0110_lens_geometry.rs"]
mod v0110_lens_geometry;

#[path = "../v0110_photo_precision.rs"]
mod v0110_photo_precision;

#[path = "../v0120_marks.rs"]
mod v0120_marks;

#[path = "../v0120_working_adjustments.rs"]
mod v0120_working_adjustments;

#[path = "../v0140_spreads.rs"]
mod v0140_spreads;

#[path = "../v0140_style_eval.rs"]
mod v0140_style_eval;

#[path = "../v0140_tables.rs"]
mod v0140_tables;
