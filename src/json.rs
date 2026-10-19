//! Strict JSON input checks.
//!
//! `serde_json` silently keeps the last value when an object contains the same
//! key twice, so agent-authored operation payloads with copy-pasted keys change
//! meaning without warning. [`reject_duplicate_keys`] scans raw input bytes for
//! repeated keys before deserialization; full syntax validation stays with
//! `serde_json`.
use anyhow::{bail, Result};
use std::collections::HashSet;

const MAX_DEPTH: usize = 128;

struct Scanner<'a> {
    bytes: &'a [u8],
    pos: usize,
}

/// Fail when any JSON object in `bytes` repeats a key. Comparison is on raw
/// key bytes, so `"\u0061"` and `"a"` count as different keys.
pub fn reject_duplicate_keys(bytes: &[u8]) -> Result<()> {
    let mut scanner = Scanner { bytes, pos: 0 };
    scanner.skip_whitespace()?;
    scanner.parse_value(0)?;
    scanner.skip_whitespace()?;
    if scanner.pos != bytes.len() {
        bail!("[invalid-input] unexpected trailing characters in JSON input");
    }
    Ok(())
}

impl Scanner<'_> {
    fn peek(&self) -> Result<u8> {
        self.bytes
            .get(self.pos)
            .copied()
            .ok_or_else(|| anyhow::anyhow!("[invalid-input] unexpected end of JSON input"))
    }

    fn skip_whitespace(&mut self) -> Result<()> {
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
        Ok(())
    }

    fn expect(&mut self, byte: u8) -> Result<()> {
        if self.peek()? != byte {
            bail!("[invalid-input] invalid JSON input");
        }
        self.pos += 1;
        Ok(())
    }

    fn parse_value(&mut self, depth: usize) -> Result<()> {
        if depth > MAX_DEPTH {
            bail!("[invalid-input] JSON input is nested too deeply");
        }
        match self.peek()? {
            b'{' => self.parse_object(depth),
            b'[' => self.parse_array(depth),
            b'"' => {
                self.parse_key()?;
                Ok(())
            }
            b't' => self.parse_literal("true"),
            b'f' => self.parse_literal("false"),
            b'n' => self.parse_literal("null"),
            b'-' | b'0'..=b'9' => self.parse_number(),
            _ => bail!("[invalid-input] invalid JSON input"),
        }
    }

    fn parse_literal(&mut self, literal: &str) -> Result<()> {
        if self.bytes[self.pos..].starts_with(literal.as_bytes()) {
            self.pos += literal.len();
            Ok(())
        } else {
            bail!("[invalid-input] invalid JSON input")
        }
    }

    fn parse_number(&mut self) -> Result<()> {
        while self.pos < self.bytes.len()
            && matches!(
                self.bytes[self.pos],
                b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9'
            )
        {
            self.pos += 1;
        }
        Ok(())
    }

    /// Parse a string, returning its raw inner bytes. Escape sequences are
    /// passed through untouched; `serde_json` validates them afterwards.
    fn parse_key(&mut self) -> Result<Vec<u8>> {
        self.expect(b'"')?;
        let mut key = Vec::new();
        loop {
            match self.bytes.get(self.pos) {
                None => bail!("[invalid-input] unexpected end of JSON input"),
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(key);
                }
                Some(b'\\') => {
                    key.push(b'\\');
                    self.pos += 1;
                    let escaped = self.peek()?;
                    key.push(escaped);
                    self.pos += 1;
                }
                Some(byte) => {
                    key.push(*byte);
                    self.pos += 1;
                }
            }
        }
    }

    fn parse_object(&mut self, depth: usize) -> Result<()> {
        self.expect(b'{')?;
        self.skip_whitespace()?;
        if self.peek()? == b'}' {
            self.pos += 1;
            return Ok(());
        }
        let mut seen = HashSet::new();
        loop {
            self.skip_whitespace()?;
            let key = self.parse_key()?;
            if !seen.insert(key.clone()) {
                bail!(
                    "[invalid-input] duplicate JSON object key \"{}\"",
                    String::from_utf8_lossy(&key)
                );
            }
            self.skip_whitespace()?;
            self.expect(b':')?;
            self.skip_whitespace()?;
            self.parse_value(depth + 1)?;
            self.skip_whitespace()?;
            match self.peek()? {
                b',' => {
                    self.pos += 1;
                }
                b'}' => {
                    self.pos += 1;
                    return Ok(());
                }
                _ => bail!("[invalid-input] invalid JSON input"),
            }
        }
    }

    fn parse_array(&mut self, depth: usize) -> Result<()> {
        self.expect(b'[')?;
        self.skip_whitespace()?;
        if self.peek()? == b']' {
            self.pos += 1;
            return Ok(());
        }
        loop {
            self.skip_whitespace()?;
            self.parse_value(depth + 1)?;
            self.skip_whitespace()?;
            match self.peek()? {
                b',' => {
                    self.pos += 1;
                }
                b']' => {
                    self.pos += 1;
                    return Ok(());
                }
                _ => bail!("[invalid-input] invalid JSON input"),
            }
        }
    }
}

/// Round a measured float for emission in public agent APIs. Derived bounds
/// carry binary representation noise (e.g. `28.799999999999997` for a 28.8
/// height); three decimals is the documented output precision, well inside
/// document precision, so equivalent geometry emits equivalent JSON.
pub fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_json_without_duplicates() {
        reject_duplicate_keys(br#"[{"a":1,"b":[true,null]},"x"]"#).unwrap();
        reject_duplicate_keys(b"{}").unwrap();
        reject_duplicate_keys(b"[]").unwrap();
        reject_duplicate_keys(br#"{"a":{"b":{"c":[1,2]}}}"#).unwrap();
    }

    #[test]
    fn rejects_repeated_keys_at_any_depth() {
        let err = reject_duplicate_keys(br#"{"x":1,"x":2}"#).unwrap_err();
        assert!(err.to_string().contains("duplicate JSON object key \"x\""));
        let err = reject_duplicate_keys(br#"[{"a":{"b":1,"b":2}}]"#).unwrap_err();
        assert!(err.to_string().contains("\"b\""));
    }

    #[test]
    fn same_key_in_sibling_objects_is_fine() {
        reject_duplicate_keys(br#"[{"x":1},{"x":2}]"#).unwrap();
    }
}
