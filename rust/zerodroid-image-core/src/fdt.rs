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
    FixupWriteConflict,
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

fn checked_block(offset: u32, size: u32, total: usize, header_size: usize) -> Result<Range<usize>, FdtError> {
    let start = usize::try_from(offset).map_err(|_| FdtError::IntegerOverflow)?;
    let length = usize::try_from(size).map_err(|_| FdtError::IntegerOverflow)?;
    let end = start.checked_add(length).ok_or(FdtError::IntegerOverflow)?;
    if start < header_size || end > total {
        return Err(FdtError::InvalidRange);
    }
    Ok(start..end)
}

fn scan_fdt_structure_end(data: &[u8], start: usize, limit: usize) -> Result<usize, FdtError> {
    let mut cursor = start;
    let mut depth = 0usize;
    let mut saw_root = false;
    while cursor.checked_add(4).ok_or(FdtError::IntegerOverflow)? <= limit {
        let token = be32(data, cursor)?;
        cursor = cursor.checked_add(4).ok_or(FdtError::IntegerOverflow)?;
        match token {
            FDT_BEGIN_NODE => {
                let tail = data.get(cursor..limit).ok_or(FdtError::Truncated)?;
                let name_end = tail.iter().position(|byte| *byte == 0).ok_or(FdtError::InvalidStructure)?;
                if !saw_root && name_end != 0 { return Err(FdtError::InvalidStructure); }
                let raw_end = cursor.checked_add(name_end).and_then(|v| v.checked_add(1))
                    .ok_or(FdtError::IntegerOverflow)?;
                cursor = align4(raw_end)?;
                if cursor > limit { return Err(FdtError::Truncated); }
                depth = depth.checked_add(1).ok_or(FdtError::IntegerOverflow)?;
                saw_root = true;
            }
            FDT_END_NODE => {
                if depth == 0 { return Err(FdtError::InvalidStructure); }
                depth -= 1;
            }
            FDT_PROP => {
                if depth == 0 { return Err(FdtError::InvalidStructure); }
                let length = usize::try_from(be32(data, cursor)?).map_err(|_| FdtError::IntegerOverflow)?;
                cursor = cursor.checked_add(8).ok_or(FdtError::IntegerOverflow)?;
                cursor = align4(cursor.checked_add(length).ok_or(FdtError::IntegerOverflow)?)?;
                if cursor > limit { return Err(FdtError::Truncated); }
            }
            FDT_NOP => {}
            FDT_END => {
                if !saw_root || depth != 0 { return Err(FdtError::InvalidStructure); }
                return Ok(cursor);
            }
            _ => return Err(FdtError::InvalidStructure),
        }
    }
    Err(FdtError::Truncated)
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
    if data.len() < 36 {
        return Err(FdtError::Truncated);
    }
    if be32(data, 0)? != FDT_MAGIC {
        return Err(FdtError::InvalidMagic);
    }
    let version = be32(data, 20)?;
    let header_size = match version {
        16 => 36usize,
        17 => FDT_HEADER_SIZE,
        _ => return Err(FdtError::UnsupportedVersion),
    };
    if data.len() < header_size { return Err(FdtError::Truncated); }
    let total_size = be32(data, 4)?;
    let total = usize::try_from(total_size).map_err(|_| FdtError::IntegerOverflow)?;
    if total < header_size || total > data.len() { return Err(FdtError::InvalidHeader); }
    let last_compatible_version = be32(data, 24)?;
    if last_compatible_version > version || last_compatible_version > 17 {
        return Err(FdtError::UnsupportedVersion);
    }
    let structure_offset = be32(data, 8)?;
    let strings_offset = be32(data, 12)?;
    let reserve_start = usize::try_from(be32(data, 16)?).map_err(|_| FdtError::IntegerOverflow)?;
    let strings_size = be32(data, 32)?;
    let strings_range = checked_block(strings_offset, strings_size, total, header_size)?;
    let struct_start = usize::try_from(structure_offset).map_err(|_| FdtError::IntegerOverflow)?;
    if struct_start < header_size || struct_start % 4 != 0 || struct_start >= total {
        return Err(FdtError::InvalidRange);
    }
    if reserve_start < header_size || reserve_start % 8 != 0 || reserve_start >= total {
        return Err(FdtError::InvalidReservationMap);
    }
    let structure_size = if version == 17 {
        be32(data, 36)?
    } else {
        let boundary = [strings_range.start, reserve_start, total].into_iter()
            .filter(|offset| *offset > struct_start).min().ok_or(FdtError::InvalidRange)?;
        let end = scan_fdt_structure_end(data, struct_start, boundary)?;
        u32::try_from(end - struct_start).map_err(|_| FdtError::IntegerOverflow)?
    };
    let struct_range = checked_block(structure_offset, structure_size, total, header_size)?;
    if struct_range.start % 4 != 0 || structure_size % 4 != 0 {
        return Err(FdtError::InvalidHeader);
    }
    let header = FdtHeader {
        total_size,
        version,
        last_compatible_version,
        boot_cpuid_phys: be32(data, 28)?,
        structure_size,
        strings_size,
    };
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
    pub declarations: Vec<FdtLocalFixup>,
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
    let mut declarations = Vec::new();
    let mut overlay_declared_phandles = Vec::new();
    for node in &overlay_index.nodes {
        let mut node_phandle = None;
        for property in &node.properties {
            if property.name != "phandle" && property.name != "linux,phandle" { continue; }
            let bytes = overlay_data.get(property.value.clone()).ok_or(FdtError::InvalidRange)?;
            if bytes.len() != 4 { return Err(FdtError::InvalidPhandle); }
            let original = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            if original == 0 || original == u32::MAX
                || node_phandle.is_some_and(|previous| previous != original)
            { return Err(FdtError::InvalidPhandle); }
            node_phandle = Some(original);
            let relocated = original.checked_add(phandle_delta).ok_or(FdtError::InvalidPhandle)?;
            if relocated == 0 || relocated == u32::MAX { return Err(FdtError::InvalidPhandle); }
            declarations.push(FdtLocalFixup {
                overlay_node_path: node.path.clone(), overlay_property: property.name.clone(),
                overlay_property_range: property.value.clone(), property_offset: 0,
                original_phandle: original, relocated_phandle: relocated,
            });
        }
        if let Some(value) = node_phandle { overlay_declared_phandles.push(value); }
    }
    overlay_declared_phandles.sort_unstable();
    if overlay_declared_phandles.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(FdtError::DuplicatePhandle);
    }

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

    let total_fixups = external.len().checked_add(local.len()).and_then(|n| n.checked_add(declarations.len()))
        .ok_or(FdtError::IntegerOverflow)?;
    if total_fixups > budget.max_items as usize { return Err(FdtError::ResourceLimit); }
    Ok(FdtOverlayFixupPlan {
        external,
        local,
        declarations,
        base_max_phandle,
        phandle_delta,
    })
}


#[cfg(test)]
mod fixup_tests {
    use super::*;

    fn node(path: &str, parent: Option<usize>, properties: Vec<FdtProperty>) -> FdtNode {
        FdtNode {
            name: path.rsplit('/').next().unwrap_or("").to_owned(),
            path: path.to_owned(),
            parent,
            properties,
        }
    }

    fn prop(name: &str, start: usize, len: usize) -> FdtProperty {
        FdtProperty { name: name.to_owned(), value: start..start + len }
    }

    fn base_fixture() -> (Vec<u8>, FdtIndex) {
        let mut data = b"/soc\0".to_vec();
        data.resize(8, 0);
        data.extend_from_slice(&7_u32.to_be_bytes());
        data.extend_from_slice(&100_u32.to_be_bytes());
        let index = FdtIndex {
            header: FdtHeader { total_size: 0, version: 17, last_compatible_version: 16,
                boot_cpuid_phys: 0, structure_size: 0, strings_size: 0 },
            reservations: Vec::new(),
            nodes: vec![
                node("/", None, vec![]),
                node("/__symbols__", Some(0), vec![prop("clk", 0, 5)]),
                node("/soc", Some(0), vec![prop("phandle", 8, 4)]),
                node("/other", Some(0), vec![prop("phandle", 12, 4)]),
            ],
            root_compatible: Vec::new(), model: None, structure: 0..0, strings: 0..0,
        };
        (data, index)
    }

    fn overlay_fixture() -> (Vec<u8>, FdtIndex) {
        let location = b"/fragment@0/__overlay__/dev:clocks:0\0";
        let mut data = location.to_vec();
        let external_value = data.len();
        data.extend_from_slice(&2_u32.to_be_bytes());
        let local_offset = data.len();
        data.extend_from_slice(&0_u32.to_be_bytes());
        let local_value = data.len();
        data.extend_from_slice(&2_u32.to_be_bytes());
        let declared_phandle = data.len();
        data.extend_from_slice(&2_u32.to_be_bytes());
        let index = FdtIndex {
            header: FdtHeader { total_size: 0, version: 17, last_compatible_version: 16,
                boot_cpuid_phys: 0, structure_size: 0, strings_size: 0 },
            reservations: Vec::new(),
            nodes: vec![
                node("/", None, vec![]),
                node("/__fixups__", Some(0), vec![prop("clk", 0, location.len())]),
                node("/fragment@0", Some(0), vec![]),
                node("/fragment@0/__overlay__", Some(2), vec![]),
                node("/fragment@0/__overlay__/dev", Some(3), vec![
                    prop("clocks", external_value, 4),
                    prop("local-ref", local_value, 4),
                    prop("phandle", declared_phandle, 4),
                ]),
                node("/__local_fixups__", Some(0), vec![]),
                node("/__local_fixups__/fragment@0", Some(5), vec![]),
                node("/__local_fixups__/fragment@0/__overlay__", Some(6), vec![]),
                node("/__local_fixups__/fragment@0/__overlay__/dev", Some(7), vec![
                    prop("local-ref", local_offset, 4),
                ]),
            ],
            root_compatible: Vec::new(), model: None, structure: 0..0, strings: 0..0,
        };
        (data, index)
    }

