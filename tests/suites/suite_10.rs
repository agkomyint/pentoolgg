// Grouped integration suite 10 of 12. Each original tests/*.rs file is
// mounted unchanged as a module below; add new test files to a suite here
// (and keep autotests disabled in Cargo.toml) instead of adding a new
// top-level tests/*.rs binary. Generated grouping: see git history.

#[path = "../agent_editing.rs"]
mod agent_editing;

#[path = "../raster_heal.rs"]
mod raster_heal;

#[path = "../studio_upgrade.rs"]
mod studio_upgrade;

#[path = "../v0110_photo_spec.rs"]
mod v0110_photo_spec;

#[path = "../v0110_studio.rs"]
mod v0110_studio;

#[path = "../v0120_bundle.rs"]
mod v0120_bundle;

#[path = "../v0120_fill_precision.rs"]
mod v0120_fill_precision;

#[path = "../v0120_roles.rs"]
mod v0120_roles;

#[path = "../v0130_protocol.rs"]
mod v0130_protocol;

#[path = "../v0130_review.rs"]
mod v0130_review;

#[path = "../v0140_wrap.rs"]
mod v0140_wrap;
