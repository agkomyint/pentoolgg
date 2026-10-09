//! Photo engine 1: high-bit-depth RGB storage and the floating-point working
//! representation of `docs/photography-v1.md`, plus the bounded DNG reader.
pub mod adjust;
pub mod catalog;
pub mod color;
pub mod detail;
pub mod develop;
pub mod dng;
pub mod dngout;
pub mod icc;
pub mod lens;
pub mod ljpeg;
pub mod local;
pub mod math;
pub mod merge;
pub mod opcode;
pub mod output;
pub mod pipeline;
pub mod pixels;
pub mod png;
pub mod profile;
pub mod raw;
pub mod tiff;
pub mod variants;
pub mod warp;

/// Document format version that adds the photography catalog and photo nodes.
pub const VERSION: u64 = 7;

/// Fail with `[cancelled]` when the current operation has been cancelled.
pub(crate) fn check_cancelled() -> anyhow::Result<()> {
    crate::composite::check_cancelled()
        .map_err(|_| anyhow::anyhow!("[cancelled] photo development cancelled"))
}
