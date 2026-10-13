// Grouped integration suite 8 of 12. Each original tests/*.rs file is
// mounted unchanged as a module below; add new test files to a suite here
// (and keep autotests disabled in Cargo.toml) instead of adding a new
// top-level tests/*.rs binary. Generated grouping: see git history.

#[path = "../export_flags.rs"]
mod export_flags;

#[path = "../v0120_collection.rs"]
mod v0120_collection;

#[path = "../v0120_editing.rs"]
mod v0120_editing;

#[path = "../v0120_output.rs"]
mod v0120_output;

#[path = "../v0120_scene.rs"]
mod v0120_scene;

#[path = "../v0120_spot_substitution.rs"]
mod v0120_spot_substitution;

#[path = "../v070_operations.rs"]
mod v070_operations;

#[path = "../v071_fixes.rs"]
mod v071_fixes;

#[path = "../v080_campaign.rs"]
mod v080_campaign;

#[path = "../v080_portability_batch.rs"]
mod v080_portability_batch;
