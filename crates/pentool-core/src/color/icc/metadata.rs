//! Bounded display metadata. Original profile bytes remain authoritative.
use super::{range, signature, u16_at, u32_at, Profile};
use anyhow::{bail, ensure, Context, Result};
use serde::Serialize;

pub const MAX_TEXT_BYTES: usize = 8192;
pub const MAX_TEXT_RECORDS: usize = 64;
pub const MAX_TEXT_STORAGE: usize = 65536;

#[derive(Debug, Clone, Serialize)]
pub struct ProfileText {
    pub text: String,
    pub language: Option<String>,
    pub country: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Metadata {
    pub description: Option<ProfileText>,
    pub copyright: Option<ProfileText>,
    pub manufacturer: Option<ProfileText>,
    pub model: Option<ProfileText>,
}

fn unicode(bytes: &[u8]) -> Result<String> {
    ensure!(
        bytes.len() % 2 == 0,
        "[malformed-resource] ICC text has odd UTF-16 byte length"
    );
    ensure!(
        bytes.len() <= MAX_TEXT_BYTES,
        "[limit-exceeded] ICC text exceeds {MAX_TEXT_BYTES} bytes"
    );
    char::decode_utf16(
        bytes
            .chunks_exact(2)
            .map(|v| u16::from_be_bytes([v[0], v[1]])),
    )
    .map(|c| c.context("[malformed-resource] ICC text contains invalid UTF-16"))
    .collect()
}

fn ascii(bytes: &[u8]) -> Result<String> {
    ensure!(
        bytes.len() <= MAX_TEXT_BYTES,
        "[limit-exceeded] ICC text exceeds {MAX_TEXT_BYTES} bytes"
    );
    ensure!(
        bytes.last() == Some(&0)
            && bytes[..bytes.len() - 1]
                .iter()
                .all(|v| (1..=127).contains(v)),
        "[malformed-resource] ICC ASCII text needs a single terminal NUL and seven-bit characters"
    );
    Ok(String::from_utf8(bytes[..bytes.len() - 1].to_vec())?)
}

fn parse(bytes: &[u8]) -> Result<Option<ProfileText>> {
    range(bytes, 0, 8)?;
    ensure!(
        bytes[4..8] == [0; 4],
        "[malformed-resource] ICC text reserved bytes are nonzero"
    );
    let plain = |text| {
        Some(ProfileText {
            text,
            language: None,
            country: None,
        })
    };
    match &bytes[..4] {
        b"mluc" => {
            let count = u32_at(bytes, 8)? as usize;
            let stride = u32_at(bytes, 12)? as usize;
            ensure!(
                count <= MAX_TEXT_RECORDS,
                "[limit-exceeded] ICC text record count exceeds {MAX_TEXT_RECORDS}"
            );
            ensure!(
                stride >= 12,
                "[malformed-resource] ICC text record size is below 12"
            );
            let table_size = count
                .checked_mul(stride)
                .context("[malformed-resource] ICC text record size overflow")?;
            range(bytes, 16, table_size)?;
            let table_end = 16 + table_size;
            let mut total = 0usize;
            let mut selected = None;
            let mut best_rank = 3;
            for i in 0..count {
                let at = 16 + i * stride;
                let language = &bytes[at..at + 2];
                let country = &bytes[at + 2..at + 4];
                ensure!(
                    language.iter().all(u8::is_ascii_lowercase)
                        && country.iter().all(u8::is_ascii_uppercase),
                    "[malformed-resource] ICC text locale must contain two-letter language/country codes"
                );
                let length = u32_at(bytes, at + 4)? as usize;
                let offset = u32_at(bytes, at + 8)? as usize;
                ensure!(
                    offset >= table_end,
                    "[malformed-resource] ICC text offset overlaps records"
                );
                total = total
                    .checked_add(length)
                    .context("[limit-exceeded] ICC text storage overflow")?;
                ensure!(
                    total <= MAX_TEXT_STORAGE,
                    "[limit-exceeded] ICC text references exceed {MAX_TEXT_STORAGE} bytes"
                );
                let text = unicode(range(bytes, offset, length)?)?;
                ensure!(
                    !text.contains('\0'),
                    "[malformed-resource] ICC localized text must not contain NUL"
                );
                let rank = if language == b"en" {
                    if country == b"US" {
                        0
                    } else {
                        1
                    }
                } else {
                    2
                };
                if rank < best_rank {
                    selected = Some(ProfileText {
                        text,
                        language: Some(signature(language)),
                        country: Some(signature(country)),
                    });
                    best_rank = rank;
                }
            }
            Ok(selected)
        }
        b"text" => Ok(plain(ascii(&bytes[8..])?)),
        b"desc" => {
            // ICC v2 textDescriptionType: ASCII, optional Unicode, ScriptCode.
            let count = u32_at(bytes, 8)? as usize;
            let text = ascii(range(bytes, 12, count)?)?;
            let at = 12 + count;
            range(bytes, at, 8)?; // legacy Unicode language code and count
            let units = u32_at(bytes, at + 4)? as usize;
            ensure!(
                units <= MAX_TEXT_BYTES / 2,
                "[limit-exceeded] ICC legacy Unicode text is too long"
            );
            let encoded = range(bytes, at + 8, units * 2)?;
            if !encoded.is_empty() {
                ensure!(
                    encoded.ends_with(&[0, 0]),
                    "[malformed-resource] ICC legacy Unicode text lacks terminal NUL"
                );
                unicode(&encoded[..encoded.len() - 2])?;
            }
            let script = range(bytes, at + 8 + units * 2, 70)?;
            ensure!(
                script[2] <= 67,
                "[malformed-resource] ICC legacy ScriptCode count exceeds 67"
            );
            // The interoperable seven-bit ASCII description is deterministic;
            // legacy Unicode/ScriptCode payloads are never interpreted as UTF-8.
            let _ = u16_at(script, 0)?;
            Ok(plain(text))
        }
        kind => bail!(
            "[unsupported-capability] ICC display text type {}",
            signature(kind)
        ),
    }
}

impl Profile<'_> {
    pub fn metadata(&self) -> Result<Metadata> {
        let field = |tag: &[u8; 4]| -> Result<Option<ProfileText>> {
            self.tag(tag)
                .map(parse)
                .transpose()
                .map(Option::flatten)
                .with_context(|| format!("ICC metadata tag {}", signature(tag)))
        };
        Ok(Metadata {
            description: field(b"desc")?,
            copyright: field(b"cprt")?,
            manufacturer: field(b"dmnd")?,
            model: field(b"dmdd")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mluc(records: &[(&[u8; 4], &str)], stride: usize) -> Vec<u8> {
        let mut out = b"mluc\0\0\0\0".to_vec();
        out.extend((records.len() as u32).to_be_bytes());
        out.extend((stride as u32).to_be_bytes());
        out.resize(16 + records.len() * stride, 0);
        for (i, (locale, text)) in records.iter().enumerate() {
            let encoded: Vec<u8> = text.encode_utf16().flat_map(u16::to_be_bytes).collect();
            let at = 16 + i * stride;
            out[at..at + 4].copy_from_slice(*locale);
            out[at + 4..at + 8].copy_from_slice(&(encoded.len() as u32).to_be_bytes());
            let offset = out.len() as u32;
            out[at + 8..at + 12].copy_from_slice(&offset.to_be_bytes());
            out.extend(encoded);
        }
        out
    }

    #[test]
    fn locale_fallback_is_deterministic_and_record_stride_is_respected() {
        for stride in [12, 13, 16] {
            let bytes = mluc(
                &[
                    (b"frFR", "Français"),
                    (b"enGB", "British"),
                    (b"enUS", "English 🖨"),
                ],
                stride,
            );
            let text = parse(&bytes).unwrap().unwrap();
            assert_eq!(text.text, "English 🖨");
            assert_eq!(text.country.as_deref(), Some("US"));
        }
        assert_eq!(
            parse(&mluc(&[(b"frFR", "Français"), (b"enGB", "British")], 12))
                .unwrap()
                .unwrap()
                .text,
            "British"
        );
        assert_eq!(
            parse(&mluc(&[(b"frFR", "Français")], 12))
                .unwrap()
                .unwrap()
                .text,
            "Français"
        );
        assert!(parse(&mluc(&[], 12)).unwrap().is_none());
    }

    #[test]
    fn every_truncation_bad_offset_and_invalid_unicode_is_refused() {
        let bytes = mluc(&[(b"enUS", "Press")], 12);
        for end in 0..bytes.len() {
            assert!(parse(&bytes[..end]).is_err(), "prefix {end}");
        }
        for (at, value) in [(8, 65), (12, 11), (20, 8194), (24, 16), (24, u32::MAX)] {
            let mut bad = bytes.clone();
            bad[at..at + 4].copy_from_slice(&value.to_be_bytes());
            assert!(parse(&bad).is_err(), "field {at} value {value}");
        }
        let mut bad = bytes.clone();
        bad[28..30].copy_from_slice(&0xd800u16.to_be_bytes());
        assert!(parse(&bad).is_err());
        let mut bad = bytes.clone();
        bad[4] = 1;
        assert!(parse(&bad).is_err());
        let mut bad = bytes;
        bad[28..30].fill(0);
        assert!(parse(&bad).is_err());
    }

    #[test]
    fn shared_localized_strings_work_and_total_references_are_bounded() {
        let mut bytes = mluc(&[(b"enGB", "Press"), (b"enUS", "Press")], 12);
        let first = bytes[24..28].to_vec();
        bytes[36..40].copy_from_slice(&first);
        assert_eq!(parse(&bytes).unwrap().unwrap().text, "Press");
        let text = "x".repeat(4096);
        let records = vec![(b"enUS", text.as_str()); 9];
        assert!(parse(&mluc(&records, 12))
            .unwrap_err()
            .to_string()
            .contains("limit-exceeded"));
    }

    #[test]
    fn legacy_ascii_and_description_require_their_full_payloads() {
        let mut text = b"text\0\0\0\0Copyright\0".to_vec();
        assert_eq!(parse(&text).unwrap().unwrap().text, "Copyright");
        text.pop();
        assert!(parse(&text).is_err());
        assert!(parse(b"text\0\0\0\0").is_err());
        let mut desc = b"desc\0\0\0\0".to_vec();
        desc.extend(6u32.to_be_bytes());
        desc.extend(b"Press\0");
        desc.extend([0; 8]);
        desc.extend([0; 70]);
        assert_eq!(parse(&desc).unwrap().unwrap().text, "Press");
        for end in 0..desc.len() {
            assert!(parse(&desc[..end]).is_err(), "prefix {end}");
        }
        *desc.last_mut().unwrap() = 200; // Unused ScriptCode bytes are opaque.
        assert!(parse(&desc).is_ok());
        let at = desc.len() - 68;
        desc[at] = 68;
        assert!(parse(&desc).is_err());
    }
}
