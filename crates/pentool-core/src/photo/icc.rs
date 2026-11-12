//! Deterministic matrix/TRC ICC v4 display profiles for the named RGB spaces.
//!
//! Output files carry the profile of their color space, generated here so no
//! profile file is needed at run time. The header date is fixed and the
//! profile ID is zero ("not calculated"), so the same space always produces
//! the same bytes.
use super::color::{self, ColorSpace, Transfer, D50};

/// ICC PCS illuminant (D50) as stored in the header, s15Fixed16.
const PCS_D50: [i32; 3] = [0x0000_f6d6, 0x0001_0000, 0x0000_d32d];

fn s15(value: f64) -> i32 {
    let scaled = value * 65536.0;
    // Round half away from zero; the values are small and finite.
    if scaled < 0.0 {
        -((-scaled + 0.5).floor() as i32)
    } else {
        (scaled + 0.5).floor() as i32
    }
}

fn push_s15(out: &mut Vec<u8>, value: f64) {
    out.extend_from_slice(&s15(value).to_be_bytes());
}

/// `mluc` with one en-US record.
fn mluc(text: &str) -> Vec<u8> {
    let utf16: Vec<u8> = text.encode_utf16().flat_map(u16::to_be_bytes).collect();
    let mut out = b"mluc".to_vec();
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&1u32.to_be_bytes());
    out.extend_from_slice(&12u32.to_be_bytes());
    out.extend_from_slice(b"enUS");
    out.extend_from_slice(&(utf16.len() as u32).to_be_bytes());
    out.extend_from_slice(&28u32.to_be_bytes());
    out.extend_from_slice(&utf16);
    out
}

fn xyz_tag(values: [f64; 3]) -> Vec<u8> {
    let mut out = b"XYZ ".to_vec();
    out.extend_from_slice(&[0; 4]);
    for v in values {
        push_s15(&mut out, v);
    }
    out
}

/// The `para` parameters of a transfer (decode direction), or `None` for PQ
/// and HLG, which a matrix/TRC profile cannot describe.
pub fn parametric(transfer: Transfer) -> Option<(u16, Vec<f64>)> {
    const BT_ALPHA: f64 = 1.099_296_826_809_44;
    const BT_BETA: f64 = 0.018_053_968_510_807;
    Some(match transfer {
        Transfer::Linear => (0, vec![1.0]),
        Transfer::Gamma(gamma) => (0, vec![gamma]),
        Transfer::Srgb => (
            3,
            vec![2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.040_45],
        ),
        Transfer::Romm => (3, vec![1.8, 1.0, 0.0, 1.0 / 16.0, 16.0 / 512.0]),
        Transfer::Bt709 => (
            3,
            vec![
                1.0 / 0.45,
                1.0 / BT_ALPHA,
                (BT_ALPHA - 1.0) / BT_ALPHA,
                1.0 / 4.5,
                4.5 * BT_BETA,
            ],
        ),
        Transfer::Pq | Transfer::Hlg => return None,
    })
}

fn para(kind: u16, params: &[f64]) -> Vec<u8> {
    let mut out = b"para".to_vec();
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&kind.to_be_bytes());
    out.extend_from_slice(&[0; 2]);
    for v in params {
        push_s15(&mut out, *v);
    }
    out
}