    fn budget() -> ParseBudget {
        ParseBudget { max_input_bytes: 4096, max_items: 32, max_nesting_depth: 8 }
    }

    #[test]
    fn plans_external_symbol_resolution_and_local_phandle_relocation() {
        let (base_data, base) = base_fixture();
        let (overlay_data, overlay) = overlay_fixture();
        let plan = plan_overlay_fixups(&base_data, &base, &overlay_data, &overlay, budget()).unwrap();
        assert_eq!(plan.base_max_phandle, 100);
        assert_eq!(plan.phandle_delta, 100);
        assert_eq!(plan.external.len(), 1);
        assert_eq!(plan.external[0].target_node_path, "/soc");
        assert_eq!(plan.external[0].target_phandle, 7);
        assert_eq!(plan.external[0].property_offset, 0);
        assert_eq!(plan.local.len(), 1);
        assert_eq!(plan.local[0].original_phandle, 2);
        assert_eq!(plan.local[0].relocated_phandle, 102);
        assert_eq!(plan.declarations.len(), 1);
        assert_eq!(plan.declarations[0].original_phandle, 2);
        assert_eq!(plan.declarations[0].relocated_phandle, 102);
        let writes = build_overlay_fixup_write_set(&overlay_data, &plan, budget()).unwrap();
        let fixed = apply_overlay_fixup_write_set(&overlay_data, &writes, budget()).unwrap();
        let declaration = &plan.declarations[0];
        let declaration_start = declaration.overlay_property_range.start;
        assert_eq!(be32(&fixed, declaration_start).unwrap(), 102);
        let local_start = plan.local[0].overlay_property_range.start;
        assert_eq!(be32(&fixed, local_start).unwrap(), 102);
    }

    #[test]
    fn rejects_external_fixup_without_base_symbol() {
        let (base_data, mut base) = base_fixture();
        base.nodes[1].properties.clear();
        let (overlay_data, overlay) = overlay_fixture();
        assert_eq!(
            plan_overlay_fixups(&base_data, &base, &overlay_data, &overlay, budget()),
            Err(FdtError::MissingSymbol),
        );
    }

