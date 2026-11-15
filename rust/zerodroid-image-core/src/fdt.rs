use core::ops::Range;

use crate::ParseBudget;

const FDT_MAGIC: u32 = 0xd00d_feed;
const FDT_HEADER_SIZE: usize = 40;
const FDT_BEGIN_NODE: u32 = 1;
const FDT_END_NODE: u32 = 2;
const FDT_PROP: u32 = 3;
const FDT_NOP: u32 = 4;
const FDT_END: u32 = 9;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FdtError {
    Truncated,
    InvalidMagic,
    UnsupportedVersion,
    InvalidHeader,
    InvalidRange,
    OverlappingBlocks,
    InvalidReservationMap,
    InvalidStructure,
    InvalidStringTable,
    InvalidUtf8,
    ResourceLimit,
    IntegerOverflow,
    MissingOverlaySubtree,
    MissingOverlayTarget,
    DuplicatePhandle,
    InvalidPhandle,
    MissingSymbols,
    MissingSymbol,
    InvalidSymbol,
    InvalidFixup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FdtHeader {
    pub total_size: u32,
    pub version: u32,
    pub last_compatible_version: u32,
    pub boot_cpuid_phys: u32,
    pub structure_size: u32,
    pub strings_size: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FdtReservation {
    pub address: u64,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FdtProperty {
    pub name: String,
    pub value: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FdtNode {
    pub name: String,
    pub path: String,
    pub parent: Option<usize>,
    pub properties: Vec<FdtProperty>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FdtIndex {
    pub header: FdtHeader,
    pub reservations: Vec<FdtReservation>,
    pub nodes: Vec<FdtNode>,
    pub root_compatible: Vec<String>,
    pub model: Option<String>,
    pub structure: Range<usize>,
    pub strings: Range<usize>,
}

fn be32(data: &[u8], offset: usize) -> Result<u32, FdtError> {
    let end = offset.checked_add(4).ok_or(FdtError::IntegerOverflow)?;
    let bytes = data.get(offset..end).ok_or(FdtError::Truncated)?;
    Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn be64(data: &[u8], offset: usize) -> Result<u64, FdtError> {
    let end = offset.checked_add(8).ok_or(FdtError::IntegerOverflow)?;
    let bytes = data.get(offset..end).ok_or(FdtError::Truncated)?;
    Ok(u64::from_be_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5], bytes[6], bytes[7],
    ]))
}

fn align4(value: usize) -> Result<usize, FdtError> {
    value.checked_add(3).map(|v| v & !3).ok_or(FdtError::IntegerOverflow)
}

fn checked_block(offset: u32, size: u32, total: usize) -> Result<Range<usize>, FdtError> {
    let start = usize::try_from(offset).map_err(|_| FdtError::IntegerOverflow)?;
    let length = usize::try_from(size).map_err(|_| FdtError::IntegerOverflow)?;
    let end = start.checked_add(length).ok_or(FdtError::IntegerOverflow)?;
    if start < FDT_HEADER_SIZE || end > total {
        return Err(FdtError::InvalidRange);
    }
    Ok(start..end)
}

fn c_string(bytes: &[u8]) -> Result<&str, FdtError> {
    let end = bytes.iter().position(|byte| *byte == 0).ok_or(FdtError::InvalidStructure)?;
    core::str::from_utf8(&bytes[..end]).map_err(|_| FdtError::InvalidUtf8)
}

fn read_string(strings: &[u8], offset: u32) -> Result<String, FdtError> {
    let start = usize::try_from(offset).map_err(|_| FdtError::IntegerOverflow)?;
    let tail = strings.get(start..).ok_or(FdtError::InvalidStringTable)?;
    let end = tail.iter().position(|byte| *byte == 0).ok_or(FdtError::InvalidStringTable)?;
    let value = core::str::from_utf8(&tail[..end]).map_err(|_| FdtError::InvalidUtf8)?;
    if value.is_empty() {
        return Err(FdtError::InvalidStringTable);
    }
    Ok(value.to_owned())
}

fn string_list(bytes: &[u8]) -> Result<Vec<String>, FdtError> {
    if bytes.is_empty() || *bytes.last().unwrap_or(&1) != 0 {
        return Err(FdtError::InvalidStructure);
    }
    let mut values = Vec::new();
    let mut start = 0;
    while start < bytes.len() {
        let relative_end = bytes[start..].iter().position(|byte| *byte == 0)
            .ok_or(FdtError::InvalidStructure)?;
        if relative_end == 0 {
            return Err(FdtError::InvalidStructure);
        }
        let end = start.checked_add(relative_end).ok_or(FdtError::IntegerOverflow)?;
        values.push(core::str::from_utf8(&bytes[start..end])
            .map_err(|_| FdtError::InvalidUtf8)?.to_owned());
        start = end.checked_add(1).ok_or(FdtError::IntegerOverflow)?;
    }
    Ok(values)
}

/// Parses an FDT/DTB into bounded structural indexes. Property payloads remain ranges
/// into the caller-owned input; no device-tree blob or property payload is copied.
pub fn parse_fdt(data: &[u8], budget: ParseBudget) -> Result<FdtIndex, FdtError> {
    budget.check_input(u64::try_from(data.len()).map_err(|_| FdtError::IntegerOverflow)?)
        .map_err(|_| FdtError::ResourceLimit)?;
    if data.len() < FDT_HEADER_SIZE {
        return Err(FdtError::Truncated);
    }
    if be32(data, 0)? != FDT_MAGIC {
        return Err(FdtError::InvalidMagic);
    }
    let header = FdtHeader {
        total_size: be32(data, 4)?,
        version: be32(data, 20)?,
        last_compatible_version: be32(data, 24)?,
        boot_cpuid_phys: be32(data, 28)?,
        strings_size: be32(data, 32)?,
        structure_size: be32(data, 36)?,
    };
    if header.version != 17
        || header.last_compatible_version > 17
        || header.last_compatible_version > header.version
    {
        return Err(FdtError::UnsupportedVersion);
    }
    let total = usize::try_from(header.total_size).map_err(|_| FdtError::IntegerOverflow)?;
    if total < FDT_HEADER_SIZE || total > data.len() {
        return Err(FdtError::InvalidHeader);
    }
    let struct_range = checked_block(be32(data, 8)?, header.structure_size, total)?;
    let strings_range = checked_block(be32(data, 12)?, header.strings_size, total)?;
    if struct_range.start % 4 != 0 || header.structure_size % 4 != 0 {
        return Err(FdtError::InvalidHeader);
    }
    let reserve_start = usize::try_from(be32(data, 16)?).map_err(|_| FdtError::IntegerOverflow)?;
    if reserve_start < FDT_HEADER_SIZE || reserve_start % 8 != 0 || reserve_start >= total {
        return Err(FdtError::InvalidReservationMap);
    }
    let mut reservations = Vec::new();
    let mut reserve_cursor = reserve_start;
    loop {
        let record_end = reserve_cursor.checked_add(16).ok_or(FdtError::IntegerOverflow)?;
        if record_end > total { return Err(FdtError::InvalidReservationMap); }
        let address = be64(data, reserve_cursor).map_err(|_| FdtError::InvalidReservationMap)?;
        let size_offset = reserve_cursor.checked_add(8).ok_or(FdtError::IntegerOverflow)?;
        let size = be64(data, size_offset).map_err(|_| FdtError::InvalidReservationMap)?;
        reserve_cursor = record_end;
        if address == 0 && size == 0 {
            break;
        }
        if size == 0 || address.checked_add(size).is_none() {
            return Err(FdtError::InvalidReservationMap);
        }
        if reservations.len() >= budget.max_items as usize {
            return Err(FdtError::ResourceLimit);
        }
        reservations.push(FdtReservation { address, size });
    }
    let reserve_range = reserve_start..reserve_cursor;
    let overlaps = |a: &Range<usize>, b: &Range<usize>| a.start < b.end && b.start < a.end;
    if overlaps(&struct_range, &strings_range)
        || overlaps(&struct_range, &reserve_range)
        || overlaps(&strings_range, &reserve_range)
    {
        return Err(FdtError::OverlappingBlocks);
    }

    let strings = &data[strings_range.clone()];
    let structure = &data[struct_range.clone()];
    let mut cursor = 0_usize;
    let mut nodes: Vec<FdtNode> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut saw_root = false;
    let mut saw_end = false;
    let mut item_count = reservations.len();
    while cursor < structure.len() {
        let token = be32(structure, cursor)?;
        cursor = cursor.checked_add(4).ok_or(FdtError::IntegerOverflow)?;
        match token {
            FDT_BEGIN_NODE => {
                item_count = item_count.checked_add(1).ok_or(FdtError::IntegerOverflow)?;
                if saw_end || item_count > budget.max_items as usize
                    || stack.len() >= budget.max_nesting_depth as usize
                {
                    return Err(FdtError::ResourceLimit);
                }
                let tail = structure.get(cursor..).ok_or(FdtError::Truncated)?;
                let name_end = tail.iter().position(|byte| *byte == 0).ok_or(FdtError::InvalidStructure)?;
                let name = core::str::from_utf8(&tail[..name_end])
                    .map_err(|_| FdtError::InvalidUtf8)?.to_owned();
                if (!saw_root && !name.is_empty()) || (saw_root && stack.is_empty()) {
                    return Err(FdtError::InvalidStructure);
                }
                let raw_end = cursor.checked_add(name_end).and_then(|v| v.checked_add(1))
                    .ok_or(FdtError::IntegerOverflow)?;
                let aligned_end = align4(raw_end)?;
                if structure.get(raw_end..aligned_end).ok_or(FdtError::Truncated)?
                    .iter().any(|byte| *byte != 0)
                {
                    return Err(FdtError::InvalidStructure);
                }
                cursor = aligned_end;
                let parent = stack.last().copied();
                let path = if let Some(parent_index) = parent {
                    let prefix = &nodes[parent_index].path;
                    if prefix == "/" { format!("/{name}") } else { format!("{prefix}/{name}") }
                } else {
                    "/".to_owned()
                };
                let index = nodes.len();
                nodes.push(FdtNode { name, path, parent, properties: Vec::new() });
                stack.push(index);
                saw_root = true;
            }
            FDT_END_NODE => {
                if stack.pop().is_none() {
                    return Err(FdtError::InvalidStructure);
                }
            }
            FDT_PROP => {
                let node_index = *stack.last().ok_or(FdtError::InvalidStructure)?;
                let length = usize::try_from(be32(structure, cursor)?)
                    .map_err(|_| FdtError::IntegerOverflow)?;
                let name_offset = be32(structure, cursor.checked_add(4).ok_or(FdtError::IntegerOverflow)?)?;
                cursor = cursor.checked_add(8).ok_or(FdtError::IntegerOverflow)?;
                let value_end = cursor.checked_add(length).ok_or(FdtError::IntegerOverflow)?;
                let value = cursor..value_end;
                let padded_end = align4(value_end)?;
                if padded_end > structure.len() {
                    return Err(FdtError::Truncated);
                }
                let name = read_string(strings, name_offset)?;
                item_count = item_count.checked_add(1).ok_or(FdtError::IntegerOverflow)?;
                if item_count > budget.max_items as usize { return Err(FdtError::ResourceLimit); }
                if nodes[node_index].properties.iter().any(|p| p.name == name) {
                    return Err(FdtError::InvalidStructure);
                }
                nodes[node_index].properties.push(FdtProperty {
                    name,
                    value: (struct_range.start + value.start)..(struct_range.start + value.end),
                });
                cursor = padded_end;
            }
            FDT_NOP => {}
            FDT_END => {
                if !saw_root || !stack.is_empty() || saw_end {
                    return Err(FdtError::InvalidStructure);
                }
                saw_end = true;
                if structure[cursor..].iter().any(|byte| *byte != 0) {
                    return Err(FdtError::InvalidStructure);
                }
                break;
            }
            _ => return Err(FdtError::InvalidStructure),
        }
    }
    if !saw_end || !stack.is_empty() {
        return Err(FdtError::InvalidStructure);
    }

    let root = nodes.first().ok_or(FdtError::InvalidStructure)?;
    let root_compatible = root.properties.iter()
        .find(|property| property.name == "compatible")
        .map(|property| {
            let bytes = data.get(property.value.clone()).ok_or(FdtError::InvalidRange)?;
            string_list(bytes)
        }).transpose()?.unwrap_or_default();
    let model = root.properties.iter().find(|property| property.name == "model")
        .map(|property| {
            let bytes = data.get(property.value.clone()).ok_or(FdtError::InvalidRange)?;
            let value = c_string(bytes)?;
            if value.len() + 1 != bytes.len() {
                return Err(FdtError::InvalidStructure);
            }
            Ok(value.to_owned())
        }).transpose()?;

    Ok(FdtIndex {
        header,
        reservations,
        nodes,
        root_compatible,
        model,
        structure: struct_range,
        strings: strings_range,
    })
}



#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FdtOverlayFragment {
    pub path: String,
    pub target_phandle: Option<u32>,
    pub target_path: Option<String>,
    pub has_overlay_subtree: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FdtOverlayIndex {
    pub fragments: Vec<FdtOverlayFragment>,
    pub has_external_fixups: bool,
    pub has_local_fixups: bool,
}

/// Exact compatible-string match against a caller-provided Guest platform allow-list.
/// No fuzzy prefix matching is performed.
pub fn matches_compatible(index: &FdtIndex, accepted: &[&str]) -> bool {
    index.root_compatible.iter().any(|value| accepted.iter().any(|candidate| value == candidate))
}

/// Extracts DTBO fragment routing metadata from an already validated FDT structure.
/// Applying overlays, resolving symbols/fixups and merging phandles remain separate operations.
pub fn index_overlay(data: &[u8], index: &FdtIndex, max_fragments: usize) -> Result<FdtOverlayIndex, FdtError> {
    let mut fragments = Vec::new();
    for (node_index, node) in index.nodes.iter().enumerate() {
        if node.parent != Some(0) || !node.name.starts_with("fragment@") {
            continue;
        }
        if fragments.len() >= max_fragments {
            return Err(FdtError::ResourceLimit);
        }
        let target_phandle = node.properties.iter()
            .find(|property| property.name == "target")
            .map(|property| {
                let bytes = data.get(property.value.clone()).ok_or(FdtError::InvalidRange)?;
                if bytes.len() != 4 { return Err(FdtError::InvalidStructure); }
                Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
            }).transpose()?;
        let target_path = node.properties.iter()
            .find(|property| property.name == "target-path")
            .map(|property| {
                let bytes = data.get(property.value.clone()).ok_or(FdtError::InvalidRange)?;
                let value = c_string(bytes)?;
                if value.len() + 1 != bytes.len() || !value.starts_with('/') {
                    return Err(FdtError::InvalidStructure);
                }
                Ok(value.to_owned())
            }).transpose()?;
        if target_phandle.is_some() && target_path.is_some() {
            return Err(FdtError::InvalidStructure);
        }
        if target_phandle.is_none() && target_path.is_none() {
            return Err(FdtError::InvalidStructure);
        }
        let has_overlay_subtree = index.nodes.iter().any(|candidate| {
            candidate.parent == Some(node_index) && candidate.name == "__overlay__"
        });
        fragments.push(FdtOverlayFragment {
            path: node.path.clone(),
            target_phandle,
            target_path,
            has_overlay_subtree,
        });
    }
    Ok(FdtOverlayIndex {
        fragments,
        has_external_fixups: index.nodes.iter().any(|node| node.path == "/__fixups__"),
        has_local_fixups: index.nodes.iter().any(|node| node.path == "/__local_fixups__"),
    })
}


#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FdtOverlayResolution {
    pub fragment_path: String,
    pub target_node_path: String,
    pub target_phandle: u32,
}

/// Resolves DTBO fragment targets against a base DTB without modifying either tree.
/// Phandle lookup is built once, sorted and checked for duplicate identifiers.
pub fn resolve_overlay_targets(
    base_data: &[u8],
    base: &FdtIndex,
    overlay: &FdtOverlayIndex,
) -> Result<Vec<FdtOverlayResolution>, FdtError> {
    let mut phandles: Vec<(u32, usize)> = Vec::new();
    for (node_index, node) in base.nodes.iter().enumerate() {
        let mut node_phandle = None;
        for property in &node.properties {
            if property.name != "phandle" && property.name != "linux,phandle" {
                continue;
            }
            let bytes = base_data.get(property.value.clone()).ok_or(FdtError::InvalidRange)?;
            if bytes.len() != 4 {
                return Err(FdtError::InvalidPhandle);
            }
            let value = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            if value == 0 || value == u32::MAX {
                return Err(FdtError::InvalidPhandle);
            }
            if node_phandle.is_some_and(|previous| previous != value) {
                return Err(FdtError::InvalidPhandle);
            }
            node_phandle = Some(value);
        }
        if let Some(value) = node_phandle {
            phandles.push((value, node_index));
        }
    }
    phandles.sort_unstable_by_key(|entry| entry.0);
    if phandles.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return Err(FdtError::DuplicatePhandle);
    }

    let mut resolved = Vec::with_capacity(overlay.fragments.len());
    for fragment in &overlay.fragments {
        if !fragment.has_overlay_subtree {
            return Err(FdtError::MissingOverlaySubtree);
        }
        let target_index = if let Some(path) = &fragment.target_path {
            base.nodes.iter().position(|node| &node.path == path)
                .ok_or(FdtError::MissingOverlayTarget)?
        } else if let Some(phandle) = fragment.target_phandle {
            let position = phandles.binary_search_by_key(&phandle, |entry| entry.0)
                .map_err(|_| FdtError::MissingOverlayTarget)?;
            phandles[position].1
        } else {
            return Err(FdtError::MissingOverlayTarget);
        };
        let target = &base.nodes[target_index];
        let target_phandle = phandles.iter()
            .find(|entry| entry.1 == target_index)
            .map(|entry| entry.0)
            .unwrap_or(0);
        resolved.push(FdtOverlayResolution {
            fragment_path: fragment.path.clone(),
            target_node_path: target.path.clone(),
            target_phandle,
        });
    }
    Ok(resolved)
}



/// Parses the DTB payload indexed by vendor_boot. The returned property ranges
/// continue to address the original vendor_boot byte slice.
pub fn parse_vendor_boot_dtb<'a>(
    vendor_boot_data: &'a [u8],
    vendor_boot: &crate::vendor_boot::VendorBootImageIndex,
    budget: ParseBudget,
) -> Result<FdtIndex, FdtError> {
    let range = vendor_boot.dtb.clone();
    if range.start >= range.end || range.end > vendor_boot_data.len() {
        return Err(FdtError::InvalidRange);
    }
    parse_fdt(&vendor_boot_data[range], budget)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<u8> {
        let mut strings = b"compatible\0model\0#address-cells\0".to_vec();
        let compatible_off = 0_u32;
        let model_off = 11_u32;
        let cells_off = 17_u32;
        let mut structure = Vec::new();
        let token = |out: &mut Vec<u8>, value: u32| out.extend_from_slice(&value.to_be_bytes());
        token(&mut structure, FDT_BEGIN_NODE);
        structure.extend_from_slice(&[0, 0, 0, 0]);
        token(&mut structure, FDT_PROP);
        structure.extend_from_slice(&24_u32.to_be_bytes());
        structure.extend_from_slice(&compatible_off.to_be_bytes());
        structure.extend_from_slice(b"vendor,board\0vendor,soc\0");
        token(&mut structure, FDT_PROP);
        structure.extend_from_slice(&12_u32.to_be_bytes());
        structure.extend_from_slice(&model_off.to_be_bytes());
        structure.extend_from_slice(b"Test Device\0");
        token(&mut structure, FDT_PROP);
        structure.extend_from_slice(&4_u32.to_be_bytes());
        structure.extend_from_slice(&cells_off.to_be_bytes());
        structure.extend_from_slice(&2_u32.to_be_bytes());
        token(&mut structure, FDT_END_NODE);
        token(&mut structure, FDT_END);
        while structure.len() % 4 != 0 { structure.push(0); }
        let reserve = vec![0_u8; 16];
        let reserve_off = FDT_HEADER_SIZE;
        let struct_off = reserve_off + reserve.len();
        let strings_off = struct_off + structure.len();
        let total = strings_off + strings.len();
        let mut data = vec![0_u8; FDT_HEADER_SIZE];
        data[0..4].copy_from_slice(&FDT_MAGIC.to_be_bytes());
        data[4..8].copy_from_slice(&(total as u32).to_be_bytes());
        data[8..12].copy_from_slice(&(struct_off as u32).to_be_bytes());
        data[12..16].copy_from_slice(&(strings_off as u32).to_be_bytes());
        data[16..20].copy_from_slice(&(reserve_off as u32).to_be_bytes());
        data[20..24].copy_from_slice(&17_u32.to_be_bytes());
        data[24..28].copy_from_slice(&16_u32.to_be_bytes());
        data[28..32].copy_from_slice(&0_u32.to_be_bytes());
        data[32..36].copy_from_slice(&(strings.len() as u32).to_be_bytes());
        data[36..40].copy_from_slice(&(structure.len() as u32).to_be_bytes());
        data.extend_from_slice(&reserve);
        data.extend_from_slice(&structure);
        data.append(&mut strings);
        data
    }

    fn budget() -> ParseBudget {
        ParseBudget { max_input_bytes: 1 << 20, max_items: 128, max_nesting_depth: 16 }
    }

    #[test]
    fn parses_root_platform_identity_and_nodes() {
        let data = fixture();
        let parsed = parse_fdt(&data, budget()).unwrap();
        assert_eq!(parsed.root_compatible, vec!["vendor,board", "vendor,soc"]);
        assert_eq!(parsed.model.as_deref(), Some("Test Device"));
        assert_eq!(parsed.nodes.len(), 1);
        assert_eq!(parsed.nodes[0].path, "/");
    }

    #[test]
    fn rejects_bad_magic_and_unsupported_version() {
        let mut data = fixture();
        data[0] = 0;
        assert_eq!(parse_fdt(&data, budget()), Err(FdtError::InvalidMagic));
        let mut data = fixture();
        data[20..24].copy_from_slice(&18_u32.to_be_bytes());
        assert_eq!(parse_fdt(&data, budget()), Err(FdtError::UnsupportedVersion));
    }

    #[test]
    fn rejects_overlapping_blocks_and_bad_reservation_map() {
        let mut data = fixture();
        let struct_off = be32(&data, 8).unwrap();
        data[12..16].copy_from_slice(&struct_off.to_be_bytes());
        assert_eq!(parse_fdt(&data, budget()), Err(FdtError::OverlappingBlocks));
        let mut data = fixture();
        data[16..20].copy_from_slice(&41_u32.to_be_bytes());
        assert_eq!(parse_fdt(&data, budget()), Err(FdtError::InvalidReservationMap));
    }

    #[test]
    fn enforces_node_and_input_budgets() {
        let data = fixture();
        let mut small = budget();
        small.max_items = 0;
        assert_eq!(parse_fdt(&data, small), Err(FdtError::ResourceLimit));
        let mut small = budget();
        small.max_input_bytes = 8;
        assert_eq!(parse_fdt(&data, small), Err(FdtError::ResourceLimit));
    }

    #[test]
    fn rejects_unbalanced_structure() {
        let mut data = fixture();
        let structure = be32(&data, 8).unwrap() as usize;
        data[structure..structure + 4].copy_from_slice(&FDT_END.to_be_bytes());
        assert_eq!(parse_fdt(&data, budget()), Err(FdtError::InvalidStructure));
    }

    #[test]
    fn matches_compatible_uses_exact_allow_list_entries() {
        let parsed = parse_fdt(&fixture(), budget()).unwrap();
        assert!(matches_compatible(&parsed, &["vendor,soc"]));
        assert!(!matches_compatible(&parsed, &["vendor"]));
    }

    #[test]
    fn overlay_index_reports_missing_fragments_without_claiming_application() {
        let parsed = parse_fdt(&fixture(), budget()).unwrap();
        let overlay = index_overlay(&fixture(), &parsed, 4).unwrap();
        assert!(overlay.fragments.is_empty());
        assert!(!overlay.has_external_fixups);
        assert!(!overlay.has_local_fixups);
    }


    fn base_with_phandle(values: &[u32]) -> (Vec<u8>, FdtIndex) {
        let mut data = Vec::new();
        let mut nodes = Vec::new();
        for (index, value) in values.iter().enumerate() {
            let start = data.len();
            data.extend_from_slice(&value.to_be_bytes());
            nodes.push(FdtNode {
                name: if index == 0 { String::new() } else { format!("node@{index}") },
                path: if index == 0 { "/".to_owned() } else { format!("/node@{index}") },
                parent: if index == 0 { None } else { Some(0) },
                properties: vec![FdtProperty { name: "phandle".to_owned(), value: start..start + 4 }],
            });
        }
        (data, FdtIndex {
            header: FdtHeader { total_size: 0, version: 17, last_compatible_version: 16,
                boot_cpuid_phys: 0, structure_size: 0, strings_size: 0 },
            reservations: Vec::new(), nodes, root_compatible: Vec::new(), model: None,
            structure: 0..0, strings: 0..0,
        })
    }

    #[test]
    fn resolves_overlay_phandle_to_base_node() {
        let (data, base) = base_with_phandle(&[1, 42]);
        let overlay = FdtOverlayIndex {
            fragments: vec![FdtOverlayFragment {
                path: "/fragment@0".to_owned(), target_phandle: Some(42),
                target_path: None, has_overlay_subtree: true,
            }],
            has_external_fixups: false, has_local_fixups: false,
        };
        let resolved = resolve_overlay_targets(&data, &base, &overlay).unwrap();
        assert_eq!(resolved[0].target_node_path, "/node@1");
        assert_eq!(resolved[0].target_phandle, 42);
    }

    #[test]
    fn resolves_overlay_target_path_exactly() {
        let (data, base) = base_with_phandle(&[1, 42]);
        let overlay = FdtOverlayIndex {
            fragments: vec![FdtOverlayFragment {
                path: "/fragment@0".to_owned(), target_phandle: None,
                target_path: Some("/node@1".to_owned()), has_overlay_subtree: true,
            }],
            has_external_fixups: false, has_local_fixups: false,
        };
        assert_eq!(resolve_overlay_targets(&data, &base, &overlay).unwrap()[0].target_node_path, "/node@1");
        let mut missing = overlay.clone();
        missing.fragments[0].target_path = Some("/node".to_owned());
        assert_eq!(resolve_overlay_targets(&data, &base, &missing), Err(FdtError::MissingOverlayTarget));
    }

    #[test]
    fn rejects_duplicate_invalid_and_missing_phandles() {
        let (data, duplicate) = base_with_phandle(&[1, 42, 42]);
        let overlay = FdtOverlayIndex {
            fragments: vec![FdtOverlayFragment {
                path: "/fragment@0".to_owned(), target_phandle: Some(42),
                target_path: None, has_overlay_subtree: true,
            }],
            has_external_fixups: false, has_local_fixups: false,
        };
        assert_eq!(resolve_overlay_targets(&data, &duplicate, &overlay), Err(FdtError::DuplicatePhandle));
        let (data, invalid) = base_with_phandle(&[u32::MAX]);
        assert_eq!(resolve_overlay_targets(&data, &invalid, &overlay), Err(FdtError::InvalidPhandle));
        let (data, base) = base_with_phandle(&[1]);
        assert_eq!(resolve_overlay_targets(&data, &base, &overlay), Err(FdtError::MissingOverlayTarget));
    }

    #[test]
    fn requires_overlay_subtree_before_resolution() {
        let (data, base) = base_with_phandle(&[1, 42]);
        let overlay = FdtOverlayIndex {
            fragments: vec![FdtOverlayFragment {
                path: "/fragment@0".to_owned(), target_phandle: Some(42),
                target_path: None, has_overlay_subtree: false,
            }],
            has_external_fixups: false, has_local_fixups: false,
        };
        assert_eq!(resolve_overlay_targets(&data, &base, &overlay), Err(FdtError::MissingOverlaySubtree));
    }

}


