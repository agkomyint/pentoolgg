// Grouped integration suite 9 of 12. Each original tests/*.rs file is
// mounted unchanged as a module below; add new test files to a suite here
// (and keep autotests disabled in Cargo.toml) instead of adding a new
// top-level tests/*.rs binary. Generated grouping: see git history.

#[path = "../ai.rs"]
mod ai;

#[path = "../instance_flags.rs"]
mod instance_flags;

#[path = "../libraries_packages.rs"]
mod libraries_packages;

#[path = "../raster_select.rs"]
mod raster_select;

#[path = "../v0110_conformance.rs"]
mod v0110_conformance;

#[path = "../v0110_detail.rs"]
mod v0110_detail;

#[path = "../v0120_hostile_print.rs"]
mod v0120_hostile_print;

#[path = "../v0120_ink_cli.rs"]
mod v0120_ink_cli;

#[path = "../v080_compositing.rs"]
mod v080_compositing;

#[path = "../comments.rs"]
mod comments;
