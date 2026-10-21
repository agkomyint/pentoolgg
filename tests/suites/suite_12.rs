// Grouped integration suite 12 of 12. Each original tests/*.rs file is
// mounted unchanged as a module below; add new test files to a suite here
// (and keep autotests disabled in Cargo.toml) instead of adding a new
// top-level tests/*.rs binary. Generated grouping: see git history.

#[path = "../raster_clone.rs"]
mod raster_clone;

#[path = "../raster_layers.rs"]
mod raster_layers;

#[path = "../render_regression.rs"]
mod render_regression;

#[path = "../v0110_variants.rs"]
mod v0110_variants;

#[path = "../v0120_gamut.rs"]
mod v0120_gamut;

#[path = "../v0120_lut.rs"]
mod v0120_lut;

#[path = "../v0120_preflight.rs"]
mod v0120_preflight;

#[path = "../v0120_print_ui.rs"]
mod v0120_print_ui;

#[path = "../v0120_trap.rs"]
mod v0120_trap;

#[path = "../v0130_fields.rs"]
mod v0130_fields;

#[path = "../v080_hardening.rs"]
mod v080_hardening;

#[path = "../v0140_layout_api.rs"]
mod v0140_layout_api;

#[path = "../v0150_scene3d.rs"]
mod v0150_scene3d;

#[path = "../v0160_bindings.rs"]
mod v0160_bindings;

#[path = "../v0160_migration.rs"]
mod v0160_migration;

#[path = "../v0160_mockup.rs"]
mod v0160_mockup;

#[path = "../v0160_packaging.rs"]
mod v0160_packaging;

#[path = "../v0160_uv.rs"]
mod v0160_uv;

#[path = "../v017_interchange.rs"]
mod v017_interchange;

#[path = "../v017_psd_export.rs"]
mod v017_psd_export;
#[path = "../v018_extension.rs"]
mod v018_extension;
#[path = "../v019_design.rs"]
mod v019_design;
#[path = "../v019_design_ui.rs"]
mod v019_design_ui;

#[path = "../v025_ai_studio.rs"]
mod v025_ai_studio;