#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FdtExternalFixup {
    pub symbol: String,
    pub target_node_path: String,
    pub target_phandle: u32,
    pub overlay_property: String,
    pub overlay_property_range: Range<usize>,
    pub property_offset: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FdtLocalFixup {
    pub overlay_node_path: String,
    pub overlay_property: String,
    pub overlay_property_range: Range<usize>,
    pub property_offset: u32,
    pub original_phandle: u32,
    pub relocated_phandle: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FdtOverlayFixupPlan {
    pub external: Vec<FdtExternalFixup>,
    pub local: Vec<FdtLocalFixup>,
    pub base_max_phandle: u32,
    pub phandle_delta: u32,
}

/// Finds a node by its canonical absolute FDT path.
fn node_by_path<'a>(index: &'a FdtIndex, path: &str) -> Option<&'a FdtNode> {
    index.nodes.iter().find(|node| node.path == path)
}

fn property_by_name<'a>(node: &'a FdtNode, name: &str) -> Option<&'a FdtProperty> {
    node.properties.iter().find(|property| property.name == name)
}

fn read_be32_at(data: &[u8], range: &Range<usize>, offset: u32) -> Result<u32, FdtError> {
    let relative = usize::try_from(offset).map_err(|_| FdtError::IntegerOverflow)?;
    let start = range.start.checked_add(relative).ok_or(FdtError::IntegerOverflow)?;
    let end = start.checked_add(4).ok_or(FdtError::IntegerOverflow)?;
    if end > range.end {
        return Err(FdtError::InvalidFixup);
    }
    be32(data, start)
}

