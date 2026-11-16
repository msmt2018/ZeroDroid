//! Bounded parser for the flat key/value subset used by Android bootconfig.
//!
//! Nested nodes, arrays, and shell-style inline comments are intentionally rejected until
//! their kernel bootconfig grammar and merge semantics are implemented explicitly.

use crate::ParseBudget;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootConfigError {
    ResourceLimit,
    InvalidUtf8,
    InvalidLine,
    InvalidKey,
    InvalidValue,
    DuplicateKeyConflict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootConfigLimits {
    pub max_line_bytes: usize,
    pub max_key_bytes: usize,
    pub max_value_bytes: usize,
}

impl Default for BootConfigLimits {
    fn default() -> Self {
        Self { max_line_bytes: 4096, max_key_bytes: 256, max_value_bytes: 2048 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootConfigEntry {
    pub key: String,
    /// Value is retained as text; no shell expansion or type coercion is performed.
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BootConfig {
    pub entries: Vec<BootConfigEntry>,
}

/// Parses unique flat `key=value` lines. Identical duplicate keys are deduplicated;
/// conflicting duplicates fail closed. The source bytes are never modified.
pub fn parse_bootconfig(
    data: &[u8],
    budget: ParseBudget,
    limits: BootConfigLimits,
) -> Result<BootConfig, BootConfigError> {
    budget.check_input(u64::try_from(data.len()).map_err(|_| BootConfigError::ResourceLimit)?)
        .map_err(|_| BootConfigError::ResourceLimit)?;
    let text = core::str::from_utf8(data).map_err(|_| BootConfigError::InvalidUtf8)?;
    let mut entries: Vec<BootConfigEntry> = Vec::new();
    let mut line_count = 0_u32;

    for raw_line in text.lines() {
        line_count = line_count.checked_add(1).ok_or(BootConfigError::ResourceLimit)?;
        budget.check_items(line_count).map_err(|_| BootConfigError::ResourceLimit)?;
        if raw_line.len() > limits.max_line_bytes {
            return Err(BootConfigError::ResourceLimit);
        }
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (raw_key, raw_value) = line.split_once('=').ok_or(BootConfigError::InvalidLine)?;
        let key = raw_key.trim();
        let value = raw_value.trim();
        if key.is_empty() || key.len() > limits.max_key_bytes {
            return Err(if key.is_empty() { BootConfigError::InvalidKey } else { BootConfigError::ResourceLimit });
        }
        if !key.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-')) {
            return Err(BootConfigError::InvalidKey);
        }
        if value.is_empty() {
            return Err(BootConfigError::InvalidValue);
        }
        if value.len() > limits.max_value_bytes {
            return Err(BootConfigError::ResourceLimit);
        }
        // Keep quoted strings byte-for-byte (apart from surrounding whitespace), but reject
        // unmatched quotes so malformed values cannot silently enter the boot argument set.
        let starts_quote = value.starts_with('"');
        let ends_quote = value.ends_with('"');
        if starts_quote != ends_quote || (value.starts_with('"') && value.len() < 2) {
            return Err(BootConfigError::InvalidValue);
        }

        if let Some(existing) = entries.iter().find(|entry| entry.key == key) {
            if existing.value != value {
                return Err(BootConfigError::DuplicateKeyConflict);
            }
            continue;
        }
        entries.push(BootConfigEntry { key: key.to_owned(), value: value.to_owned() });
    }

    Ok(BootConfig { entries })
}

/// Merges two already parsed sources without inventing source precedence.
/// A key present in both sources must have exactly the same value.
pub fn merge_bootconfig(
    first: &BootConfig,
    second: &BootConfig,
    max_entries: usize,
) -> Result<BootConfig, BootConfigError> {
    if first.entries.len().checked_add(second.entries.len()).ok_or(BootConfigError::ResourceLimit)? > max_entries {
        return Err(BootConfigError::ResourceLimit);
    }
    let mut merged = first.clone();
    for entry in &second.entries {
        if let Some(existing) = merged.entries.iter().find(|item| item.key == entry.key) {
            if existing.value != entry.value {
                return Err(BootConfigError::DuplicateKeyConflict);
            }
        } else {
            if merged.entries.len() >= max_entries {
                return Err(BootConfigError::ResourceLimit);
            }
            merged.entries.push(entry.clone());
        }
    }
    Ok(merged)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budget() -> ParseBudget {
        ParseBudget { max_input_bytes: 4096, max_items: 32, max_nesting_depth: 4 }
    }

    #[test]
    fn parses_flat_values_and_ignores_blank_or_comment_lines() {
        let parsed = parse_bootconfig(
            b"# generated\n\nandroidboot.hardware = demo\nandroidboot.mode=normal\n",
            budget(), BootConfigLimits::default()
        ).unwrap();
        assert_eq!(parsed.entries.len(), 2);
        assert_eq!(parsed.entries[0].key, "androidboot.hardware");
        assert_eq!(parsed.entries[0].value, "demo");
    }

    #[test]
    fn deduplicates_identical_keys_and_rejects_conflicts() {
        let same = parse_bootconfig(b"a=1\na=1\n", budget(), BootConfigLimits::default()).unwrap();
        assert_eq!(same.entries.len(), 1);
        assert_eq!(
            parse_bootconfig(b"a=1\na=2\n", budget(), BootConfigLimits::default()),
            Err(BootConfigError::DuplicateKeyConflict)
        );
    }

    #[test]
    fn rejects_unsupported_or_malformed_syntax() {
        assert_eq!(parse_bootconfig(b"node {\n", budget(), BootConfigLimits::default()), Err(BootConfigError::InvalidLine));
        assert_eq!(parse_bootconfig(b"bad key=1\n", budget(), BootConfigLimits::default()), Err(BootConfigError::InvalidKey));
        assert_eq!(parse_bootconfig(b"a=\"unterminated\n", budget(), BootConfigLimits::default()), Err(BootConfigError::InvalidValue));
    }

    #[test]
    fn enforces_input_line_and_item_budgets() {
        let mut limits = BootConfigLimits::default();
        limits.max_line_bytes = 3;
        assert_eq!(parse_bootconfig(b"long=value\n", budget(), limits), Err(BootConfigError::ResourceLimit));
        let mut small = budget();
        small.max_items = 1;
        assert_eq!(parse_bootconfig(b"a=1\nb=2\n", small, BootConfigLimits::default()), Err(BootConfigError::ResourceLimit));
    }

    #[test]
    fn merge_is_ordered_and_rejects_conflicting_source_values() {
        let first = parse_bootconfig(b"a=1\nb=2\n", budget(), BootConfigLimits::default()).unwrap();
        let second = parse_bootconfig(b"b=2\nc=3\n", budget(), BootConfigLimits::default()).unwrap();
        let merged = merge_bootconfig(&first, &second, 4).unwrap();
        assert_eq!(merged.entries.iter().map(|e| e.key.as_str()).collect::<Vec<_>>(), vec!["a", "b", "c"]);
        let conflict = parse_bootconfig(b"b=other\n", budget(), BootConfigLimits::default()).unwrap();
        assert_eq!(merge_bootconfig(&first, &conflict, 4), Err(BootConfigError::DuplicateKeyConflict));
    }
}
