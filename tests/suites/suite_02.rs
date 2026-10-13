// Grouped integration suite 2 of 12. Each original tests/*.rs file is
// mounted unchanged as a module below; add new test files to a suite here
// (and keep autotests disabled in Cargo.toml) instead of adding a new
// top-level tests/*.rs binary. Generated grouping: see git history.

#[path = "../batch_flags.rs"]
mod batch_flags;

#[path = "../v0110_camera_profiles.rs"]
mod v0110_camera_profiles;

#[path = "../v0110_development.rs"]
mod v0110_development;

#[path = "../v0120_ink.rs"]
mod v0120_ink;

#[path = "../v0120_interpolation.rs"]
mod v0120_interpolation;

#[path = "../v0120_profile.rs"]
mod v0120_profile;

#[path = "../v0120_recipe.rs"]
mod v0120_recipe;

#[path = "../v0130_conformance.rs"]
mod v0130_conformance;

#[path = "../v0140_publish.rs"]
mod v0140_publish;

#[path = "../v0140_merge.rs"]
mod v0140_merge;

#[path = "../v064_instances.rs"]
mod v064_instances;

#[path = "../v070_hardening.rs"]
mod v070_hardening;
