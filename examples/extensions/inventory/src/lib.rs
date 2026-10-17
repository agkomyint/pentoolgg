//! Read-only example: count every object on the selected page by kind.
//! Declares only `document.read`, so it can never change a document.
use pentool_ext_sdk::{call, log, set_result};
use serde_json::{json, Map, Value};

#[no_mangle]
pub extern "C" fn run() -> i32 {
    let mut counts = Map::new();
    let mut offset = 0u64;
    loop {
        let page = match call("document.tree", json!({"offset": offset, "limit": 200})) {
            Ok(v) => v,
            Err(e) => {
                log(&e);
                return 1;
            }
        };
        let mut seen = 0u64;
        for layer in page["layers"].as_array().into_iter().flatten() {
            for object in layer["objects"].as_array().into_iter().flatten() {
                let kind = object["kind"].as_str().unwrap_or("unknown").to_string();
                let next = counts.get(&kind).and_then(Value::as_u64).unwrap_or(0) + 1;
                counts.insert(kind, json!(next));
                seen += 1;
            }
        }
        if page["has_more"] != true || seen == 0 {
            break;
        }
        offset += seen;
    }
    set_result(json!({"objects_by_kind": counts}));
    0
}
