//! The production output transform quantizes only at an explicit delivery step.
use pentool::{
    color::{
        catalog,
        icc::Intent,
        output::{self, Alpha, Depth, Options},
        value::{Destination, Evaluator, NativeColor, ProfileSet},
    },
    composite,
};
use serde_json::{json, Value};
use std::path::Path;
const DOCUMENT: &str = "target/output.pen";

fn doc(channels: [f64; 3], alpha: f64) -> Value {
    let mut raw: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/print-v8.pen")).unwrap();
    raw["pages"][0]["canvas"] = json!({"width":2,"height":1,"background":"#00000000"});
    raw["pages"][0]["layers"][0]["nodes"] = json!([{"kind":"fill","id":"paint","x":0,"y":0,"width":2,"height":1,
        "fill":{"kind":"solid","color":{"space":"srgb:linear","channels":channels,"alpha":alpha}}}]);
    raw
}
fn with_press(raw: &mut Value) {
    catalog::import(
        raw,
        Path::new(DOCUMENT),
        include_bytes!("../docs/fixtures/print-CGATS21_CRPC7.icc"),
        "press",
        "allowed",
        None,
        None,
    )
    .unwrap();
}
fn options(destination: Value, depth: Depth, alpha: Alpha) -> Options {
    Options {
        destination: Some(Destination::parse(&destination).unwrap()),
        intent: Some(Intent::RelativeColorimetric),
        depth,
        alpha,
    }
}
fn encode(v: f64) -> f64 {
    if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}
fn q(v: f64, max: f64) -> u16 {
    (v.clamp(0.0, 1.0) * max + 0.5).floor() as u16
}
fn deliver(raw: &Value, options: &Options) -> anyhow::Result<output::OutputImage> {
    output::render(raw, Path::new(DOCUMENT), None, 1.0, options)
}

#[test]
fn rgb_delivery_quantizes_once_with_independent_transfer_and_depths() {
    let raw = doc([0.2, 0.4, 0.6], 0.234567891);
    let before = raw.clone();
    let eight = deliver(
        &raw,
        &options(json!({"space":"srgb"}), Depth::Eight, Alpha::Preserve),
    )
    .unwrap();
    assert_eq!(
        (eight.channels, eight.has_alpha, eight.samples.len()),
        (3, true, 8)
    );
    let expected: Vec<u16> = [0.2, 0.4, 0.6]
        .iter()
        .map(|v| q(encode(*v), 255.0))
        .chain([q(0.234567891, 255.0)])
        .collect();
    assert_eq!(&eight.samples[..4], expected.as_slice());
    assert_eq!(eight.samples[..4], eight.samples[4..]);
    assert_eq!(eight.to_bytes().len(), 8);
    assert!(eight.report.max_quantization_error <= 0.5 / 255.0 + 1e-12);
    assert_eq!(eight.report.pixels, 2);
    assert_eq!(eight.report.quantized_samples, 8);
    assert_eq!(eight.report.clipped_pixels, 0);

    let sixteen = deliver(
        &raw,
        &options(json!({"space":"srgb"}), Depth::Sixteen, Alpha::Preserve),
    )
    .unwrap();
    for (sample, value) in sixteen.samples[..3].iter().zip([0.2, 0.4, 0.6]) {
        assert!((i32::from(*sample) - i32::from(q(encode(value), 65535.0))).abs() <= 1);
    }
    let bytes = sixteen.to_bytes();
    assert_eq!(bytes.len(), 16);
    assert_eq!(u16::from_be_bytes([bytes[0], bytes[1]]), sixteen.samples[0]);
    assert!(sixteen.report.max_quantization_error <= 0.5 / 65535.0 + 1e-12);
    assert_eq!(raw, before);
}

#[test]
fn working_primaries_and_gamut_excursions_are_reported_at_delivery() {
    let mut raw = doc([1.0, 0.0, 0.0], 1.0);
    raw["color_management"]["roles"] = json!({"working":{"destination":{"space":"display-p3:linear"},"intent":"relative-colorimetric"}});
    // sRGB red stored in P3 working primaries is the familiar P3 triple.
    let p3 = deliver(
        &raw,
        &options(json!({"space":"display-p3"}), Depth::Eight, Alpha::Preserve),
    )
    .unwrap();
    assert_eq!(p3.samples[..3], [234, 51, 35]);
    assert_eq!(p3.report.clipped_pixels, 0);
    // Display-P3 red lies outside sRGB: it saturates and is reported.
    raw["pages"][0]["layers"][0]["nodes"][0]["fill"]["color"]["space"] = json!("display-p3:linear");
    let srgb = deliver(
        &raw,
        &options(json!({"space":"srgb"}), Depth::Eight, Alpha::Preserve),
    )
    .unwrap();
    assert_eq!(srgb.samples[..3], [255, 0, 0]);
    assert_eq!(srgb.report.clipped_pixels, 2);
    let native = deliver(
        &raw,
        &options(json!({"space":"display-p3"}), Depth::Eight, Alpha::Preserve),
    )
    .unwrap();
    assert_eq!(native.samples[..3], [255, 0, 0]);
    assert_eq!(native.report.clipped_pixels, 0);
}

