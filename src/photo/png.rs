//! Bounded 8- and 16-bit PNG input and output for photo engine 1.
//!
//! Unlike `image::decode_source_pixels` (the v5 `srgb8` contract), this path
//! keeps 16-bit samples. Gray and gray-alpha PNGs expand losslessly to RGB(A).
//! Output PNGs are untagged; the caller records the color space separately.
use super::pixels::{check_surface, Raster, Samples};
use crate::image::MAX_SOURCE_BYTES;
use anyhow::{bail, Context, Result};
use image::{ColorType, DynamicImage, ImageBuffer, ImageDecoder};
use std::io::Cursor;

/// A decoded PNG with facts the caller must not lose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded {
    pub raster: Raster,
    /// EXIF orientation 1–8. Recorded, not applied: geometry applies it.
    pub orientation: u8,
    /// The file stored gray samples.
    pub gray: bool,
}

/// Decode an 8- or 16-bit PNG, refusing oversized input before decoding pixels.
pub fn read(bytes: &[u8]) -> Result<Decoded> {
    if bytes.is_empty() || bytes.len() as u64 > MAX_SOURCE_BYTES {
        bail!("[limit-exceeded] photo PNG must contain 1 byte–128 MiB");
    }
    if image::guess_format(bytes).ok() != Some(image::ImageFormat::Png) {
        bail!("[malformed-resource] photo source is not a PNG");
    }
    if bytes.windows(4).any(|part| part == b"acTL") {
        bail!("[unsupported-capability] animated PNG requires explicit frame selection");
    }
    let mut decoder = image::ImageReader::with_format(Cursor::new(bytes), image::ImageFormat::Png)
        .into_decoder()
        .context("[malformed-resource] could not initialize PNG decoder")?;
    let (width, height) = decoder.dimensions();
    check_surface(width, height)?;
    let color = decoder.color_type();
    let orientation = decoder
        .orientation()
        .context("[malformed-resource] invalid PNG orientation metadata")?
        .to_exif();
    let decoded =
        DynamicImage::from_decoder(decoder).context("[malformed-resource] PNG decode failed")?;
    if (decoded.width(), decoded.height()) != (width, height) {
        bail!("[malformed-resource] decoded PNG dimensions changed unexpectedly");
    }
    let gray = matches!(
        color,
        ColorType::L8 | ColorType::La8 | ColorType::L16 | ColorType::La16
    );
    let (alpha, samples) = match decoded {
        DynamicImage::ImageLuma8(_) | DynamicImage::ImageRgb8(_) => {
            (false, Samples::Eight(decoded.into_rgb8().into_raw()))
        }
        DynamicImage::ImageLumaA8(_) | DynamicImage::ImageRgba8(_) => {
            (true, Samples::Eight(decoded.into_rgba8().into_raw()))
        }
        DynamicImage::ImageLuma16(_) | DynamicImage::ImageRgb16(_) => {
            (false, Samples::Sixteen(decoded.into_rgb16().into_raw()))
        }
        DynamicImage::ImageLumaA16(_) | DynamicImage::ImageRgba16(_) => {
            (true, Samples::Sixteen(decoded.into_rgba16().into_raw()))
        }
        other => bail!(
            "[unsupported-capability] PNG sample type {:?} is not 8- or 16-bit integer",
            other.color()
        ),
    };
    Ok(Decoded {
        raster: Raster::new(width, height, alpha, samples)?,
        orientation,
        gray,
    })
}

/// Encode a raster as an RGB or RGBA PNG at its own bit depth.
pub fn write(raster: &Raster) -> Result<Vec<u8>> {
    let (width, height) = (raster.width, raster.height);
    let malformed =
        || anyhow::anyhow!("[malformed-resource] raster does not match {width}x{height}");
    let image = match (&raster.samples, raster.alpha) {
        (Samples::Eight(values), false) => DynamicImage::ImageRgb8(
            ImageBuffer::from_raw(width, height, values.clone()).ok_or_else(malformed)?,
        ),
        (Samples::Eight(values), true) => DynamicImage::ImageRgba8(
            ImageBuffer::from_raw(width, height, values.clone()).ok_or_else(malformed)?,
        ),
        (Samples::Sixteen(values), false) => DynamicImage::ImageRgb16(
            ImageBuffer::from_raw(width, height, values.clone()).ok_or_else(malformed)?,
        ),
        (Samples::Sixteen(values), true) => DynamicImage::ImageRgba16(
            ImageBuffer::from_raw(width, height, values.clone()).ok_or_else(malformed)?,
        ),
    };
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png)?;
    Ok(bytes.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sixteen_bit_png_round_trips_every_code() {
        let samples: Vec<u16> = (0..65_536u32)
            .flat_map(|c| [c as u16, (65_535 - c) as u16, (c * 7) as u16, c as u16])
            .collect();
        let raster = Raster::new(256, 256, true, Samples::Sixteen(samples)).unwrap();
        let encoded = write(&raster).unwrap();
        // IHDR bit depth byte: 16, color type 6 (RGBA).
        assert_eq!(&encoded[24..26], &[16, 6]);
        let decoded = read(&encoded).unwrap();
        assert_eq!(decoded.raster, raster);
        assert_eq!((decoded.orientation, decoded.gray), (1, false));
        assert_eq!(
            write(&raster).unwrap(),
            encoded,
            "encoding is deterministic"
        );
    }

    #[test]
    fn gray_sixteen_bit_expands_losslessly() {
        let gray = ImageBuffer::<image::Luma<u16>, _>::from_raw(2, 1, vec![1, 65_000]).unwrap();
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageLuma16(gray)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        let decoded = read(bytes.get_ref()).unwrap();
        assert!(decoded.gray);
        assert_eq!(
            decoded.raster.samples,
            Samples::Sixteen(vec![1, 1, 1, 65_000, 65_000, 65_000])
        );
    }

    #[test]
    fn hostile_input_is_refused() {
        assert!(read(b"")
            .unwrap_err()
            .to_string()
            .starts_with("[limit-exceeded]"));
        assert!(read(b"GIF89a....")
            .unwrap_err()
            .to_string()
            .starts_with("[malformed-resource]"));
        // A valid signature and IHDR claiming 40000x1 is refused before decode.
        let raster = Raster::new(1, 1, false, Samples::Eight(vec![1, 2, 3])).unwrap();
        let mut bytes = write(&raster).unwrap();
        bytes[16..20].copy_from_slice(&40_000u32.to_be_bytes());
        let crc = crc32(&bytes[12..29]);
        bytes[29..33].copy_from_slice(&crc.to_be_bytes());
        let error = read(&bytes).unwrap_err().to_string();
        assert!(error.starts_with("[limit-exceeded]"), "{error}");
    }

    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = !0u32;
        for byte in bytes {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }
}
