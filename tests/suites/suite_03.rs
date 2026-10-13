// Grouped integration suite 3 of 12. Each original tests/*.rs file is
// mounted unchanged as a module below; add new test files to a suite here
// (and keep autotests disabled in Cargo.toml) instead of adding a new
// top-level tests/*.rs binary. Generated grouping: see git history.

#[path = "../raster_hardening.rs"]
mod raster_hardening;

#[path = "../v0110_merge.rs"]
mod v0110_merge;

#[path = "../v0110_raw_ingest.rs"]
mod v0110_raw_ingest;

#[path = "../v0120_proof.rs"]
mod v0120_proof;

#[path = "../v0120_working_effects.rs"]
mod v0120_working_effects;

#[path = "../v0120_working_transforms.rs"]
mod v0120_working_transforms;

#[path = "../v0130_plan.rs"]
mod v0130_plan;

#[path = "../v0130_recovery.rs"]
mod v0130_recovery;

#[path = "../v0140_book.rs"]
mod v0140_book;

#[path = "../v0140_shaping.rs"]
mod v0140_shaping;
