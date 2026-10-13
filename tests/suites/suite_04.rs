// Grouped integration suite 4 of 12. Each original tests/*.rs file is
// mounted unchanged as a module below; add new test files to a suite here
// (and keep autotests disabled in Cargo.toml) instead of adding a new
// top-level tests/*.rs binary. Generated grouping: see git history.

#[path = "../bugfix_audit.rs"]
mod bugfix_audit;

#[path = "../raster_perf.rs"]
mod raster_perf;

#[path = "../v0110_metadata.rs"]
mod v0110_metadata;

#[path = "../v0120_icc.rs"]
mod v0120_icc;

#[path = "../v0120_separation.rs"]
mod v0120_separation;

#[path = "../v0130_analysis.rs"]
mod v0130_analysis;

#[path = "../v0130_score.rs"]
mod v0130_score;

#[path = "../v0140_styles.rs"]
mod v0140_styles;

#[path = "../v080_adjustments.rs"]
mod v080_adjustments;

#[path = "../v080_transforms_fills.rs"]
mod v080_transforms_fills;
