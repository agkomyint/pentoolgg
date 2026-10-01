use pentool::{
    document::{Document, StrokeCap, StrokeJoin},
    editing::{self, PathAction},
    render,
};

fn sample() -> Document {
    serde_json::from_value(serde_json::json!({
        "format":"pentool","version":2,"name":"Edges",
        "canvas":{"width":128,"height":128,"background":"none"},
        "layers":[{"id":"ink","name":"Ink","paths":[{
            "id":"corner","d":"M 30 100 L 60 25 L 90 100",
            "stroke":"white","stroke_width":20,"fill":"none"
        }]}]
    }))
    .unwrap()
}
fn alpha(doc: &Document, x: u32, y: u32) -> u8 {
    let png = render::to_png(doc, 1.0).unwrap();
    tiny_skia::Pixmap::decode_png(&png)
        .unwrap()
        .pixel(x, y)
        .unwrap()
        .alpha()
}

#[test]
fn legacy_paths_keep_round_edges() {
    let d = sample();
    assert_eq!(d.layers[0].paths[0].stroke_linecap, StrokeCap::Round);
    assert_eq!(d.layers[0].paths[0].stroke_linejoin, StrokeJoin::Round);
    assert_eq!(d.layers[0].paths[0].stroke_miterlimit, 4.0);
}

#[test]
fn sharp_miter_and_limit_change_native_pixels() {
    let mut d = sample();
    assert_eq!(alpha(&d, 60, 8), 0);
    d.layers[0].paths[0].stroke_linejoin = StrokeJoin::Miter;
    assert!(alpha(&d, 60, 8) > 0);
    let svg = render::to_svg(&d).unwrap();
    assert!(svg.contains("stroke-linejoin=\"miter\""));
    d.layers[0].paths[0].stroke_miterlimit = 1.0;
    assert_eq!(alpha(&d, 60, 8), 0);
    d.layers[0].paths[0].stroke_linejoin = StrokeJoin::Bevel;
    assert_eq!(alpha(&d, 60, 8), 0);
}

#[test]
fn butt_round_square_caps_have_distinct_pixels() {
    let mut d = sample();
    d.layers[0].paths[0].d = "M 30 60 L 90 60".into();
    assert!(alpha(&d, 25, 60) > 0);
    assert_eq!(alpha(&d, 21, 51), 0);
    d.layers[0].paths[0].stroke_linecap = StrokeCap::Butt;
    assert_eq!(alpha(&d, 25, 60), 0);
    d.layers[0].paths[0].stroke_linecap = StrokeCap::Square;
    assert!(alpha(&d, 21, 51) > 0);
}

#[test]
fn styling_preserves_geometry_and_respects_locks_and_limits() {
    let mut d = sample();
    let geometry = d.layers[0].paths[0].d.clone();
    editing::path(
        &mut d,
        PathAction::Style {
            id: "corner".into(),
            layer: "ink".into(),
            cap: Some(StrokeCap::Butt),
            join: Some(StrokeJoin::Miter),
            miter_limit: Some(8.0),
        },
    )
    .unwrap();
    assert_eq!(d.layers[0].paths[0].d, geometry);
    let saved = serde_json::to_string(&d).unwrap();
    let loaded: Document = serde_json::from_str(&saved).unwrap();
    assert_eq!(loaded.layers[0].paths[0].stroke_linejoin, StrokeJoin::Miter);
    d.layers[0].locked = true;
    assert!(editing::path(
        &mut d,
        PathAction::Style {
            id: "corner".into(),
            layer: "ink".into(),
            cap: None,
            join: Some(StrokeJoin::Bevel),
            miter_limit: None
        }
    )
    .is_err());
    d.layers[0].locked = false;
    assert!(editing::path(
        &mut d,
        PathAction::Style {
            id: "corner".into(),
            layer: "ink".into(),
            cap: None,
            join: None,
            miter_limit: Some(f32::NAN)
        }
    )
    .is_err());
    d.layers[0].paths[0].stroke_miterlimit = 0.0;
    assert!(d.validate().is_err());
    let mut bad_join = serde_json::to_value(sample()).unwrap();
    bad_join["layers"][0]["paths"][0]["stroke_linejoin"] = serde_json::json!("spiky");
    assert!(serde_json::from_value::<Document>(bad_join).is_err());
}
