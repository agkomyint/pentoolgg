use pentool::{
    document::{identity, Document, Text, TextAlign},
    fonts,
    geometry::{self, Operation},
    render,
    text::{self, TextAction},
};
use resvg::usvg::Tree;

fn sample() -> Document {
    let mut d = Document::new(500, 300);
    d.canvas.background = "none".into();
    d.layers[0].texts.push(Text {
        id: "title".into(),
        content: "Editable & <real> text".into(),
        x: 30.0,
        y: 100.0,
        font_family: fonts::DEFAULT_FAMILY.into(),
        font_size: 32.0,
        font_weight: 700,
        italic: false,
        fill: "#ffffff".into(),
        align: TextAlign::Left,
        letter_spacing: 0.0,
        line_height: 1.2,
        transform: identity(),
    });
    d
}

#[test]
fn legacy_documents_load_without_text_fields() {
    let d: Document = serde_json::from_str(include_str!("../examples/eye.pen")).unwrap();
    assert_eq!(d.version, 1);
    assert!(d.fonts.is_empty());
    assert!(d.layers.iter().all(|l| l.texts.is_empty()));
    d.validate().unwrap();
}

#[test]
fn bundled_fonts_work_with_no_system_fonts() {
    let d = sample();
    let db = fonts::database(&d, false).unwrap();
    for weight in [400, 700] {
        for italic in [false, true] {
            fonts::ensure_family(&db, fonts::DEFAULT_FAMILY, weight, italic).unwrap();
        }
    }
    assert_eq!(db.faces().count(), 4);
}

#[test]
fn text_renders_with_bundled_fonts() {
    let d = sample();
    let svg = render::to_svg(&d).unwrap();
    let db = fonts::database(&d, false).unwrap();
    let opt = resvg::usvg::Options {
        fontdb: std::sync::Arc::new(db),
        ..Default::default()
    };
    let tree = Tree::from_str(&svg, &opt).unwrap();
    let mut pix = tiny_skia::Pixmap::new(500, 300).unwrap();
    resvg::render(&tree, tiny_skia::Transform::identity(), &mut pix.as_mut());
    assert!(pix.pixels().iter().filter(|p| p.alpha() > 0).count() > 300);
    assert!(tree.has_text_nodes());
}

#[test]
fn svg_has_live_text_embedded_fonts_and_escaped_content() {
    let d = sample();
    let svg = render::to_svg(&d).unwrap();
    assert!(svg.contains("<text "));
    assert!(svg.contains("Editable &amp; &lt;real&gt; text"));
    assert!(svg.contains("data:font/ttf;base64,"));
    let outlined = render::to_svg_outlined(&d).unwrap();
    assert!(!outlined.contains("<text"));
    assert!(outlined.contains("<path"));
}

#[test]
fn text_settings_and_multiline_roundtrip() {
    let mut d = sample();
    d.layers[0].texts[0].content = "First\nSecond".into();
    d.layers[0].texts[0].align = TextAlign::Center;
    d.layers[0].texts[0].italic = true;
    d.layers[0].texts[0].letter_spacing = 2.0;
    let encoded = serde_json::to_string(&d).unwrap();
    let decoded: Document = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded.layers[0].texts[0].content, "First\nSecond");
    let svg = render::text_svg(&decoded.layers[0].texts[0]);
    assert!(svg.contains("text-anchor=\"middle\""));
    assert_eq!(svg.matches("<tspan").count(), 2);
}

#[test]
fn layer_transform_preserves_editable_text() {
    let mut d = sample();
    let original = d.layers[0].texts[0].clone();
    let before = render::text_bounds(&d, &original).unwrap();
    geometry::execute_layer(
        &mut d,
        "layer-1",
        &Operation::Translate { dx: 20.0, dy: 30.0 },
    )
    .unwrap();
    let t = &d.layers[0].texts[0];
    assert_eq!(t.content, original.content);
    assert_eq!(t.font_family, original.font_family);
    assert_eq!(t.font_size, original.font_size);
    assert_eq!(t.transform, [1.0, 0.0, 0.0, 1.0, 20.0, 30.0]);
    let after = render::text_bounds(&d, t).unwrap();
    assert!((after.x0 - before.x0 - 20.0).abs() < 0.001);
    assert!((after.y0 - before.y0 - 30.0).abs() < 0.001);
}

#[test]
fn text_edit_respects_layer_lock_and_unknown_family() {
    let mut d = sample();
    d.layers[0].locked = true;
    assert!(text::apply(
        &mut d,
        TextAction::Remove {
            id: "title".into(),
            layer: "layer-1".into()
        }
    )
    .is_err());
    d.layers[0].locked = false;
    d.layers[0].texts[0].font_family = "DefinitelyNotARealFontFamily_pentool".into();
    assert!(render::to_png(&d, 1.0).is_err());
}

#[test]
fn bad_typography_and_bad_fonts_rejected() {
    let mut d = sample();
    d.layers[0].texts[0].font_size = f64::NAN;
    assert!(d.validate().is_err());
    d.layers[0].texts[0].font_size = 32.0;
    d.layers[0].texts[0].content = "bad\0text".into();
    assert!(d.validate().is_err());
    assert!(fonts::asset("broken".into(), vec![1, 2, 3]).is_err());
}

#[test]
fn text_id_cannot_collide_with_path() {
    let mut d = sample();
    let path = pentool::editing::PathAction::Put {
        id: "title".into(),
        layer: "layer-1".into(),
        d: "M 0 0 L 10 10".into(),
        stroke: "black".into(),
        width: 1.0,
        fill: "none".into(),
        closed: false,
    };
    assert!(pentool::editing::path(&mut d, path).is_err());
}

#[test]
fn font_embedding_survives_roundtrip() {
    let mut d = sample();
    text::add_font(
        &mut d,
        fonts::asset("portable".into(), fonts::BUNDLED[0].3.to_vec()).unwrap(),
    )
    .unwrap();
    let saved = serde_json::to_string(&d).unwrap();
    let restored: Document = serde_json::from_str(&saved).unwrap();
    assert_eq!(
        fonts::decode(&restored.fonts[0]).unwrap(),
        fonts::BUNDLED[0].3
    );
    restored.validate().unwrap();
}

#[test]
fn serialization_keeps_empty_arrays_and_identity_for_extension_merge() {
    let mut d = sample();
    d.layers[0].texts[0].transform[4] = 30.0;
    text::add_font(
        &mut d,
        fonts::asset("portable".into(), fonts::BUNDLED[0].3.to_vec()).unwrap(),
    )
    .unwrap();
    d.layers[0].texts[0].transform = identity();
    let raw = serde_json::to_value(&d).unwrap();
    assert_eq!(raw["layers"][0]["texts"][0]["transform"][4], 0.0);
    d.layers[0].texts.clear();
    d.fonts.clear();
    let raw = serde_json::to_value(&d).unwrap();
    assert_eq!(raw["layers"][0]["texts"], serde_json::json!([]));
    assert_eq!(raw["fonts"], serde_json::json!([]));
}
