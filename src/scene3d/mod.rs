//! Static 3D: model import and the deterministic CPU renderer (v0.15.0).
//!
//! Everything here is self-contained: no platform math library, no network,
//! no GPU. The CPU path is normative; goldens come only from it.

pub mod cli;
pub mod gltf;
pub mod math;
pub mod model;
pub mod node;
pub mod obj;
pub mod render;
pub mod uv;

pub use model::{Limits, Model, LIMITS};
