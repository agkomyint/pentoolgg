//! Design systems: token inspection and versioned, lockable token libraries
//! (`docs/design-systems-v1.md`). Logic lives here; `main.rs` only routes.
// DesignError is a structured, self-describing error; it is only built on failure paths.
#![allow(clippy::result_large_err)]
pub mod cli;
pub mod library;
pub mod plan;

use crate::tokens::DesignError;
use serde_json::{json, Value};

pub const DEFAULT_LIMIT: usize = 100;
pub const MAX_LIMIT: usize = 1000;

/// Validate `--offset/--limit` (default limit 100, 1..=1000).
pub fn page_args(
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<(usize, usize), DesignError> {
    let limit = limit.unwrap_or(DEFAULT_LIMIT);
    if limit == 0 || limit > MAX_LIMIT {
        return Err(DesignError::new(
            "invalid-input",
            format!("--limit must be between 1 and {MAX_LIMIT}, got {limit}"),
        )
        .action(format!("use --limit 1..={MAX_LIMIT} and --offset to page")));
    }
    Ok((offset.unwrap_or(0), limit))
}

/// Slice `items` and describe exactly what was emitted.
pub fn paginate(items: Vec<Value>, offset: usize, limit: usize) -> (Vec<Value>, Value) {
    let matches = items.len();
    let page: Vec<Value> = items.into_iter().skip(offset).take(limit).collect();
    let meta = json!({
        "matches": matches,
        "returned": page.len(),
        "offset": offset,
        "limit": limit,
        "has_more": offset + page.len() < matches,
    });
    (page, meta)
}

/// Whole-string match where `*` stands for any run of characters.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let mut rest = text;
    for (index, part) in parts.iter().enumerate() {
        if index == 0 {
            match rest.strip_prefix(part) {
                Some(next) => rest = next,
                None => return false,
            }
        } else if index == parts.len() - 1 {
            return rest.ends_with(part);
        } else {
            match rest.find(part) {
                Some(at) => rest = &rest[at + part.len()..],
                None => return false,
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        assert!(glob_match("color.*", "color.accent"));
        assert!(!glob_match("color.*", "space.md"));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("a*c*e", "abcde"));
        assert!(!glob_match("a*c", "abd"));
    }

    #[test]
    fn pagination_fields_are_exact() {
        let items: Vec<Value> = (0..5).map(|n| json!(n)).collect();
        let (page, meta) = paginate(items.clone(), 3, 10);
        assert_eq!(page.len(), 2);
        assert_eq!(meta["has_more"], false);
        let (_, meta) = paginate(items, 0, 2);
        assert_eq!(meta["has_more"], true);
        assert!(page_args(None, Some(0)).is_err());
    }
}
