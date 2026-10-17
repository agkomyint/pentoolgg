//! Editing example: add a labelled badge. Arguments: `{"text": "SALE", "x": 24, "y": 24}`.
//! Plans first, then applies, so a bad operation changes nothing.
use pentool_ext_sdk::{args, call, log, set_result};
use serde_json::json;

#[no_mangle]
pub extern "C" fn run() -> i32 {
    let a = args();
    let text = a["text"].as_str().unwrap_or("NEW").to_string();
    let x = a["x"].as_f64().unwrap_or(24.0);
    let y = a["y"].as_f64().unwrap_or(24.0);
    let info = match call("document.info", json!({})) {
        Ok(v) => v,
        Err(e) => {
            log(&e);
            return 1;
        }
    };
    let layers = info["layers"].as_array().cloned().unwrap_or_default();
    let target = layers
        .iter()
        .find(|l| l["locked"] == false)
        .and_then(|l| l["id"].as_str().map(str::to_string));
    let Some(layer) = target else {
        log("no layer to stamp into");
        return 2;
    };
    let width = 16.0 + 14.0 * text.chars().count() as f64;
    let ops = json!([
        {"type": "put-shape", "shape": "rrect", "id": "stamp-bg", "layer": layer, "mode": "replace",
         "x": x, "y": y, "width": width, "height": 40, "radius": 8, "fill": "#e11d48"},
        {"type": "put-text", "id": "stamp-label", "layer": layer, "mode": "replace",
         "content": text, "x": x + 8.0, "y": y + 27.0, "font_size": 22, "fill": "#ffffff"}
    ]);
    if let Err(e) = call("document.plan", json!({"operations": ops})) {
        log(&e);
        return 3;
    }
    match call("document.apply", json!({"operations": ops})) {
        Ok(applied) => {
            set_result(json!({"stamped": ["stamp-bg", "stamp-label"], "changes": applied["changes"]}));
            0
        }
        Err(e) => {
            log(&e);
            4
        }
    }
}