    #[test]
    fn rejects_misaligned_or_out_of_range_local_fixup_offsets() {
        let (base_data, base) = base_fixture();
        let (mut overlay_data, overlay) = overlay_fixture();
        overlay_data[overlay.nodes[8].properties[0].value.clone()]
            .copy_from_slice(&1_u32.to_be_bytes());
        assert_eq!(
            plan_overlay_fixups(&base_data, &base, &overlay_data, &overlay, budget()),
            Err(FdtError::InvalidFixup),
        );
        overlay_data[overlay.nodes[8].properties[0].value.clone()]
            .copy_from_slice(&4_u32.to_be_bytes());
        assert_eq!(
            plan_overlay_fixups(&base_data, &base, &overlay_data, &overlay, budget()),
            Err(FdtError::InvalidFixup),
        );
    }
}


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FdtFixupWriteKind {
    External,
    Local,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FdtFixupWrite {
    /// Absolute byte range in the original DTBO byte slice.
    pub range: Range<usize>,
    pub value: u32,
    pub kind: FdtFixupWriteKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FdtOverlayWriteSet {
    pub writes: Vec<FdtFixupWrite>,
    pub output_size: usize,
}

/// Converts independent fixup records into a single sorted write set.
/// Duplicate or overlapping 32-bit writes are rejected before any output is allocated.
pub fn build_overlay_fixup_write_set(
    overlay_data: &[u8],
    plan: &FdtOverlayFixupPlan,
    budget: ParseBudget,
) -> Result<FdtOverlayWriteSet, FdtError> {
    budget.check_input(u64::try_from(overlay_data.len()).map_err(|_| FdtError::IntegerOverflow)?)
        .map_err(|_| FdtError::ResourceLimit)?;
    let total = plan.external.len().checked_add(plan.local.len())
        .and_then(|n| n.checked_add(plan.declarations.len()))
        .ok_or(FdtError::IntegerOverflow)?;
    if total > budget.max_items as usize {
        return Err(FdtError::ResourceLimit);
    }
    let mut writes = Vec::with_capacity(total);
    for entry in &plan.external {
        let relative = usize::try_from(entry.property_offset)
            .map_err(|_| FdtError::IntegerOverflow)?;
        let start = entry.overlay_property_range.start.checked_add(relative)
            .ok_or(FdtError::IntegerOverflow)?;
        let end = start.checked_add(4).ok_or(FdtError::IntegerOverflow)?;
        if end > entry.overlay_property_range.end || end > overlay_data.len() {
            return Err(FdtError::InvalidFixup);
        }
        writes.push(FdtFixupWrite {
            range: start..end,
            value: entry.target_phandle,
            kind: FdtFixupWriteKind::External,
        });
    }
    for entry in &plan.local {
        let relative = usize::try_from(entry.property_offset)
            .map_err(|_| FdtError::IntegerOverflow)?;
        let start = entry.overlay_property_range.start.checked_add(relative)
            .ok_or(FdtError::IntegerOverflow)?;
        let end = start.checked_add(4).ok_or(FdtError::IntegerOverflow)?;
        if end > entry.overlay_property_range.end || end > overlay_data.len() {
            return Err(FdtError::InvalidFixup);
        }
        writes.push(FdtFixupWrite {
            range: start..end,
            value: entry.relocated_phandle,
            kind: FdtFixupWriteKind::Local,
        });
    }
    for entry in &plan.declarations {
        let start = entry.overlay_property_range.start;
        let end = start.checked_add(4).ok_or(FdtError::IntegerOverflow)?;
        if entry.overlay_property_range.len() != 4 || end > overlay_data.len() {
            return Err(FdtError::InvalidFixup);
        }
        writes.push(FdtFixupWrite {
            range: start..end,
            value: entry.relocated_phandle,
            kind: FdtFixupWriteKind::Local,
        });
    }
    writes.sort_unstable_by_key(|write| write.range.start);
    for pair in writes.windows(2) {
        if pair[0].range.end > pair[1].range.start {
            return Err(FdtError::FixupWriteConflict);
        }
    }
    Ok(FdtOverlayWriteSet { writes, output_size: overlay_data.len() })
}

/// Applies a previously validated write set to a fresh bounded copy.
/// The source is never modified; callers receive either a complete output or an error.
pub fn apply_overlay_fixup_write_set(
    overlay_data: &[u8],
    write_set: &FdtOverlayWriteSet,
    budget: ParseBudget,
) -> Result<Vec<u8>, FdtError> {
    budget.check_input(u64::try_from(overlay_data.len()).map_err(|_| FdtError::IntegerOverflow)?)
        .map_err(|_| FdtError::ResourceLimit)?;
    if write_set.output_size != overlay_data.len()
        || write_set.writes.len() > budget.max_items as usize
    {
        return Err(FdtError::InvalidFixup);
    }
    let mut previous_end = 0usize;
    for write in &write_set.writes {
        if write.range.start < previous_end || write.range.end > overlay_data.len()
            || write.range.end.checked_sub(write.range.start) != Some(4)
        {
            return Err(FdtError::FixupWriteConflict);
        }
        previous_end = write.range.end;
    }
    // Allocate only after validating every write, preserving all-or-nothing semantics.
    let mut output = overlay_data.to_vec();
    for write in &write_set.writes {
        output[write.range.clone()].copy_from_slice(&write.value.to_be_bytes());
    }
    Ok(output)
}

#[cfg(test)]
mod fixup_write_tests {
    use super::*;

    fn budget() -> ParseBudget {
        ParseBudget { max_input_bytes: 4096, max_items: 16, max_nesting_depth: 8 }
    }

    fn range(start: usize, len: usize) -> Range<usize> { start..start + len }

    fn plan() -> FdtOverlayFixupPlan {
        FdtOverlayFixupPlan {
            external: vec![FdtExternalFixup {
                symbol: "clk".to_owned(), target_node_path: "/soc".to_owned(),
                target_phandle: 7, overlay_property: "clocks".to_owned(),
                overlay_property_range: range(8, 8), property_offset: 0,
            }],
            local: vec![FdtLocalFixup {
                overlay_node_path: "/fragment@0/dev".to_owned(),
                overlay_property: "link".to_owned(),
                overlay_property_range: range(16, 4), property_offset: 0,
                original_phandle: 2, relocated_phandle: 102,
            }],
            declarations: vec![FdtLocalFixup {
                overlay_node_path: "/fragment@0/dev".to_owned(),
                overlay_property: "phandle".to_owned(),
                overlay_property_range: range(20, 4), property_offset: 0,
                original_phandle: 2, relocated_phandle: 102,
            }],
            base_max_phandle: 100, phandle_delta: 100,
        }
    }

    #[test]
    fn builds_sorted_write_set_and_applies_atomically_to_copy() {
        let mut source = vec![0_u8; 24];
        source[8..12].copy_from_slice(&2_u32.to_be_bytes());
        source[16..20].copy_from_slice(&2_u32.to_be_bytes());
        let writes = build_overlay_fixup_write_set(&source, &plan(), budget()).unwrap();
        assert_eq!(writes.writes.len(), 3);
        let output = apply_overlay_fixup_write_set(&source, &writes, budget()).unwrap();
        assert_eq!(u32::from_be_bytes(output[8..12].try_into().unwrap()), 7);
        assert_eq!(u32::from_be_bytes(output[16..20].try_into().unwrap()), 102);
        assert_eq!(u32::from_be_bytes(output[20..24].try_into().unwrap()), 102);
        assert_eq!(u32::from_be_bytes(source[8..12].try_into().unwrap()), 2);
        assert_eq!(u32::from_be_bytes(source[16..20].try_into().unwrap()), 2);
    }

    #[test]
    fn rejects_duplicate_or_overlapping_write_targets() {
        let source = vec![0_u8; 24];
        let mut conflicting = plan();
        conflicting.local[0].overlay_property_range = range(8, 4);
        assert_eq!(
            build_overlay_fixup_write_set(&source, &conflicting, budget()),
            Err(FdtError::FixupWriteConflict),
        );
    }

    #[test]
    fn rejects_out_of_range_writes_and_budget_excess() {
        let source = vec![0_u8; 24];
        let mut invalid = plan();
        invalid.external[0].property_offset = 8;
        assert_eq!(
            build_overlay_fixup_write_set(&source, &invalid, budget()),
            Err(FdtError::InvalidFixup),
        );
        let mut small = budget();
        small.max_items = 1;
        assert_eq!(
            build_overlay_fixup_write_set(&source, &plan(), small),
            Err(FdtError::ResourceLimit),
        );
    }

    #[test]
    fn rejects_forged_write_set_before_allocating_output() {
        let source = vec![0_u8; 24];
        let forged = FdtOverlayWriteSet {
            writes: vec![FdtFixupWrite {
                range: 20..25, value: 9, kind: FdtFixupWriteKind::External,
            }],
            output_size: 24,
        };
        assert_eq!(
            apply_overlay_fixup_write_set(&source, &forged, budget()),
            Err(FdtError::FixupWriteConflict),
        );
    }
}


#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FdtOwnedProperty {
    pub name: String,
    pub value: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FdtOwnedNode {
    pub name: String,
    pub path: String,
    pub parent: Option<usize>,
    pub properties: Vec<FdtOwnedProperty>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FdtMergedTree {
    pub reservations: Vec<FdtReservation>,
    pub boot_cpuid_phys: u32,
    pub nodes: Vec<FdtOwnedNode>,
    pub root_compatible: Vec<String>,
    pub model: Option<String>,
}

/// One already-indexed and fixup-prepared overlay in an ordered DTBO transaction.
#[derive(Debug, Clone, Copy)]
pub struct FdtOverlayBatchEntry<'a> {
    pub data: &'a [u8],
    pub index: &'a FdtIndex,
    pub resolutions: &'a [FdtOverlayResolution],
}

/// Applies raw DTBO inputs as one atomic, ordered transaction:
/// parse -> plan external/local fixups against the accumulated base -> write to a
/// private copy -> parse fixed DTBO -> resolve fragment targets -> merge -> serialize.
/// A failure returns no partially updated DTB. Each next overlay sees all prior changes.
pub fn apply_dtbo_sequence(
    base_data: &[u8],
    overlays: &[&[u8]],
    budget: ParseBudget,
) -> Result<Vec<u8>, FdtError> {
    if overlays.is_empty() || overlays.len() > budget.max_items as usize {
        return Err(FdtError::ResourceLimit);
    }
    budget.check_input(u64::try_from(base_data.len()).map_err(|_| FdtError::IntegerOverflow)?)
        .map_err(|_| FdtError::ResourceLimit)?;
    let mut current_data = base_data.to_vec();
    let mut cumulative_input = current_data.len();
    for raw_overlay in overlays {
        cumulative_input = cumulative_input.checked_add(raw_overlay.len())
            .ok_or(FdtError::IntegerOverflow)?;
        if cumulative_input > budget.max_input_bytes as usize {
            return Err(FdtError::ResourceLimit);
        }

        let base_index = parse_fdt(&current_data, budget)?;
        let raw_index = parse_fdt(raw_overlay, budget)?;
        let plan = plan_overlay_fixups(&current_data, &base_index, raw_overlay, &raw_index, budget)?;
        let write_set = build_overlay_fixup_write_set(raw_overlay, &plan, budget)?;
        let fixed_overlay = apply_overlay_fixup_write_set(raw_overlay, &write_set, budget)?;
        let fixed_index = parse_fdt(&fixed_overlay, budget)?;
        let overlay_index = index_overlay(&fixed_overlay, &fixed_index, budget.max_items as usize)?;
        let resolutions = resolve_overlay_targets(&current_data, &base_index, &overlay_index)?;
        let merged = merge_overlay_fragments(
            &current_data, &base_index, &fixed_overlay, &fixed_index, &resolutions, budget,
        )?;
        current_data = serialize_fdt_v17(&merged, budget)?;
    }
    // Final parse is part of the transaction contract, not deferred to the caller.
    parse_fdt(&current_data, budget)?;
    Ok(current_data)
}

/// Applies prepared overlays in order. Each successful intermediate tree is
/// serialized and reparsed before the next overlay, so the next resolution set
/// must refer to the accumulated tree. No partial tree is returned on failure.
pub fn merge_overlay_sequence(
    base_data: &[u8],
    base: &FdtIndex,
    overlays: &[FdtOverlayBatchEntry<'_>],
    budget: ParseBudget,
) -> Result<FdtMergedTree, FdtError> {
    if overlays.is_empty() || overlays.len() > budget.max_items as usize {
        return Err(FdtError::ResourceLimit);
    }
    budget.check_input(u64::try_from(base_data.len()).map_err(|_| FdtError::IntegerOverflow)?)
        .map_err(|_| FdtError::ResourceLimit)?;
    let mut current_data = base_data.to_vec();
    let mut current_index = base.clone();
    let mut final_tree = None;

    for entry in overlays {
        let combined_input = current_data.len().checked_add(entry.data.len())
            .ok_or(FdtError::IntegerOverflow)?;
        if combined_input > budget.max_input_bytes as usize {
            return Err(FdtError::ResourceLimit);
        }
        let merged = merge_overlay_fragments(
            &current_data, &current_index, entry.data, entry.index,
            entry.resolutions, budget,
        )?;
        let serialized = serialize_fdt_v17(&merged, budget)?;
        let reparsed = parse_fdt(&serialized, budget)?;
        current_data = serialized;
        current_index = reparsed;
        final_tree = Some(merged);
    }
    final_tree.ok_or(FdtError::InvalidStructure)
}

fn validate_overlay_resolutions(
    base_data: &[u8],
    base: &FdtIndex,
    overlay_data: &[u8],
    overlay: &FdtIndex,
    resolutions: &[FdtOverlayResolution],
    budget: ParseBudget,
) -> Result<(), FdtError> {
    if resolutions.is_empty() || resolutions.len() > budget.max_items as usize {
        return Err(FdtError::ResourceLimit);
    }
    let fragments: Vec<&FdtNode> = overlay.nodes.iter()
        .filter(|node| node.parent == Some(0) && node.name.starts_with("fragment@"))
        .collect();
    if fragments.len() != resolutions.len() {
        return Err(FdtError::MissingOverlayTarget);
    }

    let mut seen = Vec::with_capacity(resolutions.len());
    for resolution in resolutions {
        if seen.iter().any(|path| *path == resolution.fragment_path.as_str()) {
            return Err(FdtError::InvalidStructure);
        }
        seen.push(resolution.fragment_path.as_str());

        let fragment_index = overlay.nodes.iter()
            .position(|node| node.path == resolution.fragment_path)
            .ok_or(FdtError::MissingOverlayTarget)?;
        let fragment = &overlay.nodes[fragment_index];
        if fragment.parent != Some(0) || !fragment.name.starts_with("fragment@")
            || !overlay.nodes.iter().any(|node| {
                node.parent == Some(fragment_index) && node.name == "__overlay__"
            })
        {
            return Err(FdtError::MissingOverlaySubtree);
        }

        let target_index = base.nodes.iter()
            .position(|node| node.path == resolution.target_node_path)
            .ok_or(FdtError::MissingOverlayTarget)?;
        let target = &base.nodes[target_index];
        let mut target_phandle = None;
        for property in &target.properties {
            if property.name == "phandle" || property.name == "linux,phandle" {
                let bytes = base_data.get(property.value.clone()).ok_or(FdtError::InvalidRange)?;
                if bytes.len() != 4 { return Err(FdtError::InvalidPhandle); }
                let value = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                if value == 0 || value == u32::MAX
                    || target_phandle.is_some_and(|previous| previous != value)
                {
                    return Err(FdtError::InvalidPhandle);
                }
                target_phandle = Some(value);
            }
        }
        if target_phandle.unwrap_or(0) != resolution.target_phandle {
            return Err(FdtError::InvalidPhandle);
        }

        let target_path = fragment.properties.iter().find(|p| p.name == "target-path");
        let target_handle = fragment.properties.iter().find(|p| p.name == "target");
        match (target_path, target_handle) {
            (Some(path), None) => {
                let bytes = overlay_data.get(path.value.clone()).ok_or(FdtError::InvalidRange)?;
                let value = c_string(bytes)?;
                if value.len() + 1 != bytes.len() || value != resolution.target_node_path {
                    return Err(FdtError::MissingOverlayTarget);
                }
            }
            (None, Some(handle)) => {
                let bytes = overlay_data.get(handle.value.clone()).ok_or(FdtError::InvalidRange)?;
                if bytes.len() != 4 {
                    return Err(FdtError::InvalidPhandle);
                }
                let value = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                if value == 0 || value == u32::MAX || value != resolution.target_phandle {
                    return Err(FdtError::MissingOverlayTarget);
                }
            }
            _ => return Err(FdtError::MissingOverlayTarget),
        }
    }
    Ok(())
}

/// Combines reservation maps conservatively. Exact duplicates are deduplicated;
/// overlapping ranges are rejected because their ownership cannot be inferred.
fn merge_reservation_maps(
    base: &[FdtReservation],
    overlay: &[FdtReservation],
    budget: ParseBudget,
) -> Result<Vec<FdtReservation>, FdtError> {
    let total = base.len().checked_add(overlay.len()).ok_or(FdtError::IntegerOverflow)?;
    if total > budget.max_items as usize { return Err(FdtError::ResourceLimit); }
    let mut merged = Vec::with_capacity(total);
    merged.extend_from_slice(base);
    merged.extend_from_slice(overlay);
    for entry in &merged {
        if entry.size == 0 || entry.address.checked_add(entry.size).is_none() {
            return Err(FdtError::InvalidReservationMap);
        }
    }
    merged.sort_unstable_by_key(|entry| (entry.address, entry.size));
    merged.dedup_by(|left, right| left.address == right.address && left.size == right.size);
    for pair in merged.windows(2) {
        let previous_end = pair[0].address.checked_add(pair[0].size)
            .ok_or(FdtError::InvalidReservationMap)?;
        if pair[1].address < previous_end {
            return Err(FdtError::InvalidReservationMap);
        }
    }
    Ok(merged)
}

/// Materializes a bounded, owned tree and applies the already-resolved fragment
/// subtrees in fragment order. This is the merge stage only: serialization into a
/// new DTB; exported symbol rewriting is included in the returned owned tree.
pub fn merge_overlay_fragments(
    base_data: &[u8],
    base: &FdtIndex,
    overlay_data: &[u8],
    overlay: &FdtIndex,
    resolutions: &[FdtOverlayResolution],
    budget: ParseBudget,
) -> Result<FdtMergedTree, FdtError> {
    budget.check_input(u64::try_from(base_data.len()).map_err(|_| FdtError::IntegerOverflow)?)
        .map_err(|_| FdtError::ResourceLimit)?;
    budget.check_input(u64::try_from(overlay_data.len()).map_err(|_| FdtError::IntegerOverflow)?)
        .map_err(|_| FdtError::ResourceLimit)?;

    // The index is caller-supplied and public. Re-parse both inputs so stale or
    // mismatched offsets/topology can never be used to copy bytes or route nodes.
    if parse_fdt(base_data, budget)? != *base || parse_fdt(overlay_data, budget)? != *overlay {
        return Err(FdtError::InvalidStructure);
    }
    // The Linux overlay application model does not merge an overlay's
    // reservation map into the live tree. Reject rather than silently discard
    // reservations that may describe memory the guest must keep unavailable.
    validate_overlay_resolutions(base_data, base, overlay_data, overlay, resolutions, budget)?;
    if base.nodes.is_empty() || overlay.nodes.is_empty()
        || base.nodes.len().checked_add(overlay.nodes.len()).ok_or(FdtError::IntegerOverflow)?
            > budget.max_items as usize
    {
        return Err(FdtError::ResourceLimit);
    }

    let mut nodes = Vec::with_capacity(base.nodes.len().min(budget.max_items as usize));
    let mut copied_bytes = 0usize;
    for source in &base.nodes {
        let parent = source.parent;
        let mut properties = Vec::with_capacity(source.properties.len());
        for property in &source.properties {
            let value = base_data.get(property.value.clone()).ok_or(FdtError::InvalidRange)?;
            copied_bytes = copied_bytes.checked_add(value.len()).ok_or(FdtError::IntegerOverflow)?;
            if copied_bytes > budget.max_input_bytes as usize {
                return Err(FdtError::ResourceLimit);
            }
            properties.push(FdtOwnedProperty { name: property.name.clone(), value: value.to_vec() });
        }
        nodes.push(FdtOwnedNode {
            name: source.name.clone(),
            path: source.path.clone(),
            parent,
            properties,
        });
    }

    // Validate the base index topology before using parent indices as array offsets.
    if nodes[0].path != "/" || nodes[0].parent.is_some() {
        return Err(FdtError::InvalidStructure);
    }
    for (index, node) in nodes.iter().enumerate().skip(1) {
        let parent = node.parent.ok_or(FdtError::InvalidStructure)?;
        if parent >= index || parent >= nodes.len() {
            return Err(FdtError::InvalidStructure);
        }
        let expected = if nodes[parent].path == "/" {
            format!("/{}", node.name)
        } else {
            format!("{}/{}", nodes[parent].path, node.name)
        };
        if expected != node.path {
            return Err(FdtError::InvalidStructure);
        }
        if nodes[..index].iter().any(|prior| prior.parent == Some(parent) && prior.name == node.name) {
            return Err(FdtError::InvalidStructure);
        }
    }

    let base_property_count = nodes.iter().try_fold(0usize, |count, node| {
        count.checked_add(node.properties.len()).ok_or(FdtError::IntegerOverflow)
    })?;
    let mut item_count = nodes.len().checked_add(base_property_count)
        .ok_or(FdtError::IntegerOverflow)?;
    if item_count > budget.max_items as usize {
        return Err(FdtError::ResourceLimit);
    }

    // Reject malformed or ambiguous overlay topology before recursive merging.
    let mut overlay_depths = vec![0usize; overlay.nodes.len()];
    for (index, node) in overlay.nodes.iter().enumerate() {
        if index == 0 {
            if node.path != "/" || node.parent.is_some() {
                return Err(FdtError::InvalidStructure);
            }
            continue;
        }
        let parent = node.parent.ok_or(FdtError::InvalidStructure)?;
        if parent >= index || parent >= overlay.nodes.len() {
            return Err(FdtError::InvalidStructure);
        }
        overlay_depths[index] = overlay_depths[parent].checked_add(1)
            .ok_or(FdtError::IntegerOverflow)?;
        if overlay_depths[index] > budget.max_nesting_depth as usize
            || overlay.nodes[..index].iter().any(|prior| {
                prior.parent == Some(parent) && prior.name == node.name
            })
        {
            return Err(FdtError::InvalidStructure);
        }
    }

    for resolution in resolutions {
        let fragment_index = overlay.nodes.iter().position(|node| node.path == resolution.fragment_path)
            .ok_or(FdtError::MissingOverlayTarget)?;
        let target_index = nodes.iter().position(|node| node.path == resolution.target_node_path)
            .ok_or(FdtError::MissingOverlayTarget)?;
        let subtree_index = overlay.nodes.iter().position(|node| {
            node.parent == Some(fragment_index) && node.name == "__overlay__"
        }).ok_or(FdtError::MissingOverlaySubtree)?;

        merge_one_overlay_node(
            base_data,
            overlay_data,
            overlay,
            subtree_index,
            &mut nodes,
            target_index,
            &mut item_count,
            &mut copied_bytes,
            budget,
        )?;
    }

    // A merged tree must not contain duplicate or reserved phandle values.
    let mut phandles = Vec::new();
    for node in &nodes {
        let mut value = None;
        for property in &node.properties {
            if property.name == "phandle" || property.name == "linux,phandle" {
                if property.value.len() != 4 { return Err(FdtError::InvalidPhandle); }
                let current = u32::from_be_bytes([
                    property.value[0], property.value[1], property.value[2], property.value[3],
                ]);
                if current == 0 || current == u32::MAX
                    || value.is_some_and(|previous| previous != current)
                {
                    return Err(FdtError::InvalidPhandle);
                }
                value = Some(current);
            }
        }
        if let Some(value) = value { phandles.push(value); }
    }
    phandles.sort_unstable();
    if phandles.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(FdtError::DuplicatePhandle);
    }

    let root = nodes.first().ok_or(FdtError::InvalidStructure)?;
    let compatible = root.properties.iter().find(|p| p.name == "compatible")
        .map(|p| string_list(&p.value)).transpose()?.unwrap_or_default();
    let model = root.properties.iter().find(|p| p.name == "model")
        .map(|p| {
            let value = c_string(&p.value)?;
            if value.len() + 1 != p.value.len() { return Err(FdtError::InvalidStructure); }
            Ok(value.to_owned())
        }).transpose()?;

    let reservations = merge_reservation_maps(&base.reservations, &overlay.reservations, budget)?;
    let mut merged = FdtMergedTree {
        reservations,
        boot_cpuid_phys: base.header.boot_cpuid_phys,
        nodes,
        root_compatible: compatible,
        model,
    };
    merge_overlay_symbols(
        base, overlay_data, overlay, resolutions, &mut merged, budget,
    )?;
    Ok(merged)
}

/// Merges exported overlay symbols into the base tree's /__symbols__ node.
/// Overlay paths rooted below fragment@N/__overlay__ are rewritten to the
/// corresponding destination path. Ambiguous or conflicting exports fail closed.
fn merge_overlay_symbols(
    base: &FdtIndex,
    overlay_data: &[u8],
    overlay: &FdtIndex,
    resolutions: &[FdtOverlayResolution],
    merged: &mut FdtMergedTree,
    budget: ParseBudget,
) -> Result<(), FdtError> {
    let base_symbols_index = base.nodes.iter().position(|node| node.path == "/__symbols__");
    let overlay_symbols_index = overlay.nodes.iter().position(|node| node.path == "/__symbols__");
    let Some(overlay_symbols_index) = overlay_symbols_index else {
        return Ok(());
    };

    // Validate existing exports before using them as the destination namespace.
    if let Some(index) = base_symbols_index {
        for property in &merged.nodes[index].properties {
            let path = c_string(&property.value)?;
            if path.len() + 1 != property.value.len() || !path.starts_with('/')
                || !merged.nodes.iter().any(|node| node.path == path)
            {
                return Err(FdtError::InvalidSymbol);
            }
        }
    }

    let mut destination_index = if let Some(index) = base_symbols_index {
        index
    } else {
        let index = merged.nodes.len();
        if index.checked_add(1).ok_or(FdtError::IntegerOverflow)? > budget.max_items as usize {
            return Err(FdtError::ResourceLimit);
        }
        merged.nodes.push(FdtOwnedNode {
            name: "__symbols__".to_owned(),
            path: "/__symbols__".to_owned(),
            parent: Some(0),
            properties: Vec::new(),
        });
        index
    };

    let source = &overlay.nodes[overlay_symbols_index];
    for symbol in &source.properties {
        if symbol.name.is_empty() || symbol.name.contains('/') || symbol.name.as_bytes().contains(&0) {
            return Err(FdtError::InvalidSymbol);
        }
        let bytes = overlay_data.get(symbol.value.clone()).ok_or(FdtError::InvalidRange)?;
        let source_path = c_string(bytes)?;
        if source_path.len() + 1 != bytes.len() || !source_path.starts_with('/')
            || !overlay.nodes.iter().any(|node| node.path == source_path)
        {
            return Err(FdtError::InvalidSymbol);
        }

        let mut mapped_path: Option<String> = None;
        for resolution in resolutions {
            let overlay_root = format!("{}/__overlay__", resolution.fragment_path);
            if source_path == overlay_root {
                if mapped_path.replace(resolution.target_node_path.clone()).is_some() {
                    return Err(FdtError::InvalidSymbol);
                }
            } else if let Some(suffix) = source_path.strip_prefix(&format!("{overlay_root}/")) {
                let candidate = if resolution.target_node_path == "/" {
                    format!("/{suffix}")
                } else {
                    format!("{}/{suffix}", resolution.target_node_path)
                };
                if mapped_path.replace(candidate).is_some() {
                    return Err(FdtError::InvalidSymbol);
                }
            }
        }
        let mapped_path = mapped_path.ok_or(FdtError::InvalidSymbol)?;
        if !merged.nodes.iter().any(|node| node.path == mapped_path) {
            return Err(FdtError::InvalidSymbol);
        }
        let mut value = mapped_path.into_bytes();
        value.push(0);

        if let Some(existing) = merged.nodes[destination_index].properties.iter()
            .find(|property| property.name == symbol.name)
        {
            if existing.value != value {
                return Err(FdtError::InvalidSymbol);
            }
            continue;
        }
        let property_count = merged.nodes.iter().try_fold(0usize, |count, node| {
            count.checked_add(node.properties.len()).ok_or(FdtError::IntegerOverflow)
        })?;
        if merged.nodes.len().checked_add(property_count).and_then(|v| v.checked_add(1))
            .ok_or(FdtError::IntegerOverflow)? > budget.max_items as usize
        {
            return Err(FdtError::ResourceLimit);
        }
        merged.nodes[destination_index].properties.push(FdtOwnedProperty {
            name: symbol.name.clone(),
            value,
        });
        destination_index = merged.nodes.iter().position(|node| node.path == "/__symbols__")
            .ok_or(FdtError::InvalidSymbol)?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn merge_one_overlay_node(
    _base_data: &[u8],
    overlay_data: &[u8],
    overlay: &FdtIndex,
    source_index: usize,
    destination: &mut Vec<FdtOwnedNode>,
    destination_index: usize,
    item_count: &mut usize,
    copied_bytes: &mut usize,
    budget: ParseBudget,
) -> Result<(), FdtError> {
    let source = overlay.nodes.get(source_index).ok_or(FdtError::InvalidStructure)?;
    for property in &source.properties {
        let value = overlay_data.get(property.value.clone()).ok_or(FdtError::InvalidRange)?;
        let existing = destination[destination_index].properties.iter()
            .position(|candidate| candidate.name == property.name);
        if let Some(property_index) = existing {
            *copied_bytes = copied_bytes.checked_add(value.len()).ok_or(FdtError::IntegerOverflow)?;
            if *copied_bytes > budget.max_input_bytes as usize { return Err(FdtError::ResourceLimit); }
            destination[destination_index].properties[property_index].value = value.to_vec();
        } else {
            *item_count = item_count.checked_add(1).ok_or(FdtError::IntegerOverflow)?;
            *copied_bytes = copied_bytes.checked_add(value.len()).ok_or(FdtError::IntegerOverflow)?;
            if *item_count > budget.max_items as usize
                || *copied_bytes > budget.max_input_bytes as usize
            {
                return Err(FdtError::ResourceLimit);
            }
            destination[destination_index].properties.push(FdtOwnedProperty {
                name: property.name.clone(),
                value: value.to_vec(),
            });
        }
    }

    for (child_index, child) in overlay.nodes.iter().enumerate()
        .filter(|(_, node)| node.parent == Some(source_index))
    {
        let existing_child = destination.iter().position(|candidate| {
            candidate.parent == Some(destination_index) && candidate.name == child.name
        });
        if let Some(existing_index) = existing_child {
            merge_one_overlay_node(
                _base_data, overlay_data, overlay, child_index, destination,
                existing_index, item_count, copied_bytes, budget,
            )?;
        } else {
            clone_overlay_subtree(
                overlay_data, overlay, child_index, destination, destination_index,
                item_count, copied_bytes, budget,
            )?;
        }
    }
    Ok(())
}

fn clone_overlay_subtree(
    overlay_data: &[u8],
    overlay: &FdtIndex,
    source_index: usize,
    destination: &mut Vec<FdtOwnedNode>,
    parent_index: usize,
    item_count: &mut usize,
    copied_bytes: &mut usize,
    budget: ParseBudget,
) -> Result<usize, FdtError> {
    let source = overlay.nodes.get(source_index).ok_or(FdtError::InvalidStructure)?;
    *item_count = item_count.checked_add(1).ok_or(FdtError::IntegerOverflow)?;
    if *item_count > budget.max_items as usize { return Err(FdtError::ResourceLimit); }
    let path = if destination[parent_index].path == "/" {
        format!("/{}", source.name)
    } else {
        format!("{}/{}", destination[parent_index].path, source.name)
    };
    if destination.iter().any(|node| node.parent == Some(parent_index) && node.name == source.name) {
        return Err(FdtError::InvalidStructure);
    }
    let mut properties = Vec::with_capacity(source.properties.len());
    for property in &source.properties {
        let value = overlay_data.get(property.value.clone()).ok_or(FdtError::InvalidRange)?;
        *item_count = item_count.checked_add(1).ok_or(FdtError::IntegerOverflow)?;
        *copied_bytes = copied_bytes.checked_add(value.len()).ok_or(FdtError::IntegerOverflow)?;
        if *item_count > budget.max_items as usize
            || *copied_bytes > budget.max_input_bytes as usize
        {
            return Err(FdtError::ResourceLimit);
        }
        properties.push(FdtOwnedProperty { name: property.name.clone(), value: value.to_vec() });
    }
    let index = destination.len();
    destination.push(FdtOwnedNode {
        name: source.name.clone(),
        path,
        parent: Some(parent_index),
        properties,
    });
    for (child_index, _) in overlay.nodes.iter().enumerate()
        .filter(|(_, node)| node.parent == Some(source_index))
    {
        clone_overlay_subtree(
            overlay_data, overlay, child_index, destination, index,
            item_count, copied_bytes, budget,
        )?;
    }
    Ok(index)
}


/// Serializes an owned merged tree as a canonical FDT v17 blob and reparses it
/// before returning. The caller receives no output if structural round-trip
/// validation or any output budget check fails.
pub fn serialize_fdt_v17(
    tree: &FdtMergedTree,
    budget: ParseBudget,
) -> Result<Vec<u8>, FdtError> {
    if tree.nodes.is_empty() || tree.nodes.len() > budget.max_items as usize {
        return Err(FdtError::ResourceLimit);
    }
    if tree.nodes[0].name != "" || tree.nodes[0].path != "/" || tree.nodes[0].parent.is_some() {
        return Err(FdtError::InvalidStructure);
    }

    let mut item_count = tree.nodes.len();
    let mut string_names: Vec<String> = Vec::new();
    let mut property_bytes = 0usize;
    for (index, node) in tree.nodes.iter().enumerate() {
        if node.name.as_bytes().contains(&0) || (index > 0 && (node.name.is_empty() || node.name.contains('/'))) {
            return Err(FdtError::InvalidStructure);
        }
        if index > 0 {
            let parent = node.parent.ok_or(FdtError::InvalidStructure)?;
            if parent >= index || parent >= tree.nodes.len() {
                return Err(FdtError::InvalidStructure);
            }
            let expected = if tree.nodes[parent].path == "/" {
                format!("/{}", node.name)
            } else {
                format!("{}/{}", tree.nodes[parent].path, node.name)
            };
            if expected != node.path {
                return Err(FdtError::InvalidStructure);
            }
            if tree.nodes[..index].iter().any(|prior| prior.parent == Some(parent) && prior.name == node.name) {
                return Err(FdtError::InvalidStructure);
            }
        }
        for (property_index, property) in node.properties.iter().enumerate() {
            if property.name.is_empty() || property.name.as_bytes().contains(&0)
                || node.properties[..property_index].iter().any(|prior| prior.name == property.name)
            {
                return Err(FdtError::InvalidStructure);
            }
            item_count = item_count.checked_add(1).ok_or(FdtError::IntegerOverflow)?;
            property_bytes = property_bytes.checked_add(property.value.len()).ok_or(FdtError::IntegerOverflow)?;
            if !string_names.iter().any(|name| name == &property.name) {
                string_names.push(property.name.clone());
            }
        }
    }
    if item_count > budget.max_items as usize
        || property_bytes > budget.max_input_bytes as usize
        || tree.reservations.len().checked_add(item_count).ok_or(FdtError::IntegerOverflow)?
            > budget.max_items as usize
    {
        return Err(FdtError::ResourceLimit);
    }
    for reservation in &tree.reservations {
        if reservation.size == 0 || reservation.address.checked_add(reservation.size).is_none() {
            return Err(FdtError::InvalidReservationMap);
        }
    }

    let mut strings = Vec::new();
    let mut name_offsets = Vec::with_capacity(string_names.len());
    for name in &string_names {
        let offset = u32::try_from(strings.len()).map_err(|_| FdtError::IntegerOverflow)?;
        name_offsets.push((name.clone(), offset));
        strings.extend_from_slice(name.as_bytes());
        strings.push(0);
    }

    let mut structure = Vec::new();
    emit_owned_node(tree, 0, &name_offsets, &mut structure, 0, budget)?;
    structure.extend_from_slice(&FDT_END.to_be_bytes());

    let reservation_bytes = tree.reservations.len().checked_add(1)
        .and_then(|count| count.checked_mul(16)).ok_or(FdtError::IntegerOverflow)?;
    let reserve_offset = FDT_HEADER_SIZE;
    let structure_offset = reserve_offset.checked_add(reservation_bytes)
        .ok_or(FdtError::IntegerOverflow)?;
    let strings_offset = structure_offset.checked_add(structure.len())
        .ok_or(FdtError::IntegerOverflow)?;
    let total_size = strings_offset.checked_add(strings.len())
        .ok_or(FdtError::IntegerOverflow)?;
    if total_size > budget.max_input_bytes as usize {
        return Err(FdtError::ResourceLimit);
    }

    let mut output = vec![0_u8; FDT_HEADER_SIZE];
    output[0..4].copy_from_slice(&FDT_MAGIC.to_be_bytes());
    output[4..8].copy_from_slice(&u32::try_from(total_size).map_err(|_| FdtError::IntegerOverflow)?.to_be_bytes());
    output[8..12].copy_from_slice(&u32::try_from(structure_offset).map_err(|_| FdtError::IntegerOverflow)?.to_be_bytes());
    output[12..16].copy_from_slice(&u32::try_from(strings_offset).map_err(|_| FdtError::IntegerOverflow)?.to_be_bytes());
    output[16..20].copy_from_slice(&u32::try_from(reserve_offset).map_err(|_| FdtError::IntegerOverflow)?.to_be_bytes());
    output[20..24].copy_from_slice(&17_u32.to_be_bytes());
    output[24..28].copy_from_slice(&16_u32.to_be_bytes());
    output[28..32].copy_from_slice(&tree.boot_cpuid_phys.to_be_bytes());
    output[32..36].copy_from_slice(&u32::try_from(strings.len()).map_err(|_| FdtError::IntegerOverflow)?.to_be_bytes());
    output[36..40].copy_from_slice(&u32::try_from(structure.len()).map_err(|_| FdtError::IntegerOverflow)?.to_be_bytes());
    for reservation in &tree.reservations {
        output.extend_from_slice(&reservation.address.to_be_bytes());
        output.extend_from_slice(&reservation.size.to_be_bytes());
    }
    output.extend_from_slice(&[0_u8; 16]);
    output.extend_from_slice(&structure);
    output.extend_from_slice(&strings);

    let reparsed = parse_fdt(&output, budget)?;
    if reparsed.nodes.len() != tree.nodes.len()
        || reparsed.reservations != tree.reservations
        || reparsed.header.boot_cpuid_phys != tree.boot_cpuid_phys
    {
        return Err(FdtError::InvalidStructure);
    }
    for (parsed, owned) in reparsed.nodes.iter().zip(&tree.nodes) {
        if parsed.name != owned.name || parsed.path != owned.path || parsed.parent != owned.parent
            || parsed.properties.len() != owned.properties.len()
        {
            return Err(FdtError::InvalidStructure);
        }
        for (parsed_property, owned_property) in parsed.properties.iter().zip(&owned.properties) {
            if parsed_property.name != owned_property.name
                || output.get(parsed_property.value.clone()) != Some(owned_property.value.as_slice())
            {
                return Err(FdtError::InvalidStructure);
            }
        }
    }
    Ok(output)
}

fn emit_owned_node(
    tree: &FdtMergedTree,
    node_index: usize,
    name_offsets: &[(String, u32)],
    structure: &mut Vec<u8>,
    depth: usize,
    budget: ParseBudget,
) -> Result<(), FdtError> {
    // parse_fdt counts the root as nesting level one, so the deepest valid
    // node has zero-based depth max_nesting_depth - 1.
    if depth >= budget.max_nesting_depth as usize {
        return Err(FdtError::ResourceLimit);
    }
    let node = tree.nodes.get(node_index).ok_or(FdtError::InvalidStructure)?;
    structure.extend_from_slice(&FDT_BEGIN_NODE.to_be_bytes());
    structure.extend_from_slice(node.name.as_bytes());
    structure.push(0);
    while structure.len() % 4 != 0 { structure.push(0); }
    if structure.len() > budget.max_input_bytes as usize {
        return Err(FdtError::ResourceLimit);
    }

    for property in &node.properties {
        let name_offset = name_offsets.iter().find(|(name, _)| name == &property.name)
            .map(|(_, offset)| *offset).ok_or(FdtError::InvalidStringTable)?;
        structure.extend_from_slice(&FDT_PROP.to_be_bytes());
        structure.extend_from_slice(&u32::try_from(property.value.len())
            .map_err(|_| FdtError::IntegerOverflow)?.to_be_bytes());
        structure.extend_from_slice(&name_offset.to_be_bytes());
        structure.extend_from_slice(&property.value);
        while structure.len() % 4 != 0 { structure.push(0); }
        if structure.len() > budget.max_input_bytes as usize {
            return Err(FdtError::ResourceLimit);
        }
    }

    for (child_index, child) in tree.nodes.iter().enumerate()
        .filter(|(_, candidate)| candidate.parent == Some(node_index))
    {
        let _ = child;
        emit_owned_node(tree, child_index, name_offsets, structure, depth + 1, budget)?;
    }
    structure.extend_from_slice(&FDT_END_NODE.to_be_bytes());
    if structure.len() > budget.max_input_bytes as usize {
        return Err(FdtError::ResourceLimit);
    }
    Ok(())
}

#[cfg(test)]
mod fdt_serializer_tests {
    use super::*;

    fn budget() -> ParseBudget {
        ParseBudget { max_input_bytes: 16 * 1024, max_items: 128, max_nesting_depth: 16 }
    }

    fn tree() -> FdtMergedTree {
        FdtMergedTree {
            reservations: vec![FdtReservation { address: 0x8000_0000, size: 0x1000 }],
            boot_cpuid_phys: 3,
            nodes: vec![
                FdtOwnedNode {
                    name: String::new(), path: "/".to_owned(), parent: None,
                    properties: vec![
                        FdtOwnedProperty { name: "compatible".to_owned(), value: b"vendor,board\0".to_vec() },
                        FdtOwnedProperty { name: "#address-cells".to_owned(), value: 2_u32.to_be_bytes().to_vec() },
                    ],
                },
                FdtOwnedNode {
                    name: "soc".to_owned(), path: "/soc".to_owned(), parent: Some(0),
                    properties: vec![
                        FdtOwnedProperty { name: "status".to_owned(), value: b"okay\0".to_vec() },
                    ],
                },
                FdtOwnedNode {
                    name: "uart@1000".to_owned(), path: "/soc/uart@1000".to_owned(), parent: Some(1),
                    properties: vec![
                        FdtOwnedProperty { name: "reg".to_owned(), value: vec![0, 0, 0x10, 0] },
                    ],
                },
            ],
            root_compatible: vec!["vendor,board".to_owned()],
            model: None,
        }
    }

    #[test]
    fn serializes_and_round_trips_tree_and_reservations() {
        let tree = tree();
        let output = serialize_fdt_v17(&tree, budget()).unwrap();
        let parsed = parse_fdt(&output, budget()).unwrap();
        assert_eq!(parsed.nodes.len(), 3);
        assert_eq!(parsed.nodes[2].path, "/soc/uart@1000");
        assert_eq!(parsed.reservations, tree.reservations);
        assert_eq!(parsed.header.boot_cpuid_phys, 3);
        assert_eq!(parsed.root_compatible, vec!["vendor,board"]);
    }

    #[test]
    fn rejects_invalid_topology_and_budget_before_output() {
        let mut invalid = tree();
        invalid.nodes[2].parent = Some(99);
        assert_eq!(serialize_fdt_v17(&invalid, budget()), Err(FdtError::InvalidStructure));
        let mut small = budget();
        small.max_input_bytes = 64;
        assert_eq!(serialize_fdt_v17(&tree(), small), Err(FdtError::ResourceLimit));
    }

    #[test]
    fn rejects_duplicate_property_names() {
        let mut invalid = tree();
        invalid.nodes[0].properties.push(FdtOwnedProperty {
            name: "compatible".to_owned(), value: b"duplicate\0".to_vec(),
        });
        assert_eq!(serialize_fdt_v17(&invalid, budget()), Err(FdtError::InvalidStructure));
    }
}


#[cfg(test)]
mod overlay_merge_pipeline_tests {
    use super::*;

    fn budget() -> ParseBudget {
        ParseBudget { max_input_bytes: 32 * 1024, max_items: 256, max_nesting_depth: 16 }
    }

    fn prop(name: &str, value: &[u8]) -> FdtOwnedProperty {
        FdtOwnedProperty { name: name.to_owned(), value: value.to_vec() }
    }

    fn node(name: &str, path: &str, parent: Option<usize>, properties: Vec<FdtOwnedProperty>) -> FdtOwnedNode {
        FdtOwnedNode { name: name.to_owned(), path: path.to_owned(), parent, properties }
    }

    #[test]
    fn overlay_symbols_are_rewritten_to_merged_paths() {
        let base_tree = FdtMergedTree {
            reservations: vec![], boot_cpuid_phys: 0,
            nodes: vec![
                node("", "/", None, vec![prop("compatible", b"vendor,base\0")]),
                node("soc", "/soc", Some(0), vec![]),
            ],
            root_compatible: vec!["vendor,base".to_owned()], model: None,
        };
        let overlay_tree = FdtMergedTree {
            reservations: vec![], boot_cpuid_phys: 0,
            nodes: vec![
                node("", "/", None, vec![]),
                node("fragment@0", "/fragment@0", Some(0), vec![prop("target-path", b"/soc\0")]),
                node("__overlay__", "/fragment@0/__overlay__", Some(1), vec![]),
                node("uart@1000", "/fragment@0/__overlay__/uart@1000", Some(2), vec![]),
                node("__symbols__", "/__symbols__", Some(0), vec![
                    prop("serial0", b"/fragment@0/__overlay__/uart@1000\0"),
                ]),
            ],
            root_compatible: vec![], model: None,
        };
        let base_data = serialize_fdt_v17(&base_tree, budget()).unwrap();
        let overlay_data = serialize_fdt_v17(&overlay_tree, budget()).unwrap();
        let base_index = parse_fdt(&base_data, budget()).unwrap();
        let overlay_index = parse_fdt(&overlay_data, budget()).unwrap();
        let resolutions = vec![FdtOverlayResolution {
            fragment_path: "/fragment@0".to_owned(),
            target_node_path: "/soc".to_owned(),
            target_phandle: 0,
        }];
        let merged = merge_overlay_fragments(
            &base_data, &base_index, &overlay_data, &overlay_index, &resolutions, budget(),
        ).unwrap();
        let symbols = merged.nodes.iter().find(|node| node.path == "/__symbols__").unwrap();
        assert_eq!(symbols.properties.iter().find(|p| p.name == "serial0").unwrap().value,
            b"/soc/uart@1000\0");
        let output = serialize_fdt_v17(&merged, budget()).unwrap();
        let parsed = parse_fdt(&output, budget()).unwrap();
        assert!(parsed.nodes.iter().any(|node| node.path == "/__symbols__"));
    }

    #[test]
    fn fdt_v16_header_is_parsed_without_v17_structure_size_field() {
        let mut data = serialize_fdt_v17(&FdtMergedTree {
            reservations: vec![FdtReservation { address: 0x8000_0000, size: 0x1000 }],
            boot_cpuid_phys: 4,
            nodes: vec![node("", "/", None, vec![prop("compatible", b"vendor,v16\0")])],
            root_compatible: vec!["vendor,v16".to_owned()], model: None,
        }, budget()).unwrap();
        data[20..24].copy_from_slice(&16_u32.to_be_bytes());
        data[36..40].fill(0);
        let parsed = parse_fdt(&data, budget()).unwrap();
        assert_eq!(parsed.header.version, 16);
        assert_eq!(parsed.header.structure_size as usize, parsed.structure.len());
        assert_eq!(parsed.root_compatible, vec!["vendor,v16"]);
        assert_eq!(parsed.reservations[0].address, 0x8000_0000);
    }

    #[test]
    fn merges_overlay_reservations_and_rejects_overlap() {
        let base_tree = FdtMergedTree {
            reservations: vec![FdtReservation { address: 0x1000, size: 0x100 }],
            boot_cpuid_phys: 0,
            nodes: vec![node("", "/", None, vec![prop("compatible", b"vendor,base\0")])],
            root_compatible: vec!["vendor,base".to_owned()], model: None,
        };
        let overlay_tree = FdtMergedTree {
            reservations: vec![
                FdtReservation { address: 0x2000, size: 0x100 },
                FdtReservation { address: 0x1000, size: 0x100 },
            ],
            boot_cpuid_phys: 0,
            nodes: vec![
                node("", "/", None, vec![]),
                node("fragment@0", "/fragment@0", Some(0), vec![prop("target-path", b"/\0")]),
                node("__overlay__", "/fragment@0/__overlay__", Some(1), vec![]),
            ],
            root_compatible: vec![], model: None,
        };
        let base_data = serialize_fdt_v17(&base_tree, budget()).unwrap();
        let overlay_data = serialize_fdt_v17(&overlay_tree, budget()).unwrap();
        let base_index = parse_fdt(&base_data, budget()).unwrap();
        let overlay_index = parse_fdt(&overlay_data, budget()).unwrap();
        let resolutions = [FdtOverlayResolution {
            fragment_path: "/fragment@0".to_owned(),
            target_node_path: "/".to_owned(), target_phandle: 0,
        }];
        let merged = merge_overlay_fragments(
            &base_data, &base_index, &overlay_data, &overlay_index, &resolutions, budget(),
        ).unwrap();
        assert_eq!(merged.reservations.len(), 2);
        assert_eq!(merged.reservations[0].address, 0x1000);
        assert_eq!(merged.reservations[1].address, 0x2000);

        let mut overlapping = overlay_tree.clone();
        overlapping.reservations = vec![FdtReservation { address: 0x1080, size: 0x100 }];
        let overlap_data = serialize_fdt_v17(&overlapping, budget()).unwrap();
        let overlap_index = parse_fdt(&overlap_data, budget()).unwrap();
        assert_eq!(
            merge_overlay_fragments(
                &base_data, &base_index, &overlap_data, &overlap_index, &resolutions, budget(),
            ),
            Err(FdtError::InvalidReservationMap),
        );
    }

    #[test]
    fn raw_pipeline_relocates_phandle_declarations_and_local_references_together() {
        let base_tree = FdtMergedTree {
            reservations: vec![], boot_cpuid_phys: 0,
            nodes: vec![
                node("", "/", None, vec![prop("compatible", b"vendor,base\0")]),
                node("__symbols__", "/__symbols__", Some(0), vec![prop("soc", b"/soc\0")]),
                node("soc", "/soc", Some(0), vec![prop("phandle", &100_u32.to_be_bytes())]),
            ],
            root_compatible: vec!["vendor,base".to_owned()], model: None,
        };
        let overlay_tree = FdtMergedTree {
            reservations: vec![], boot_cpuid_phys: 0,
            nodes: vec![
                node("", "/", None, vec![]),
                node("fragment@0", "/fragment@0", Some(0), vec![
                    prop("target", &u32::MAX.to_be_bytes()),
                ]),
                node("__overlay__", "/fragment@0/__overlay__", Some(1), vec![]),
                node("dev", "/fragment@0/__overlay__/dev", Some(2), vec![
                    prop("phandle", &1_u32.to_be_bytes()),
                    prop("link", &1_u32.to_be_bytes()),
                ]),
                node("__fixups__", "/__fixups__", Some(0), vec![
                    prop("soc", b"/fragment@0:target:0\0"),
                ]),
                node("__local_fixups__", "/__local_fixups__", Some(0), vec![]),
                node("fragment@0", "/__local_fixups__/fragment@0", Some(5), vec![]),
                node("__overlay__", "/__local_fixups__/fragment@0/__overlay__", Some(6), vec![]),
                node("dev", "/__local_fixups__/fragment@0/__overlay__/dev", Some(7), vec![
                    prop("link", &0_u32.to_be_bytes()),
                ]),
            ],
            root_compatible: vec![], model: None,
        };
        let second_overlay_tree = FdtMergedTree {
            reservations: vec![], boot_cpuid_phys: 0,
            nodes: vec![
                node("", "/", None, vec![]),
                node("fragment@0", "/fragment@0", Some(0), vec![
                    prop("target-path", b"/soc/dev\0"),
                ]),
                node("__overlay__", "/fragment@0/__overlay__", Some(1), vec![]),
                node("child", "/fragment@0/__overlay__/child", Some(2), vec![
                    prop("phandle", &1_u32.to_be_bytes()),
                    prop("link", &1_u32.to_be_bytes()),
                ]),
                node("__local_fixups__", "/__local_fixups__", Some(0), vec![]),
                node("fragment@0", "/__local_fixups__/fragment@0", Some(4), vec![]),
                node("__overlay__", "/__local_fixups__/fragment@0/__overlay__", Some(5), vec![]),
                node("child", "/__local_fixups__/fragment@0/__overlay__/child", Some(6), vec![
                    prop("link", &0_u32.to_be_bytes()),
                ]),
            ],
            root_compatible: vec![], model: None,
        };
        let base = serialize_fdt_v17(&base_tree, budget()).unwrap();
        let overlay = serialize_fdt_v17(&overlay_tree, budget()).unwrap();
        let second_overlay = serialize_fdt_v17(&second_overlay_tree, budget()).unwrap();
        let output = apply_dtbo_sequence(&base, &[&overlay, &second_overlay], budget()).unwrap();
        let parsed = parse_fdt(&output, budget()).unwrap();
        let read = |path: &str, name: &str| {
            let node = parsed.nodes.iter().find(|n| n.path == path).unwrap();
            let property = node.properties.iter().find(|p| p.name == name).unwrap();
            be32(&output, property.value.start).unwrap()
        };
        assert_eq!(read("/soc/dev", "phandle"), 101);
        assert_eq!(read("/soc/dev", "link"), 101);
        assert_eq!(read("/soc/dev/child", "phandle"), 102);
        assert_eq!(read("/soc/dev/child", "link"), 102);
    }

    #[test]
    fn applies_real_dtc_generated_external_local_fixups_and_symbols() {
        if std::env::var_os("ZERODROID_DTC_FIXTURES").is_none() {
            return;
        }
        let fixture_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures-generated");
        let base = std::fs::read(fixture_dir.join("base.dtb"))
            .expect("CI must generate base.dtb with dtc -@");
        let overlay = std::fs::read(fixture_dir.join("overlay.dtbo"))
            .expect("CI must generate overlay.dtbo with dtc -@");
        let output = apply_dtbo_sequence(&base, &[&overlay], budget()).unwrap();
        let parsed = parse_fdt(&output, budget()).unwrap();
        let serial = parsed.nodes.iter()
            .find(|node| node.path == "/soc/serial@1000").unwrap();
        let child = parsed.nodes.iter()
            .find(|node| node.path == "/soc/serial@1000/child@0").unwrap();
        let read = |node: &FdtNode, name: &str| {
            let property = node.properties.iter().find(|p| p.name == name).unwrap();
            be32(&output, property.value.start).unwrap()
        };
        let serial_phandle = read(serial, "phandle");
        let child_phandle = read(child, "phandle");
        assert_ne!(serial_phandle, child_phandle);
        assert_eq!(read(child, "remote"), serial_phandle);
        assert_eq!(read(child, "self"), child_phandle);
        let symbols = parsed.nodes.iter().find(|node| node.path == "/__symbols__").unwrap();
        let child_symbol = symbols.properties.iter().find(|p| p.name == "child0").unwrap();
        assert_eq!(
            c_string(&output[child_symbol.value.clone()]).unwrap(),
            "/soc/serial@1000/child@0",
        );
    }

    #[test]
    fn raw_dtbo_sequence_runs_fixup_resolve_merge_and_serialization_pipeline() {
        let base_tree = FdtMergedTree {
            reservations: vec![], boot_cpuid_phys: 0,
            nodes: vec![
                node("", "/", None, vec![prop("compatible", b"vendor,base\0")]),
                node("soc", "/soc", Some(0), vec![]),
            ],
            root_compatible: vec!["vendor,base".to_owned()], model: None,
        };
        let overlay_tree = FdtMergedTree {
            reservations: vec![], boot_cpuid_phys: 0,
            nodes: vec![
                node("", "/", None, vec![]),
                node("fragment@0", "/fragment@0", Some(0), vec![prop("target-path", b"/soc\0")]),
                node("__overlay__", "/fragment@0/__overlay__", Some(1), vec![]),
                node("uart@1000", "/fragment@0/__overlay__/uart@1000", Some(2), vec![
                    prop("compatible", b"ns16550a\0"),
                ]),
            ],
            root_compatible: vec![], model: None,
        };
        let base = serialize_fdt_v17(&base_tree, budget()).unwrap();
        let overlay = serialize_fdt_v17(&overlay_tree, budget()).unwrap();
        let output = apply_dtbo_sequence(&base, &[&overlay], budget()).unwrap();
        let parsed = parse_fdt(&output, budget()).unwrap();
        assert!(parsed.nodes.iter().any(|n| n.path == "/soc/uart@1000"));
    }

    #[test]
    fn applies_ordered_overlay_sequence_against_accumulated_tree() {
        let base_tree = FdtMergedTree {
            reservations: vec![], boot_cpuid_phys: 0,
            nodes: vec![
                node("", "/", None, vec![prop("compatible", b"vendor,base\0")]),
                node("soc", "/soc", Some(0), vec![]),
            ],
            root_compatible: vec!["vendor,base".to_owned()], model: None,
        };
        let first_tree = FdtMergedTree {
            reservations: vec![], boot_cpuid_phys: 0,
            nodes: vec![
                node("", "/", None, vec![]),
                node("fragment@0", "/fragment@0", Some(0), vec![prop("target-path", b"/soc\0")]),
                node("__overlay__", "/fragment@0/__overlay__", Some(1), vec![]),
                node("uart@1000", "/fragment@0/__overlay__/uart@1000", Some(2), vec![]),
            ],
            root_compatible: vec![], model: None,
        };
        let second_tree = FdtMergedTree {
            reservations: vec![], boot_cpuid_phys: 0,
            nodes: vec![
                node("", "/", None, vec![]),
                node("fragment@0", "/fragment@0", Some(0), vec![prop("target-path", b"/soc/uart@1000\0")]),
                node("__overlay__", "/fragment@0/__overlay__", Some(1), vec![
                    prop("status", b"okay\0"),
                ]),
            ],
            root_compatible: vec![], model: None,
        };
        let base_data = serialize_fdt_v17(&base_tree, budget()).unwrap();
        let first_data = serialize_fdt_v17(&first_tree, budget()).unwrap();
        let second_data = serialize_fdt_v17(&second_tree, budget()).unwrap();
        let base_index = parse_fdt(&base_data, budget()).unwrap();
        let first_index = parse_fdt(&first_data, budget()).unwrap();
        let second_index = parse_fdt(&second_data, budget()).unwrap();
        let first_resolution = [FdtOverlayResolution {
            fragment_path: "/fragment@0".to_owned(),
            target_node_path: "/soc".to_owned(), target_phandle: 0,
        }];
        let second_resolution = [FdtOverlayResolution {
            fragment_path: "/fragment@0".to_owned(),
            target_node_path: "/soc/uart@1000".to_owned(), target_phandle: 0,
        }];
        let entries = [
            FdtOverlayBatchEntry { data: &first_data, index: &first_index, resolutions: &first_resolution },
            FdtOverlayBatchEntry { data: &second_data, index: &second_index, resolutions: &second_resolution },
        ];
        let merged = merge_overlay_sequence(&base_data, &base_index, &entries, budget()).unwrap();
        let uart = merged.nodes.iter().find(|n| n.path == "/soc/uart@1000").unwrap();
        assert_eq!(uart.properties.iter().find(|p| p.name == "status").unwrap().value, b"okay\0");
    }

    #[test]
    fn merges_fragment_then_serializes_and_reparses_result() {
        let base_tree = FdtMergedTree {
            reservations: vec![],
            boot_cpuid_phys: 0,
            nodes: vec![
                node("", "/", None, vec![prop("compatible", b"vendor,base\0")]),
                node("soc", "/soc", Some(0), vec![prop("status", b"disabled\0")]),
            ],
            root_compatible: vec!["vendor,base".to_owned()],
            model: None,
        };
        let overlay_tree = FdtMergedTree {
            reservations: vec![],
            boot_cpuid_phys: 0,
            nodes: vec![
                node("", "/", None, vec![]),
                node("fragment@0", "/fragment@0", Some(0), vec![
                    prop("target-path", b"/soc\0"),
                ]),
                node("__overlay__", "/fragment@0/__overlay__", Some(1), vec![
                    prop("status", b"okay\0"),
                ]),
                node("uart@1000", "/fragment@0/__overlay__/uart@1000", Some(2), vec![
                    prop("compatible", b"ns16550a\0"),
                ]),
            ],
            root_compatible: vec![],
            model: None,
        };
        let base_data = serialize_fdt_v17(&base_tree, budget()).unwrap();
        let overlay_data = serialize_fdt_v17(&overlay_tree, budget()).unwrap();
        let base_index = parse_fdt(&base_data, budget()).unwrap();
        let overlay_index = parse_fdt(&overlay_data, budget()).unwrap();
        let resolutions = vec![FdtOverlayResolution {
            fragment_path: "/fragment@0".to_owned(),
            target_node_path: "/soc".to_owned(),
            target_phandle: 0,
        }];

        let merged = merge_overlay_fragments(
            &base_data, &base_index, &overlay_data, &overlay_index, &resolutions, budget(),
        ).unwrap();
        let forged_resolution = [FdtOverlayResolution {
            fragment_path: "/fragment@0".to_owned(),
            target_node_path: "/".to_owned(),
            target_phandle: 0,
        }];
        assert_eq!(
            merge_overlay_fragments(
                &base_data, &base_index, &overlay_data, &overlay_index,
                &forged_resolution, budget(),
            ),
            Err(FdtError::MissingOverlayTarget),
        );
        assert_eq!(
            merge_overlay_fragments(
                &base_data, &base_index, &overlay_data, &overlay_index, &[], budget(),
            ),
            Err(FdtError::ResourceLimit),
        );
        let mut stale_index = base_index.clone();
        stale_index.nodes[1].path = "/tampered".to_owned();
        assert_eq!(
            merge_overlay_fragments(
                &base_data, &stale_index, &overlay_data, &overlay_index, &resolutions, budget(),
            ),
            Err(FdtError::InvalidStructure),
        );
        let soc = merged.nodes.iter().find(|n| n.path == "/soc").unwrap();
        assert_eq!(soc.properties.iter().find(|p| p.name == "status").unwrap().value, b"okay\0");
        assert!(merged.nodes.iter().any(|n| n.path == "/soc/uart@1000"));

        let output = serialize_fdt_v17(&merged, budget()).unwrap();
        let reparsed = parse_fdt(&output, budget()).unwrap();
        assert!(reparsed.nodes.iter().any(|n| n.path == "/soc/uart@1000"));
    }
}
