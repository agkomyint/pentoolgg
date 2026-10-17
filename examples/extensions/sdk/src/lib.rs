//! Guest-side helper for Pentool extension api 1 (compile to wasm32-unknown-unknown).
//!
//! The host provides one import, `pentool.call`. A request is a JSON object
//! `{"method": ..., "params": ...}`; the response is `{"ok": true, "result": ...}`
//! or `{"ok": false, "error": {"code", "message"}}`.
use serde_json::{json, Value};

#[link(wasm_import_module = "pentool")]
extern "C" {
    #[link_name = "call"]
    fn host_call(req_ptr: *const u8, req_len: usize, out_ptr: *mut u8, out_cap: usize) -> i32;
}

/// Call a host method. `Err` carries the host's error message.
pub fn call(method: &str, params: Value) -> Result<Value, String> {
    let request = json!({"method": method, "params": params}).to_string();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut n = unsafe {
        host_call(request.as_ptr(), request.len(), buffer.as_mut_ptr(), buffer.len())
    };
    if n < 0 {
        // The response did not fit: grow the buffer and re-read it (empty request).
        buffer = vec![0u8; (-n) as usize];
        n = unsafe { host_call(core::ptr::null(), 0, buffer.as_mut_ptr(), buffer.len()) };
    }
    let response: Value =
        serde_json::from_slice(&buffer[..n as usize]).map_err(|e| e.to_string())?;
    if response["ok"] == true {
        Ok(response["result"].clone())
    } else {
        Err(response["error"]["message"].as_str().unwrap_or("host error").to_string())
    }
}

pub fn log(message: &str) {
    let _ = call("log", json!({"message": message}));
}

/// Arguments passed with `pentool extension run --args ...`.
pub fn args() -> Value {
    call("args", Value::Null).unwrap_or(Value::Null)["args"].clone()
}

pub fn set_result(value: Value) {
    let _ = call("result", json!({"value": value}));
}
