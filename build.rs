//! Embeds the browser renderer when `web/pentool_core.wasm` exists. Build it with
//! `cargo build -p pentool-wasm --release --target wasm32-unknown-unknown` and copy
//! the result to `web/pentool_core.wasm`. Without it, Studio renders on the server.
fn main() {
    println!("cargo::rustc-check-cfg=cfg(has_wasm_core)");
    println!("cargo::rerun-if-changed=web/pentool_core.wasm");
    println!("cargo::rerun-if-changed=build.rs");
    if std::path::Path::new("web/pentool_core.wasm").exists() {
        println!("cargo::rustc-cfg=has_wasm_core");
    }
}
