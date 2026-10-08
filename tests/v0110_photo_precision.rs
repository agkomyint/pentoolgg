//! High-bit-depth RGB paths (v0.11.0 item 2): 16-bit storage and the `f32`
//! working representation, checked against the pinned Display P3 fixture.
use pentool::photo::color::ColorSpace;
use pentool::photo::pixels::{from_working, to_working, Depth, Dither, Raster, Samples};
use pentool::photo::png;

fn fixture() -> Raster {
    let bytes = std::fs::read("docs/fixtures/photo-rgb16-p3.png").unwrap();
    let decoded = png::read(&bytes).unwrap();
    assert_eq!((decoded.raster.width, decoded.raster.height), (64, 8));
    assert!(!decoded.gray && !decoded.raster.alpha);
    decoded.raster
}

fn sixteen(raster: &Raster) -> &[u16] {
    match &raster.samples {
        Samples::Sixteen(values) => values,
        Samples::Eight(_) => panic!("16-bit path produced 8-bit samples"),
    }
}

fn convert(raster: &Raster, from: &str, to: &str) -> (Raster, u64) {
    let working = to_working(raster, &ColorSpace::named(from).unwrap()).unwrap();
    let (out, report) = from_working(
        &working,
        &ColorSpace::named(to).unwrap(),
        Depth::Sixteen,
        Dither::None,
    )
    .unwrap();
    (out, report.clipped_pixels)
}

#[test]
fn p3_fixture_round_trips_through_the_working_space_bit_exactly() {
    let source = fixture();
    let (back, clipped) = convert(&source, "display-p3", "display-p3");
    assert_eq!(back, source);
    assert_eq!(clipped, 0);
    // Re-encoding as PNG keeps 16 bits per sample.
    let encoded = png::write(&back).unwrap();
    assert_eq!(&encoded[24..26], &[16, 2], "IHDR bit depth and color type");
    assert_eq!(png::read(&encoded).unwrap().raster, source);
}

#[test]
fn sixteen_bit_paths_never_collapse_to_eight_bits() {
    let source = fixture();
    let (back, _) = convert(&source, "display-p3", "display-p3");
    // Codes 0..63 on the near-black row and the gray row's low bits survive.
    let row7 = &sixteen(&back)[7 * 64 * 3..8 * 64 * 3];
    assert!(row7.chunks(3).enumerate().all(|(x, p)| p == [x as u16; 3]));
    let distinct: std::collections::BTreeSet<u16> = sixteen(&back).iter().copied().collect();
    assert!(
        distinct.len() > 256,
        "only {} distinct codes",
        distinct.len()
    );
    assert!(distinct.iter().any(|code| code % 257 != 0));
}

#[test]
fn p3_through_prophoto_sixteen_bit_storage_stays_within_the_derived_bound() {
    let source = fixture();
    // After Bradford D65 -> D50, Display P3 red needs ProPhoto blue -0.00127,
    // so saturated P3 reds and cyans fall outside [0, 1] ProPhoto storage. The
    // unclamped f32 working space holds them; 16-bit ProPhoto files clip them.
    let p3 = ColorSpace::named("display-p3").unwrap();
    let red = pentool::photo::color::apply(
        &pentool::photo::color::multiply(
            &ColorSpace::named("prophoto").unwrap().from_working(),
            &p3.to_working(),
        ),
        [1.0, 0.0, 0.0],
    );
    assert!(red[2] < -0.001, "{red:?}");
    let rec2020 = ColorSpace::named("rec2020").unwrap();
    let red2020 = pentool::photo::color::apply(
        &pentool::photo::color::multiply(
            &ColorSpace::named("prophoto").unwrap().from_working(),
            &rec2020.to_working(),
        ),
        [1.0, 0.0, 0.0],
    );
    assert!(red2020.iter().any(|v| *v < 0.0), "{red2020:?}");
    let (prophoto, clipped) = convert(&source, "display-p3", "prophoto");
    assert!(clipped > 0);
    let (back, _) = convert(&prophoto, "prophoto", "display-p3");
    let linear = |code: u16| p3.transfer.decode(f64::from(code) / 65_535.0);
    let mut worst_code = 0;
    let mut worst_linear = 0.0f64;
    // Rows 0 (red) and 3 (cyan) clip; every other row must survive.
    for row in [1, 2, 4, 5, 6, 7] {
        let range = row * 192..(row + 1) * 192;
        for (a, b) in sixteen(&source)[range.clone()]
            .iter()
            .zip(&sixteen(&back)[range])
        {
            worst_code = worst_code.max(a.abs_diff(*b));
            worst_linear = worst_linear.max((linear(*a) - linear(*b)).abs());
        }
    }
    // Bound: half a ProPhoto 16-bit step at white (decode slope 1.8), through
    // the largest absolute row sum of the ProPhoto -> P3 matrix, plus half a
    // P3 step at white (decode slope 2.4 / 1.055). Measured: 3.29e-5, 9 codes.
    let back_matrix = pentool::photo::color::multiply(
        &p3.from_working(),
        &ColorSpace::named("prophoto").unwrap().to_working(),
    );
    let gain = back_matrix
        .iter()
        .map(|row| row.iter().map(|v| v.abs()).sum::<f64>())
        .fold(0.0, f64::max);
    let bound = gain * 1.8 * 0.5 / 65_535.0 + 2.4 / 1.055 * 0.5 / 65_535.0;
    assert!(worst_linear <= bound, "{worst_linear:e} > {bound:e}");
    assert!(worst_code <= 9, "{worst_code}");
    // Grays survive within three codes; the near-black row exactly.
    let gray = |raster: &Raster| sixteen(raster)[6 * 192..8 * 192].to_vec();
    let (source_gray, back_gray) = (gray(&source), gray(&back));
    assert!(source_gray
        .iter()
        .zip(&back_gray)
        .all(|(a, b)| a.abs_diff(*b) <= 3));
    assert_eq!(&source_gray[192..], &back_gray[192..]);
}

#[test]
fn out_of_srgb_colors_are_clipped_and_reported_not_wrapped() {
    let source = fixture();
    let (srgb, clipped) = convert(&source, "display-p3", "srgb");
    assert!(clipped > 0);
    // Full P3 green becomes full sRGB green, never a wrapped or negative code.
    let green = &sixteen(&srgb)[(64 + 63) * 3..(64 + 64) * 3];
    assert_eq!(green, [0, 65_535, 0]);
    // Grays are inside every gamut and stay gray.
    let row7 = &sixteen(&srgb)[7 * 64 * 3..8 * 64 * 3];
    assert!(row7.chunks(3).all(|p| p[0] == p[1] && p[1] == p[2]));
}
