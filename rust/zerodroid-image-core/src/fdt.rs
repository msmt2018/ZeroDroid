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
    let reserve_start = usize::try_from(be32(data, 16)?).map_err(|_| FdtError::IntegerOverflow)?;
    if reserve_start < FDT_HEADER_SIZE || reserve_start % 8 != 0 || reserve_start >= total {
        return Err(FdtError::InvalidReservationMap);
    }
    let mut reservations = Vec::new();
    let mut reserve_cursor = reserve_start;
    loop {
        let address = be64(data, reserve_cursor).map_err(|_| FdtError::InvalidReservationMap)?;
        let size_offset = reserve_cursor.checked_add(8).ok_or(FdtError::IntegerOverflow)?;
        let size = be64(data, size_offset).map_err(|_| FdtError::InvalidReservationMap)?;
        reserve_cursor = reserve_cursor.checked_add(16).ok_or(FdtError::IntegerOverflow)?;
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
    while cursor < structure.len() {
        let token = be32(structure, cursor)?;
        cursor = cursor.checked_add(4).ok_or(FdtError::IntegerOverflow)?;
        match token {
            FDT_BEGIN_NODE => {
                if saw_end || nodes.len() >= budget.max_items as usize
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
}
