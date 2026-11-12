//! Collaboration. The operation log, merge, claims and statistics live in
//! `pentool-core`; this module re-exports them and adds the networked parts
//! (sync, presence, permissions, tokens, evidence), which need sockets and the
//! HTTP stack. See `docs/architecture.md`.
pub use pentool_core::collab::*;

#[cfg(not(target_arch = "wasm32"))]
#[doc(hidden)]
pub mod evidence;
#[cfg(not(target_arch = "wasm32"))]
pub mod perm;
#[cfg(not(target_arch = "wasm32"))]
pub mod presence;
#[cfg(not(target_arch = "wasm32"))]
pub mod sync;
#[cfg(not(target_arch = "wasm32"))]
pub mod token;
