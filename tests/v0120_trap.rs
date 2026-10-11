//! Manual trapping: a declared spread of an object's ink into its neighbor.
use pentool::color::{
    catalog,
    separation::{self, Options, Separations},
    spot::{Ink, Library, Provenance},
};
use serde_json::{json, Value};
use std::path::Path;

const DOCUMENT: &str = "target/trap.pen";

fn ink(id: &str) -> Ink {
    Ink {
        id: id.into(),
        name: format!("Ink {id}"),
        aliases: vec![],
        alternate: json!({"space":"srgb","channels":[0.95,0.22,0.03]}),
        provenance: Provenance {
            source: "test library".into(),
            publisher: None,
            revision: None,
            license: None,
        },
        redistribution: "allowed".into(),
    }
}

fn cmyk(c: [f64; 4]) -> Value {
    json!({"space":"icc-cmyk","profile":"press","channels":c})
}

fn rect(id: &str, x: u32, width: u32, fill: Value, extra: Value) -> Value {
    let mut node = json!({"kind":"rect","id":id,"x":x,"y":0,"width":width,"height":2,
        "style":{"fill":fill}});
    for (key, value) in extra.as_object().into_iter().flatten() {
        node["style"][key] = value.clone();
    }
    node
}

fn doc(nodes: Vec<Value>) -> Value {
    let mut raw: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/print-v8.pen")).unwrap();
    raw["pages"][0]["canvas"] = json!({"width":8,"height":2,"background":"#00000000"});
    catalog::import(
        &mut raw,
        Path::new(DOCUMENT),
        include_bytes!("../docs/fixtures/print-CGATS21_CRPC7.icc"),
        "press",
        "allowed",
        None,
        None,
    )
    .unwrap();
    raw["pages"][0]["layers"][0]["nodes"] = json!(nodes);
    raw["color_management"]["roles"] = json!({"output":{
        "destination":{"space":"icc-cmyk","profile":"press"},"intent":"relative-colorimetric"}});
    let mut library = Library::default();
    for id in ["zeta", "alpha"] {
        library.insert(ink(id)).unwrap();
    }
    raw["color_management"]["inks"] = serde_json::to_value(&library).unwrap();
    raw
}

fn separate(raw: &Value) -> anyhow::Result<Separations> {
    separation::separate(
        raw,
        Path::new(DOCUMENT),
        None,
        &Options {
            destination: None,
            intent: None,
            scale: 1.0,
            auto_trap: None,
        },
    )
}

fn at(result: &Separations, plate: &str, x: usize) -> f64 {
    let plate = result.plates.iter().find(|p| p.id == plate).unwrap();
    plate.samples[x]
}

fn near(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-9, "{a} != {b}");
}

fn scene(trap: Value) -> Value {
    // A light cyan block (x 0..4) beside a dark block (x 4..8). The light block
    // declares the trap so its ink spreads under the dark neighbor.
    doc(vec![
        rect(
            "dark",
            4,
            4,
            json!({"fallback":cmyk([0.9, 0.9, 0.9, 0.9])}),
            json!({}),
        ),
        rect(
            "light",
            0,
            4,
            json!({"fallback":cmyk([0.5, 0.0, 0.0, 0.0])}),
            if trap.is_null() {
                json!({})
            } else {
                json!({"trap":trap})
            },
        ),
    ])
}

#[test]
fn trap_spreads_light_ink_into_the_neighbor_only_where_it_raises_the_plate() {
    let untrapped = separate(&scene(Value::Null)).unwrap();
    assert_eq!(untrapped.traps, 0);
    // Without a trap the light block is knocked out of the dark one at the edge.
    near(at(&untrapped, "C", 4), 0.9);
    let trapped = separate(&scene(json!({"width":1}))).unwrap();
    assert_eq!(trapped.traps, 1);
    // The painted order is dark then light, so the fringe lands on the dark
    // block: C is already higher (0.9 > 0.5), so nothing changes there.
    near(at(&trapped, "C", 4), 0.9);
    near(at(&trapped, "C", 3), at(&untrapped, "C", 3));
    // M is 0.9 under the fringe and the light value is 0: unchanged too.
    near(at(&trapped, "M", 4), 0.9);
    // Reverse roles: the dark object traps into the light one.
    let reverse = doc(vec![
        rect(
            "light",
            0,
            4,
            json!({"fallback":cmyk([0.5, 0.0, 0.0, 0.0])}),
            json!({}),
        ),
        rect(
            "dark",
            4,
            4,
            json!({"fallback":cmyk([0.9, 0.9, 0.9, 0.9])}),
            json!({"trap":{"width":2}}),
        ),
    ]);
    let result = separate(&reverse).unwrap();
    // Two pixels of fringe: x = 2 and 3 pick up the dark values.
    near(at(&result, "M", 3), 0.9);
    near(at(&result, "M", 2), 0.9);
    near(at(&result, "M", 1), 0.0);
    // C raises from 0.5 to 0.9 inside the fringe only.
    near(at(&result, "C", 2), 0.9);
    near(at(&result, "C", 1), 0.5);
    // Deterministic.
    assert_eq!(result, separate(&reverse).unwrap());
}

#[test]
fn malformed_and_oversized_traps_are_refused() {
    for (trap, needle) in [
        (json!({"width":0}), "trap width"),
        (json!({"width":-1}), "trap width"),
        (json!({"width":"2"}), "trap width"),
        (json!({}), "trap width"),
        (json!({"width":1,"mode":"choke"}), "only accepts width"),
        (json!(3), "must be an object"),
        (json!({"width":100}), "limit"),
    ] {
        let raw = scene(trap.clone());
        let error = separate(&raw).unwrap_err().to_string();
        assert!(error.contains(needle), "{trap}: {error}");
    }
    let mut both = scene(json!({"width":1}));
    both["pages"][0]["layers"][0]["nodes"][1]["style"]["overprint"] = json!({"fill":true});
    let error = separate(&both).unwrap_err().to_string();
    assert!(error.contains("trap and overprint"), "{error}");
}