#[test]
fn icc_rgb_delivery_matches_the_native_conversion() {
    let mut raw = doc([0.2, 0.4, 0.6], 1.0);
    catalog::import(
        &mut raw,
        Path::new(DOCUMENT),
        include_bytes!("../docs/fixtures/print-rgb-linear-v4.icc"),
        "probe",
        "allowed",
        None,
        None,
    )
    .unwrap();
    let probe = json!({"space":"icc-rgb","profile":"probe"});
    let image = deliver(
        &raw,
        &options(probe.clone(), Depth::Sixteen, Alpha::Preserve),
    )
    .unwrap();
    let set = ProfileSet::load(&raw, Path::new(DOCUMENT), ["probe"]).unwrap();
    let mut engine = Evaluator::new(&set, Intent::RelativeColorimetric).unwrap();
    let native =
        NativeColor::parse(&json!({"space":"srgb:linear","channels":[0.2,0.4,0.6]})).unwrap();
    let converted = engine
        .convert(&native, &Destination::parse(&probe).unwrap())
        .unwrap();
    let expected: Vec<u16> = converted
        .color
        .channels()
        .iter()
        .map(|v| q(*v, 65535.0))
        .collect();
    assert_eq!(&image.samples[..3], expected.as_slice());
    assert_eq!(image.samples[3], 65535);
}

#[test]
fn output_role_is_the_default_and_must_be_configured() {
    let mut raw = doc([0.5; 3], 1.0);
    let default = Options {
        destination: None,
        intent: None,
        depth: Depth::Eight,
        alpha: Alpha::Preserve,
    };
    let error = deliver(&raw, &default).unwrap_err().to_string();
    assert!(error.contains("output color role"), "{error}");
    raw["color_management"]["roles"] =
        json!({"output":{"destination":{"space":"srgb"},"intent":"perceptual"}});
    let image = deliver(&raw, &default).unwrap();
    assert_eq!(image.intent, Intent::Perceptual);
    assert_eq!(image.destination.space(), "srgb");
    assert_eq!(image.samples[0], q(encode(0.5), 255.0));
    assert_eq!(image.json()["scope"], "production-output-transform");
}

#[test]
fn cmyk_delivery_matches_the_native_icc_conversion_and_refuses_hidden_alpha() {
    let mut raw = doc([0.2, 0.4, 0.6], 1.0);
    with_press(&mut raw);
    let press = json!({"space":"icc-cmyk","profile":"press"});
    let image = deliver(&raw, &options(press.clone(), Depth::Eight, Alpha::Preserve)).unwrap();
    assert_eq!((image.channels, image.has_alpha), (4, false));
    assert_eq!(image.report.profile.as_deref(), Some("press"));
    let set = ProfileSet::load(&raw, Path::new(DOCUMENT), ["press"]).unwrap();
    let mut engine = Evaluator::new(&set, Intent::RelativeColorimetric).unwrap();
    let native =
        NativeColor::parse(&json!({"space":"srgb:linear","channels":[0.2,0.4,0.6]})).unwrap();
    let converted = engine
        .convert(&native, &Destination::parse(&press).unwrap())
        .unwrap();
    let expected: Vec<u16> = converted
        .color
        .channels()
        .iter()
        .map(|v| q(*v, 255.0))
        .collect();
    assert_eq!(&image.samples[..4], expected.as_slice());
    assert_eq!(image.samples[..4], image.samples[4..]);

    // Half-covered pixels have no CMYK alpha plane unless explicitly flattened.
    let mut half = doc([0.0; 3], 0.5);
    with_press(&mut half);
    let error = deliver(
        &half,
        &options(press.clone(), Depth::Eight, Alpha::Preserve),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("alpha flatten"), "{error}");
    let flat = deliver(
        &half,
        &options(press.clone(), Depth::Eight, Alpha::FlattenOnWhite),
    )
    .unwrap();
    let gray =
        NativeColor::parse(&json!({"space":"srgb:linear","channels":[0.5,0.5,0.5]})).unwrap();
    let set = ProfileSet::load(&half, Path::new(DOCUMENT), ["press"]).unwrap();
    let mut engine = Evaluator::new(&set, Intent::RelativeColorimetric).unwrap();
    let converted = engine
        .convert(&gray, &Destination::parse(&press).unwrap())
        .unwrap();
    let expected: Vec<u16> = converted
        .color
        .channels()
        .iter()
        .map(|v| q(*v, 255.0))
        .collect();
    assert_eq!(&flat.samples[..4], expected.as_slice());
}

#[test]
fn unsupported_destinations_and_depths_fail_without_mutation() {
    let raw = doc([0.5; 3], 1.0);
    let before = raw.clone();
    for (destination, needle) in [
        (json!({"space":"lab-d50"}), "Lab delivery"),
        (json!({"space":"srgb:linear"}), "encoded device values"),
    ] {
        let error = deliver(&raw, &options(destination, Depth::Eight, Alpha::Preserve))
            .unwrap_err()
            .to_string();
        assert!(error.contains(needle), "{error}");
    }
    assert!(Depth::parse(12).is_err());
    assert_eq!(Depth::parse(16).unwrap().bits(), 16);
    // A profile reference that is not in the catalog fails before any pixel work.
    let error = deliver(
        &raw,
        &options(
            json!({"space":"icc-cmyk","profile":"missing"}),
            Depth::Eight,
            Alpha::Preserve,
        ),
    )
    .unwrap_err()
    .to_string();
    assert!(!error.is_empty());
    assert_eq!(raw, before);
    // Ordinary preview exports remain the documented sRGB8 renderer.
    let preview = composite::render(&raw, Path::new(DOCUMENT), None, 1.0).unwrap();
    assert_eq!(preview.dimensions(), (2, 1));
}
