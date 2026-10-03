use pentool::{document::Document, render};
use std::{fs, path::Path};

fn load(path: &str) -> Document {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn assert_pixels(name: &str, tolerance: u8) {
    let doc = load(&format!("examples/{name}.pen"));
    let actual = tiny_skia::Pixmap::decode_png(&render::to_png(&doc, 2.0).unwrap()).unwrap();
    let expected =
        tiny_skia::Pixmap::decode_png(&fs::read(format!("examples/{name}.png")).unwrap()).unwrap();
    assert_eq!(actual.width(), expected.width());
    assert_eq!(actual.height(), expected.height());
    let mut different = 0usize;
    let mut largest = 0u8;
    for (a, b) in actual.data().iter().zip(expected.data()) {
        let delta = a.abs_diff(*b);
        largest = largest.max(delta);
        if delta > tolerance {
            different += 1;
        }
    }
    if different > 0 {
        let dir = Path::new("target/render-diffs");
        fs::create_dir_all(dir).unwrap();
        fs::write(
            dir.join(format!("{name}-actual.png")),
            actual.encode_png().unwrap(),
        )
        .unwrap();
        panic!("{name}: {different} channel values exceeded tolerance {tolerance}; largest delta {largest}; actual saved under target/render-diffs");
    }
}

#[test]
fn path_and_transparency_goldens_match_exactly() {
    assert_pixels("eye", 0);
}

#[test]
fn caps_joins_fills_and_miters_golden_matches_exactly() {
    assert_pixels("stroke-edges", 0);
}

#[test]
fn multiline_bundled_font_faces_golden_matches_with_tolerance() {
    assert_pixels("text-demo", 1);
}

#[test]
fn svg_contract_keeps_ids_live_text_and_portable_fonts() {
    let doc = load("examples/text-demo.pen");
    let svg = render::to_svg(&doc).unwrap();
    assert!(svg.contains("<text xml:space=\"preserve\""));
    assert!(svg.contains("<tspan"));
    assert!(svg.contains("@font-face"));
    assert!(svg.contains("data:font/ttf;base64,"));
    for layer in doc.layers.iter().filter(|layer| layer.visible) {
        assert!(svg.contains(&format!("<g id=\"{}\">", layer.id)));
    }
}

#[test]
fn png_matches_portable_svg_at_release_scales() {
    let doc = load("examples/eye.pen");
    let svg = render::to_svg(&doc).unwrap();
    let tree = resvg::usvg::Tree::from_str(&svg, &render::options(&doc).unwrap()).unwrap();
    for scale in [1.0f32, 2.0, 4.0, 8.0] {
        let expected =
            tiny_skia::Pixmap::decode_png(&render::to_png(&doc, scale).unwrap()).unwrap();
        let mut actual = tiny_skia::Pixmap::new(expected.width(), expected.height()).unwrap();
        resvg::render(
            &tree,
            tiny_skia::Transform::from_scale(scale, scale),
            &mut actual.as_mut(),
        );
        assert_eq!(actual.data(), expected.data(), "scale {scale}");
    }
}
