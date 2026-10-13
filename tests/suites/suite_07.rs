// Grouped integration suite 7 of 12. Each original tests/*.rs file is
// mounted unchanged as a module below; add new test files to a suite here
// (and keep autotests disabled in Cargo.toml) instead of adding a new
// top-level tests/*.rs binary. Generated grouping: see git history.

#[path = "../v0101_fixes.rs"]
mod v0101_fixes;

#[path = "../v0120_production.rs"]
mod v0120_production;

#[path = "../v0120_slug.rs"]
mod v0120_slug;

#[path = "../v0130_guides.rs"]
mod v0130_guides;

#[path = "../v0130_stroke_path.rs"]
mod v0130_stroke_path;

#[path = "../v0140_compose.rs"]
mod v0140_compose;

#[path = "../v0140_grids.rs"]
mod v0140_grids;

#[path = "../v0140_layout_spec.rs"]
mod v0140_layout_spec;

#[path = "../v062_authoring.rs"]
mod v062_authoring;

#[path = "../v081_fixes.rs"]
mod v081_fixes;
