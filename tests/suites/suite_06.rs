// Grouped integration suite 6 of 12. Each original tests/*.rs file is
// mounted unchanged as a module below; add new test files to a suite here
// (and keep autotests disabled in Cargo.toml) instead of adding a new
// top-level tests/*.rs binary. Generated grouping: see git history.

#[path = "../importing.rs"]
mod importing;

#[path = "../raster_brush.rs"]
mod raster_brush;

#[path = "../typography.rs"]
mod typography;

#[path = "../v0110_export.rs"]
mod v0110_export;

#[path = "../v0120_compositing.rs"]
mod v0120_compositing;

#[path = "../v0120_gradients.rs"]
mod v0120_gradients;

#[path = "../v0120_link.rs"]
mod v0120_link;

#[path = "../v0120_pdfx.rs"]
mod v0120_pdfx;

#[path = "../v0120_working_scene.rs"]
mod v0120_working_scene;

#[path = "../v0130_media.rs"]
mod v0130_media;
