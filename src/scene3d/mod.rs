//! Static 3D: model import and the deterministic CPU renderer (v0.15.0).
//!
//! Everything here is self-contained: no platform math library, no network,
//! no GPU. The CPU path is normative; goldens come only from it.

pub mod gltf;
pub mod math;
pub mod model;
pub mod obj;

pub use model::{Limits, Model, LIMITS};