fn read_nul_strings(bytes: &[u8]) -> Result<Vec<&str>, FdtError> {
    if bytes.is_empty() || bytes.last() != Some(&0) {
        return Err(FdtError::InvalidFixup);
    }
    let mut result = Vec::new();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        let remaining = &bytes[cursor..];
        let length = remaining.iter().position(|byte| *byte == 0)
            .ok_or(FdtError::InvalidFixup)?;
        if length == 0 {
            return Err(FdtError::InvalidFixup);
        }
        let value = core::str::from_utf8(&remaining[..length])
            .map_err(|_| FdtError::InvalidUtf8)?;
        result.push(value);
        cursor = cursor.checked_add(length).and_then(|v| v.checked_add(1))
            .ok_or(FdtError::IntegerOverflow)?;
    }
    Ok(result)
}

fn parse_external_location(value: &str) -> Result<(&str, &str, u32), FdtError> {
    let mut parts = value.rsplitn(3, ':');
    let offset = parts.next().ok_or(FdtError::InvalidFixup)?;
    let property = parts.next().ok_or(FdtError::InvalidFixup)?;
    let path = parts.next().ok_or(FdtError::InvalidFixup)?;
    if path.is_empty() || !path.starts_with('/') || property.is_empty() || offset.is_empty()
        || !offset.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(FdtError::InvalidFixup);
    }
    let parsed = u32::from_str_radix(offset, 16).map_err(|_| FdtError::InvalidFixup)?;
    if parsed % 4 != 0 {
        return Err(FdtError::InvalidFixup);
    }
    Ok((path, property, parsed))
}