/// The ICC v4.3 display profile of `space`, or `None` when its transfer has
/// no parametric curve (PQ, HLG). The rendering intent field is `intent`
/// (0 perceptual, 1 relative colorimetric).
pub fn profile(space: &ColorSpace, intent: u32) -> Option<Vec<u8>> {
    let (kind, params) = parametric(space.transfer)?;
    let adapt = color::bradford(space.white, D50);
    let colorants = color::multiply(&adapt, &space.to_xyz());
    let column = |j: usize| [colorants[0][j], colorants[1][j], colorants[2][j]];
    let mut chad = b"sf32".to_vec();
    chad.extend_from_slice(&[0; 4]);
    for row in adapt {
        for v in row {
            push_s15(&mut chad, v);
        }
    }
    let trc = para(kind, &params);
    let tags: Vec<([u8; 4], Vec<u8>)> = vec![
        (*b"desc", mluc(&format!("pentool {}", space.id))),
        (*b"cprt", mluc("No copyright, use freely")),
        (*b"wtpt", xyz_tag(color::xyz(D50))),
        (*b"chad", chad),
        (*b"rXYZ", xyz_tag(column(0))),
        (*b"gXYZ", xyz_tag(column(1))),
        (*b"bXYZ", xyz_tag(column(2))),
        (*b"rTRC", trc.clone()),
        (*b"gTRC", trc.clone()),
        (*b"bTRC", trc),
    ];
    // The three TRC tags share one element.
    let mut table = Vec::new();
    let mut data: Vec<u8> = Vec::new();
    let start = 128 + 4 + 12 * tags.len();
    let mut trc_at: Option<(u32, u32)> = None;
    for (signature, body) in &tags {
        let shared = signature.ends_with(b"TRC");
        let (offset, size) = match (shared, trc_at) {
            (true, Some(at)) => at,
            _ => {
                while data.len() % 4 != 0 {
                    data.push(0);
                }
                let at = ((start + data.len()) as u32, body.len() as u32);
                data.extend_from_slice(body);
                if shared {
                    trc_at = Some(at);
                }
                at
            }
        };
        table.extend_from_slice(signature);
        table.extend_from_slice(&offset.to_be_bytes());
        table.extend_from_slice(&size.to_be_bytes());
    }
    while data.len() % 4 != 0 {
        data.push(0);
    }
    let size = (start + data.len()) as u32;
    let mut out = Vec::with_capacity(size as usize);
    out.extend_from_slice(&size.to_be_bytes());
    out.extend_from_slice(&[0; 4]); // preferred CMM
    out.extend_from_slice(&0x0430_0000u32.to_be_bytes());
    out.extend_from_slice(b"mntrRGB XYZ ");
    for part in [2026u16, 1, 1, 0, 0, 0] {
        out.extend_from_slice(&part.to_be_bytes());
    }
    out.extend_from_slice(b"acsp");
    out.extend_from_slice(&[0; 4]); // platform
    out.extend_from_slice(&[0; 4]); // flags
    out.extend_from_slice(&[0; 8]); // manufacturer, model
    out.extend_from_slice(&[0; 8]); // attributes
    out.extend_from_slice(&intent.to_be_bytes());
    for v in PCS_D50 {
        out.extend_from_slice(&v.to_be_bytes());
    }
    out.extend_from_slice(&[0; 4]); // creator
    out.extend_from_slice(&[0; 16]); // profile ID: not calculated
    out.extend_from_slice(&[0; 28]);
    out.extend_from_slice(&(tags.len() as u32).to_be_bytes());
    out.extend_from_slice(&table);
    out.extend_from_slice(&data);
    debug_assert_eq!(out.len(), size as usize);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag<'a>(icc: &'a [u8], signature: &[u8; 4]) -> &'a [u8] {
        let count = u32::from_be_bytes(icc[128..132].try_into().unwrap()) as usize;
        for i in 0..count {
            let entry = &icc[132 + 12 * i..144 + 12 * i];
            if &entry[..4] == signature {
                let offset = u32::from_be_bytes(entry[4..8].try_into().unwrap()) as usize;
                let size = u32::from_be_bytes(entry[8..12].try_into().unwrap()) as usize;
                return &icc[offset..offset + size];
            }
        }
        panic!("no tag {signature:?}")
    }

    fn s15_at(bytes: &[u8], at: usize) -> f64 {
        f64::from(i32::from_be_bytes(bytes[at..at + 4].try_into().unwrap())) / 65536.0
    }

    #[test]
    fn srgb_profile_has_published_colorants_and_curve() {
        let space = ColorSpace::named("srgb").unwrap();
        let icc = profile(&space, 0).unwrap();
        assert_eq!(&icc[36..40], b"acsp");
        assert_eq!(&icc[12..24], b"mntrRGB XYZ ");
        assert_eq!(
            u32::from_be_bytes(icc[0..4].try_into().unwrap()) as usize,
            icc.len()
        );
        // D50-adapted sRGB colorants as published by the ICC (sRGB v4 profile).
        let red = tag(&icc, b"rXYZ");
        let expected = [0.4361, 0.2225, 0.0139];
        for (i, e) in expected.iter().enumerate() {
            assert!((s15_at(red, 8 + 4 * i) - e).abs() < 1.5e-3, "{i}");
        }
        let trc = tag(&icc, b"rTRC");
        assert_eq!(&trc[..4], b"para");
        assert_eq!(u16::from_be_bytes([trc[8], trc[9]]), 3);
        assert!((s15_at(trc, 12) - 2.4).abs() < 1e-4);
        assert_eq!(
            tag(&icc, b"gTRC").as_ptr(),
            trc.as_ptr(),
            "TRCs share one element"
        );
        assert_eq!(profile(&space, 0).unwrap(), icc, "deterministic");
    }

    #[test]
    fn colorants_sum_to_the_pcs_white_and_hdr_has_no_profile() {
        for name in color::NAMED_SPACES {
            let space = ColorSpace::named(name).unwrap();
            let icc = profile(&space, 1).unwrap();
            assert_eq!(u32::from_be_bytes(icc[64..68].try_into().unwrap()), 1);
            let sum: Vec<f64> = (0..3)
                .map(|i| {
                    [b"rXYZ", b"gXYZ", b"bXYZ"]
                        .iter()
                        .map(|t| s15_at(tag(&icc, t), 8 + 4 * i))
                        .sum()
                })
                .collect();
            let d50 = color::xyz(D50);
            for i in 0..3 {
                assert!((sum[i] - d50[i]).abs() < 1e-4, "{name}: {sum:?}");
            }
        }
        let mut pq = ColorSpace::named("rec2020").unwrap();
        pq.transfer = Transfer::Pq;
        assert!(profile(&pq, 0).is_none());
    }
}
