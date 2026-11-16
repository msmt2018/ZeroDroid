//! Bounded parser for the flat key/value subset used by Android bootconfig.
//!
//! Nested nodes, arrays, and shell-style inline comments are intentionally rejected until
//! their kernel bootconfig grammar and merge semantics are implemented explicitly.

use crate::ParseBudget;
use std::collections::BTreeMap;

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
    let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
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

        if let Some(existing) = seen.get(key) {
            if *existing != value {
                return Err(BootConfigError::DuplicateKeyConflict);
            }
            continue;
        }
        seen.insert(key, value);
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
    let mut merged = first.clone();
    let mut seen: BTreeMap<&str, &str> = first.entries.iter()
        .map(|entry| (entry.key.as_str(), entry.value.as_str())).collect();
    for entry in &second.entries {
        if let Some(existing) = seen.get(entry.key.as_str()) {
            if *existing != entry.value {
                return Err(BootConfigError::DuplicateKeyConflict);
            }
        } else {
            if merged.entries.len() >= max_entries {
                return Err(BootConfigError::ResourceLimit);
            }
            merged.entries.push(entry.clone());
            seen.insert(entry.key.as_str(), entry.value.as_str());
        }
    }
    Ok(merged)
}



/// Parses the bootconfig tail indexed by a vendor_boot v4 image parser.
pub fn parse_vendor_bootconfig(
    image: &[u8],
    index: &crate::vendor_boot::VendorBootImageIndex,
    budget: ParseBudget,
    limits: BootConfigLimits,
) -> Result<Option<BootConfig>, BootConfigError> {
    let Some(range) = &index.bootconfig else {
        return Ok(None);
    };
    if range.start > range.end || range.end > image.len() {
        return Err(BootConfigError::ResourceLimit);
    }
    parse_bootconfig(&image[range.clone()], budget, limits).map(Some)
}
\n#[cfg(test)]
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
    fn parses_vendor_bootconfig_tail_from_validated_range() {
        use crate::vendor_boot::{VendorBootHeaderV3V4, VendorBootImageIndex};
        let image = b"prefixandroidboot.hardware=demo\n";
        let index = VendorBootImageIndex {
            header: VendorBootHeaderV3V4 {
                header_version: 4, page_size: 4096, kernel_addr: 0, ramdisk_addr: 0,
                vendor_ramdisk_size: 0, header_size: 2128, dtb_size: 0, dtb_addr: 0,
                vendor_ramdisk_table_size: 0, vendor_ramdisk_table_entry_num: 0,
                vendor_ramdisk_table_entry_size: 108, bootconfig_size: 30,
            },
            header_page: 0..0, vendor_ramdisk: 0..0, dtb: 0..0,
            vendor_ramdisk_table: None, bootconfig: Some(6..image.len()),
            ramdisk_entries: Vec::new(), total_indexed_bytes: image.len() as u64,
        };
        let parsed = parse_vendor_bootconfig(
            image, &index, budget(), BootConfigLimits::default()
        ).unwrap().unwrap();
        assert_eq!(parsed.entries[0].key, "androidboot.hardware");
        assert_eq!(parsed.entries[0].value, "demo");
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
