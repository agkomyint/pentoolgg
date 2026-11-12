//! Deterministic structural document comparison.
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Serialize)]
pub struct Change {
    pub path: String,
    pub before: Option<Value>,
    pub after: Option<Value>,
}

pub fn structural(before: &Value, after: &Value) -> Vec<Change> {
    fn walk(path: &str, before: Option<&Value>, after: Option<&Value>, out: &mut Vec<Change>) {
        if out.len() >= 10_000 || before == after {
            return;
        }
        // Raster tile store entries are content-addressed PNG payloads; report
        // that a tile was added, removed or rewritten without its base64 data.
        if path
            .strip_prefix("/raster_tiles/")
            .is_some_and(|rest| !rest.contains('/'))
        {
            let summary = |entry: Option<&Value>| {
                entry.map(|e| {
                    serde_json::json!({
                        "raster_tile": true,
                        "encoding": e["encoding"],
                        "data_bytes": e["data"].as_str().map_or(0, str::len),
                    })
                })
            };
            out.push(Change {
                path: path.into(),
                before: summary(before),
                after: summary(after),
            });
            return;
        }
        match (before, after) {
            (Some(Value::Object(a)), Some(Value::Object(b))) => {
                let mut keys: Vec<_> = a.keys().chain(b.keys()).collect();
                keys.sort();
                keys.dedup();
                for key in keys {
                    walk(
                        &format!("{path}/{}", key.replace('~', "~0").replace('/', "~1")),
                        a.get(key),
                        b.get(key),
                        out,
                    );
                }
            }
            (Some(Value::Array(a)), Some(Value::Array(b))) => {
                for index in 0..a.len().max(b.len()) {
                    walk(&format!("{path}/{index}"), a.get(index), b.get(index), out);
                }
            }
            _ => out.push(Change {
                path: if path.is_empty() {
                    "/".into()
                } else {
                    path.into()
                },
                before: before.cloned(),
                after: after.cloned(),
            }),
        }
    }
    let mut out = Vec::new();
    walk("", Some(before), Some(after), &mut out);
    out
}
