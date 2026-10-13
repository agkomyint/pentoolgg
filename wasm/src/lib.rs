//! C-ABI surface of the browser renderer. No `wasm-bindgen`: the JS glue in
//! `web-ui/src/lib/wasmCore.ts` talks to these few exports directly.
//!
//! Protocol: JS allocates buffers with `alloc`, writes the document JSON and
//! page id, calls `render`, then reads the RGBA result through `result_*`.
//! A non-zero return means failure; `error_ptr`/`error_len` hold the message.
use std::path::Path;

struct Output {
    pixels: Vec<u8>,
    width: u32,
    height: u32,
    error: String,
}

static mut OUTPUT: Output = Output {
    pixels: Vec::new(),
    width: 0,
    height: 0,
    error: String::new(),
};

#[allow(static_mut_refs)]
fn output() -> &'static mut Output {
    // The module is single-threaded; JS never calls into it re-entrantly.
    unsafe { &mut OUTPUT }
}

#[no_mangle]
pub extern "C" fn alloc(len: usize) -> *mut u8 {
    let mut buffer = Vec::<u8>::with_capacity(len.max(1));
    let ptr = buffer.as_mut_ptr();
    std::mem::forget(buffer);
    ptr
}

/// # Safety
/// `ptr` and `len` must come from a previous `alloc(len)` call.
#[no_mangle]
pub unsafe extern "C" fn dealloc(ptr: *mut u8, len: usize) {
    drop(Vec::from_raw_parts(ptr, 0, len.max(1)));
}

fn text<'a>(ptr: *const u8, len: usize) -> Result<&'a str, String> {
    // SAFETY: JS passes a pointer/length pair it just wrote through `alloc`.
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
    std::str::from_utf8(bytes).map_err(|e| format!("input is not UTF-8: {e}"))
}

fn render_inner(
    doc: *const u8,
    doc_len: usize,
    page: *const u8,
    page_len: usize,
    scale: f32,
) -> Result<(), String> {
    let raw: serde_json::Value =
        serde_json::from_str(text(doc, doc_len)?).map_err(|e| format!("invalid document: {e}"))?;
    let page = text(page, page_len)?;
    let page = (!page.is_empty()).then_some(page);
    let image = pentool::composite::render(&raw, Path::new(""), page, scale)
        .map_err(|e| format!("{e:#}"))?;
    let out = output();
    out.width = image.width();
    out.height = image.height();
    out.pixels = image.into_raw();
    Ok(())
}

/// Render one page to RGBA8. Returns 0 on success, 1 on failure.
#[no_mangle]
pub extern "C" fn render(
    doc: *const u8,
    doc_len: usize,
    page: *const u8,
    page_len: usize,
    scale: f32,
) -> i32 {
    output().error.clear();
    match std::panic::catch_unwind(|| render_inner(doc, doc_len, page, page_len, scale)) {
        Ok(Ok(())) => 0,
        Ok(Err(message)) => {
            output().error = message;
            1
        }
        Err(_) => {
            output().error = "the renderer panicked".to_string();
            1
        }
    }
}

#[no_mangle]
pub extern "C" fn result_ptr() -> *const u8 {
    output().pixels.as_ptr()
}
#[no_mangle]
pub extern "C" fn result_len() -> usize {
    output().pixels.len()
}
#[no_mangle]
pub extern "C" fn result_width() -> u32 {
    output().width
}
#[no_mangle]
pub extern "C" fn result_height() -> u32 {
    output().height
}
#[no_mangle]
pub extern "C" fn error_ptr() -> *const u8 {
    output().error.as_ptr()
}
#[no_mangle]
pub extern "C" fn error_len() -> usize {
    output().error.len()
}