/// Builds a read-only fixup plan for one overlay against one base tree.
/// The caller must apply the returned writes to a separate bounded output buffer;
/// this function never mutates either input.
pub fn plan_overlay_fixups(
    base_data: &[u8],
    base: &FdtIndex,
    overlay_data: &[u8],
    overlay_index: &FdtIndex,
    budget: ParseBudget,
) -> Result<FdtOverlayFixupPlan, FdtError> {
    let mut base_phandles = Vec::new();
    for node in &base.nodes {
        let mut value = None;
        for property in &node.properties {
            if property.name == "phandle" || property.name == "linux,phandle" {
                let bytes = base_data.get(property.value.clone()).ok_or(FdtError::InvalidRange)?;
                if bytes.len() != 4 { return Err(FdtError::InvalidPhandle); }
                let current = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                if current == 0 || current == u32::MAX
                    || value.is_some_and(|previous| previous != current)
                {
                    return Err(FdtError::InvalidPhandle);
                }
                value = Some(current);
            }
        }
        if let Some(phandle) = value { base_phandles.push(phandle); }
    }
    base_phandles.sort_unstable();
    if base_phandles.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(FdtError::DuplicatePhandle);
    }
    let base_max_phandle = base_phandles.last().copied().unwrap_or(0);
    let phandle_delta = base_max_phandle;
    let mut external = Vec::new();
    let mut local = Vec::new();

    if let Some(fixup_node) = node_by_path(overlay_index, "/__fixups__") {
        let symbols = node_by_path(base, "/__symbols__").ok_or(FdtError::MissingSymbols)?;
        for fixup_property in &fixup_node.properties {
            let symbol_bytes = base_data.get(property_by_name(symbols, &fixup_property.name)
                .ok_or(FdtError::MissingSymbol)?.value.clone()).ok_or(FdtError::InvalidRange)?;
            let symbol_path = c_string(symbol_bytes)?;
            if symbol_path.len() + 1 != symbol_bytes.len() || !symbol_path.starts_with('/') {
                return Err(FdtError::InvalidSymbol);
            }
            let target = node_by_path(base, symbol_path).ok_or(FdtError::MissingOverlayTarget)?;
            let target_property = property_by_name(target, "phandle")
                .or_else(|| property_by_name(target, "linux,phandle"))
                .ok_or(FdtError::InvalidPhandle)?;
            let target_bytes = base_data.get(target_property.value.clone()).ok_or(FdtError::InvalidRange)?;
            if target_bytes.len() != 4 { return Err(FdtError::InvalidPhandle); }
            let target_phandle = u32::from_be_bytes([
                target_bytes[0], target_bytes[1], target_bytes[2], target_bytes[3],
            ]);
            if target_phandle == 0 || target_phandle == u32::MAX {
                return Err(FdtError::InvalidPhandle);
            }
            let locations = overlay_data.get(fixup_property.value.clone())
                .ok_or(FdtError::InvalidRange)?;
            for location in read_nul_strings(locations)? {
                let (path, property_name, offset) = parse_external_location(location)?;
                let overlay_node = node_by_path(overlay_index, path)
                    .ok_or(FdtError::InvalidFixup)?;
                let target_property = property_by_name(overlay_node, property_name)
                    .ok_or(FdtError::InvalidFixup)?;
                read_be32_at(overlay_data, &target_property.value, offset)?;
                external.push(FdtExternalFixup {
                    symbol: fixup_property.name.clone(),
                    target_node_path: symbol_path.to_owned(),
                    target_phandle,
                    overlay_property: property_name.to_owned(),
                    overlay_property_range: target_property.value.clone(),
                    property_offset: offset,
                });
                if external.len() > budget.max_items as usize {
                    return Err(FdtError::ResourceLimit);
                }
            }
        }
    }

    if let Some(local_root) = node_by_path(overlay_index, "/__local_fixups__") {
        for fixup_node in overlay_index.nodes.iter().filter(|node| {
            node.path.starts_with("/__local_fixups__/") && node.path != "/__local_fixups__"
        }) {
            let suffix = fixup_node.path.strip_prefix("/__local_fixups__")
                .ok_or(FdtError::InvalidFixup)?;
            let overlay_path = if suffix.is_empty() { "/" } else { suffix };
            let overlay_node = node_by_path(overlay_index, overlay_path)
                .ok_or(FdtError::InvalidFixup)?;
            for offset_property in &fixup_node.properties {
                let overlay_property = property_by_name(overlay_node, &offset_property.name)
                    .ok_or(FdtError::InvalidFixup)?;
                let encoded_offsets = overlay_data.get(offset_property.value.clone())
                    .ok_or(FdtError::InvalidRange)?;
                if encoded_offsets.is_empty() || encoded_offsets.len() % 4 != 0 {
                    return Err(FdtError::InvalidFixup);
                }
                for chunk in encoded_offsets.chunks_exact(4) {
                    let offset = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                    if offset % 4 != 0 {
                        return Err(FdtError::InvalidFixup);
                    }
                    let original_phandle = read_be32_at(
                        overlay_data, &overlay_property.value, offset,
                    )?;
                    if original_phandle == 0 || original_phandle == u32::MAX {
                        return Err(FdtError::InvalidPhandle);
                    }
                    let relocated_phandle = original_phandle.checked_add(phandle_delta)
                        .ok_or(FdtError::InvalidPhandle)?;
                    if relocated_phandle == 0 || relocated_phandle == u32::MAX {
                        return Err(FdtError::InvalidPhandle);
                    }
                    local.push(FdtLocalFixup {
                        overlay_node_path: overlay_path.to_owned(),
                        overlay_property: offset_property.name.clone(),
                        overlay_property_range: overlay_property.value.clone(),
                        property_offset: offset,
                        original_phandle,
                        relocated_phandle,
                    });
                    if local.len() > budget.max_items as usize {
                        return Err(FdtError::ResourceLimit);
                    }
                }
            }
        }
        let _ = local_root;
    }

    Ok(FdtOverlayFixupPlan {
        external,
        local,
        base_max_phandle,
        phandle_delta,
    })
}
