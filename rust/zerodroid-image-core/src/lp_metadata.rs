use core::ops::Range;

use super::{ParseBudget, ParseError};

const LP_GEOMETRY_MAGIC: u32 = 0x616c_4467;
const LP_GEOMETRY_SIZE: usize = 4096;
const LP_GEOMETRY_STRUCT_SIZE: usize = 52;
const LP_RESERVED_BYTES: u64 = 4096;
const LP_SECTOR_SIZE: u64 = 512;
const LP_METADATA_MAGIC: u32 = 0x414c_5030;
const LP_HEADER_V1_0_SIZE: usize = 128;
const LP_HEADER_V1_2_SIZE: usize = 256;
const LP_PARTITION_ENTRY_SIZE: u32 = 52;
const LP_EXTENT_ENTRY_SIZE: u32 = 24;
const LP_GROUP_ENTRY_SIZE: u32 = 48;
const LP_BLOCK_DEVICE_ENTRY_SIZE: u32 = 64;
const LP_PARTITION_ATTRIBUTE_MASK: u32 = 0x0f;
const LP_GROUP_SLOT_SUFFIXED: u32 = 1;
const LP_BLOCK_DEVICE_SLOT_SUFFIXED: u32 = 1;
const LP_TARGET_LINEAR: u32 = 0;
const LP_TARGET_ZERO: u32 = 1;

fn u16le(data: &[u8], offset: usize) -> Result<u16, ParseError> {
    let end = offset.checked_add(2).ok_or(ParseError::IntegerOverflow)?;
    let b = data.get(offset..end).ok_or(ParseError::OutOfBounds)?;
    Ok(u16::from_le_bytes([b[0], b[1]]))
}
fn u32le(data: &[u8], offset: usize) -> Result<u32, ParseError> {
    let end = offset.checked_add(4).ok_or(ParseError::IntegerOverflow)?;
    let b = data.get(offset..end).ok_or(ParseError::OutOfBounds)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}
fn u64le(data: &[u8], offset: usize) -> Result<u64, ParseError> {
    let end = offset.checked_add(8).ok_or(ParseError::IntegerOverflow)?;
    let b = data.get(offset..end).ok_or(ParseError::OutOfBounds)?;
    Ok(u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
}
fn fixed_name(data: &[u8]) -> Result<String, ParseError> {
    let end = data.iter().position(|b| *b == 0).unwrap_or(data.len());
    if data[end..].iter().any(|b| *b != 0) || end == 0 {
        return Err(ParseError::InvalidBootHeader);
    }
    let name = &data[..end];
    if !name.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'_') {
        return Err(ParseError::InvalidBootHeader);
    }
    Ok(name.iter().map(|b| char::from(*b)).collect())
}

// Small dependency-free SHA-256 implementation. It processes full blocks
// directly and uses a fixed-size tail, avoiding a second input-sized buffer.
fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98,0x71374491,0xb5c0fbcf,0xe9b5dba5,0x3956c25b,0x59f111f1,0x923f82a4,0xab1c5ed5,
        0xd807aa98,0x12835b01,0x243185be,0x550c7dc3,0x72be5d74,0x80deb1fe,0x9bdc06a7,0xc19bf174,
        0xe49b69c1,0xefbe4786,0x0fc19dc6,0x240ca1cc,0x2de92c6f,0x4a7484aa,0x5cb0a9dc,0x76f988da,
        0x983e5152,0xa831c66d,0xb00327c8,0xbf597fc7,0xc6e00bf3,0xd5a79147,0x06ca6351,0x14292967,
        0x27b70a85,0x2e1b2138,0x4d2c6dfc,0x53380d13,0x650a7354,0x766a0abb,0x81c2c92e,0x92722c85,
        0xa2bfe8a1,0xa81a664b,0xc24b8b70,0xc76c51a3,0xd192e819,0xd6990624,0xf40e3585,0x106aa070,
        0x19a4c116,0x1e376c08,0x2748774c,0x34b0bcb5,0x391c0cb3,0x4ed8aa4a,0x5b9cca4f,0x682e6ff3,
        0x748f82ee,0x78a5636f,0x84c87814,0x8cc70208,0x90befffa,0xa4506ceb,0xbef9a3f7,0xc67178f2,
    ];
    fn compress(state: &mut [u32; 8], block: &[u8]) {
        let mut w = [0_u32; 64];
        for i in 0..16 {
            let p = i * 4;
            w[i] = u32::from_be_bytes([block[p], block[p+1], block[p+2], block[p+3]]);
        }
        for i in 16..64 {
            let s0 = w[i-15].rotate_right(7) ^ w[i-15].rotate_right(18) ^ (w[i-15] >> 3);
            let s1 = w[i-2].rotate_right(17) ^ w[i-2].rotate_right(19) ^ (w[i-2] >> 10);
            w[i] = w[i-16].wrapping_add(s0).wrapping_add(w[i-7]).wrapping_add(s1);
        }
        let [mut a,mut b,mut c,mut d,mut e,mut f,mut g,mut h] = *state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = h.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            h=g; g=f; f=e; e=d.wrapping_add(t1); d=c; c=b; b=a; a=t1.wrapping_add(t2);
        }
        state[0]=state[0].wrapping_add(a); state[1]=state[1].wrapping_add(b);
        state[2]=state[2].wrapping_add(c); state[3]=state[3].wrapping_add(d);
        state[4]=state[4].wrapping_add(e); state[5]=state[5].wrapping_add(f);
        state[6]=state[6].wrapping_add(g); state[7]=state[7].wrapping_add(h);
    }
    let mut state = [0x6a09e667,0xbb67ae85,0x3c6ef372,0xa54ff53a,0x510e527f,0x9b05688c,0x1f83d9ab,0x5be0cd19];
    let full = data.len() / 64;
    for i in 0..full { compress(&mut state, &data[i*64..i*64+64]); }
    let tail = &data[full*64..];
    let mut final_blocks = [0_u8; 128];
    final_blocks[..tail.len()].copy_from_slice(tail);
    final_blocks[tail.len()] = 0x80;
    let final_len = if tail.len() < 56 { 64 } else { 128 };
    let bit_len = (data.len() as u64).wrapping_mul(8).to_be_bytes();
    final_blocks[final_len-8..final_len].copy_from_slice(&bit_len);
    compress(&mut state, &final_blocks[..64]);
    if final_len == 128 { compress(&mut state, &final_blocks[64..]); }
    let mut out = [0_u8; 32];
    for (i, word) in state.iter().enumerate() { out[i*4..i*4+4].copy_from_slice(&word.to_be_bytes()); }
    out
}
fn sha256_zeroed(data: &[u8], zero: Range<usize>) -> Result<[u8; 32], ParseError> {
    if zero.start > zero.end || zero.end > data.len() { return Err(ParseError::OutOfBounds); }
    let mut copy = data.to_vec();
    copy[zero].fill(0);
    Ok(sha256(&copy))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LpGeometry {
    pub metadata_max_size: u32,
    pub metadata_slot_count: u32,
    pub logical_block_size: u32,
    pub source_range: Range<usize>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LpTableDescriptor { pub offset: u32, pub num_entries: u32, pub entry_size: u32 }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LpPartition {
    pub name: String, pub attributes: u32, pub first_extent_index: u32,
    pub num_extents: u32, pub group_index: u32, pub logical_sectors: u64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LpExtent {
    pub num_sectors: u64, pub target_type: u32, pub target_data: u64,
    pub target_source: u32, pub physical_range_sectors: Option<Range<u64>>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LpGroup { pub name: String, pub flags: u32, pub maximum_size: u64 }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LpBlockDevice {
    pub first_logical_sector: u64, pub alignment: u32, pub alignment_offset: u32,
    pub size: u64, pub partition_name: String, pub flags: u32,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LpMetadataIndex {
    pub geometry: LpGeometry, pub slot_number: u32, pub major_version: u16,
    pub minor_version: u16, pub header_flags: u32,
    pub header_range: Range<usize>, pub tables_range: Range<usize>,
    pub partitions: Vec<LpPartition>, pub extents: Vec<LpExtent>,
    pub groups: Vec<LpGroup>, pub block_devices: Vec<LpBlockDevice>,
    pub metadata_copy_range: Range<usize>,
}

fn parse_geometry(data: &[u8], offset: usize) -> Result<LpGeometry, ParseError> {
    let end = offset.checked_add(LP_GEOMETRY_SIZE).ok_or(ParseError::IntegerOverflow)?;
    let area = data.get(offset..end).ok_or(ParseError::OutOfBounds)?;
    if u32le(area, 0)? != LP_GEOMETRY_MAGIC { return Err(ParseError::InvalidMagic); }
    let struct_size = u32le(area, 4)? as usize;
    if struct_size != LP_GEOMETRY_STRUCT_SIZE { return Err(ParseError::InvalidHeaderSize); }
    let geometry_bytes = &area[..struct_size];
    if sha256_zeroed(geometry_bytes, 8..40)? != geometry_bytes[8..40] { return Err(ParseError::ChecksumMismatch); }
    let metadata_max_size = u32le(area, 40)?;
    let metadata_slot_count = u32le(area, 44)?;
    let logical_block_size = u32le(area, 48)?;
    if metadata_max_size == 0 || metadata_max_size as u64 % LP_SECTOR_SIZE != 0
        || metadata_slot_count == 0 || metadata_slot_count > 8
        || logical_block_size < 512 || logical_block_size % 512 != 0
        || !logical_block_size.is_power_of_two()
    { return Err(ParseError::InvalidBootHeader); }
    Ok(LpGeometry { metadata_max_size, metadata_slot_count, logical_block_size, source_range: offset..offset+struct_size })
}

fn parse_descriptor(data: &[u8], offset: usize) -> Result<LpTableDescriptor, ParseError> {
    Ok(LpTableDescriptor { offset:u32le(data,offset)?, num_entries:u32le(data,offset+4)?, entry_size:u32le(data,offset+8)? })
}
fn table_range(desc: LpTableDescriptor, tables_size: u32, min_entry_size: u32) -> Result<Range<usize>, ParseError> {
    if desc.entry_size < min_entry_size || (desc.num_entries > 0 && desc.entry_size == 0) {
        return Err(ParseError::InvalidHeaderSize);
    }
    let size = u64::from(desc.num_entries).checked_mul(u64::from(desc.entry_size)).ok_or(ParseError::IntegerOverflow)?;
    let end = u64::from(desc.offset).checked_add(size).ok_or(ParseError::IntegerOverflow)?;
    if end > u64::from(tables_size) { return Err(ParseError::OutOfBounds); }
    Ok(desc.offset as usize..usize::try_from(end).map_err(|_|ParseError::IntegerOverflow)?)
}

/// Parses Android liblp metadata from a complete super image for the requested metadata slot.
/// Primary metadata is preferred; the matching backup copy is attempted if primary validation fails.
pub fn parse_lp_metadata(data: &[u8], slot_number: u32, budget: ParseBudget) -> Result<LpMetadataIndex, ParseError> {
    let input_size = u64::try_from(data.len()).map_err(|_|ParseError::IntegerOverflow)?;
    budget.check_input(input_size)?;
    // AOSP layout: 4 KiB reserved prefix, then two 4 KiB geometry blocks.
    let geometry = match parse_geometry(data, LP_RESERVED_BYTES as usize) {
        Ok(value) => value,
        Err(primary_error) => parse_geometry(data, LP_RESERVED_BYTES as usize + LP_GEOMETRY_SIZE)
            .map_err(|_| primary_error)?,
    };
    if slot_number >= geometry.metadata_slot_count { return Err(ParseError::OutOfBounds); }
    let slots = u64::from(geometry.metadata_slot_count);
    let max_size = u64::from(geometry.metadata_max_size);
    let primary_offset = LP_RESERVED_BYTES
        .checked_add((LP_GEOMETRY_SIZE as u64).checked_mul(2).ok_or(ParseError::IntegerOverflow)?)
        .and_then(|v| v.checked_add(max_size.checked_mul(u64::from(slot_number))?))
        .ok_or(ParseError::IntegerOverflow)?;
    // AOSP liblp stores all backup metadata slots after the primary slot array.
    let backup_offset = primary_offset
        .checked_add(max_size.checked_mul(slots).ok_or(ParseError::IntegerOverflow)?)
        .ok_or(ParseError::IntegerOverflow)?;
    let primary = usize::try_from(primary_offset).map_err(|_|ParseError::IntegerOverflow)?;
    let backup = usize::try_from(backup_offset).map_err(|_|ParseError::IntegerOverflow)?;
    let parse = |offset: usize| parse_metadata_copy(data, offset, geometry.clone(), slot_number, budget);
    match parse(primary) {
        Ok(index) => Ok(index),
        Err(primary_error) => parse(backup).map_err(|_| primary_error),
    }
}

fn parse_metadata_copy(
    data: &[u8], offset: usize, geometry: LpGeometry, slot_number: u32, budget: ParseBudget,
) -> Result<LpMetadataIndex, ParseError> {
    let header_min_end = offset.checked_add(LP_HEADER_V1_0_SIZE).ok_or(ParseError::IntegerOverflow)?;
    let base = data.get(offset..header_min_end).ok_or(ParseError::OutOfBounds)?;
    if u32le(base,0)? != LP_METADATA_MAGIC { return Err(ParseError::InvalidMagic); }
    let major = u16le(base,4)?;
    let minor = u16le(base,6)?;
    if major != 10 || minor > 2 { return Err(ParseError::UnsupportedVersion); }
    let header_size = u32le(base,8)? as usize;
    let expected_header_size = if minor >= 2 { LP_HEADER_V1_2_SIZE } else { LP_HEADER_V1_0_SIZE };
    if header_size != expected_header_size { return Err(ParseError::InvalidHeaderSize); }
    let header_end = offset.checked_add(header_size).ok_or(ParseError::IntegerOverflow)?;
    let header = data.get(offset..header_end).ok_or(ParseError::OutOfBounds)?;
    if sha256_zeroed(header,12..44)? != header[12..44] { return Err(ParseError::ChecksumMismatch); }
    let tables_size = u32le(header,44)?;
    let tables_size_u64 = u64::from(tables_size);
    if tables_size_u64 > u64::from(budget.max_input_bytes) { return Err(ParseError::ResourceLimit); }
    let tables_start = header_end;
    if u64::from(header_size as u32).checked_add(tables_size_u64).ok_or(ParseError::IntegerOverflow)?
        > u64::from(geometry.metadata_max_size) {
        return Err(ParseError::ResourceLimit);
    }
    let tables_end = tables_start.checked_add(tables_size as usize).ok_or(ParseError::IntegerOverflow)?;
    let tables = data.get(tables_start..tables_end).ok_or(ParseError::OutOfBounds)?;
    if sha256(tables) != header[48..80] { return Err(ParseError::ChecksumMismatch); }

    let partitions_desc = parse_descriptor(header,80)?;
    let extents_desc = parse_descriptor(header,92)?;
    let groups_desc = parse_descriptor(header,104)?;
    let devices_desc = parse_descriptor(header,116)?;
    if partitions_desc.entry_size != LP_PARTITION_ENTRY_SIZE
        || extents_desc.entry_size != LP_EXTENT_ENTRY_SIZE
        || groups_desc.entry_size != LP_GROUP_ENTRY_SIZE
        || devices_desc.entry_size != LP_BLOCK_DEVICE_ENTRY_SIZE {
        return Err(ParseError::InvalidHeaderSize);
    }
    let partitions_range = table_range(partitions_desc,tables_size,LP_PARTITION_ENTRY_SIZE)?;
    let extents_range = table_range(extents_desc,tables_size,LP_EXTENT_ENTRY_SIZE)?;
    let groups_range = table_range(groups_desc,tables_size,LP_GROUP_ENTRY_SIZE)?;
    let devices_range = table_range(devices_desc,tables_size,LP_BLOCK_DEVICE_ENTRY_SIZE)?;
    let mut ranges = vec![partitions_range.clone(),extents_range.clone(),groups_range.clone(),devices_range.clone()];
    ranges.sort_unstable_by_key(|r|r.start);
    for pair in ranges.windows(2) {
        if pair[0].end > pair[1].start { return Err(ParseError::InvalidBootHeader); }
    }
    if ranges.first().is_some_and(|r| r.start != 0)
        || ranges.last().is_some_and(|r| r.end != tables_size as usize) {
        return Err(ParseError::InvalidBootHeader);
    }
    let total_items = partitions_desc.num_entries.checked_add(extents_desc.num_entries)
        .and_then(|v|v.checked_add(groups_desc.num_entries)).and_then(|v|v.checked_add(devices_desc.num_entries))
        .ok_or(ParseError::IntegerOverflow)?;
    budget.check_items(total_items)?;
    if devices_desc.num_entries == 0 { return Err(ParseError::InvalidBootHeader); }

    if groups_desc.num_entries == 0 { return Err(ParseError::InvalidBootHeader); }
    let mut group_names = std::collections::HashSet::new();
    let mut groups = Vec::with_capacity(groups_desc.num_entries as usize);
    for i in 0..groups_desc.num_entries as usize {
        let p = groups_range.start + i * groups_desc.entry_size as usize;
        let row = &tables[p..p+groups_desc.entry_size as usize];
        let name = fixed_name(&row[..36])?;
        let flags = u32le(row,36)?;
        let maximum_size = u64le(row,40)?;
        if flags & !LP_GROUP_SLOT_SUFFIXED != 0 { return Err(ParseError::InvalidBootHeader); }
        if !group_names.insert(name.clone()) { return Err(ParseError::InvalidBootHeader); }
        groups.push(LpGroup{name,flags,maximum_size});
    }
    let mut device_names = std::collections::HashSet::new();
    let mut block_devices = Vec::with_capacity(devices_desc.num_entries as usize);
    for i in 0..devices_desc.num_entries as usize {
        let p = devices_range.start + i * devices_desc.entry_size as usize;
        let row = &tables[p..p+devices_desc.entry_size as usize];
        let device = LpBlockDevice {
            first_logical_sector:u64le(row,0)?, alignment:u32le(row,8)?, alignment_offset:u32le(row,12)?,
            size:u64le(row,16)?, partition_name:fixed_name(&row[24..60])?, flags:u32le(row,60)?,
        };
        if device.size == 0 || device.size % LP_SECTOR_SIZE != 0
            || (device.alignment != 0 && device.alignment_offset >= device.alignment)
            || device.flags & !LP_BLOCK_DEVICE_SLOT_SUFFIXED != 0
            || device.first_logical_sector.checked_mul(LP_SECTOR_SIZE).ok_or(ParseError::IntegerOverflow)? >= device.size
        { return Err(ParseError::InvalidBootHeader); }
        if !device_names.insert(device.partition_name.clone()) { return Err(ParseError::InvalidBootHeader); }
        block_devices.push(device);
    }

    let mut extents = Vec::with_capacity(extents_desc.num_entries as usize);
    for i in 0..extents_desc.num_entries as usize {
        let p = extents_range.start + i * extents_desc.entry_size as usize;
        let row = &tables[p..p+extents_desc.entry_size as usize];
        let num_sectors = u64le(row,0)?;
        let target_type = u32le(row,8)?;
        let target_data = u64le(row,12)?;
        let target_source = u32le(row,20)?;
        if num_sectors == 0 { return Err(ParseError::InvalidBootHeader); }
        let physical_range_sectors = match target_type {
            LP_TARGET_LINEAR => {
                let device = block_devices.get(target_source as usize).ok_or(ParseError::OutOfBounds)?;
                let end = target_data.checked_add(num_sectors).ok_or(ParseError::IntegerOverflow)?;
                let device_sectors = device.size / LP_SECTOR_SIZE;
                if target_data < device.first_logical_sector || end > device_sectors { return Err(ParseError::OutOfBounds); }
                Some(target_data..end)
            }
            LP_TARGET_ZERO => {
                if target_data != 0 || target_source != 0 { return Err(ParseError::InvalidBootHeader); }
                None
            }
            _ => return Err(ParseError::InvalidBootHeader),
        };
        extents.push(LpExtent{num_sectors,target_type,target_data,target_source,physical_range_sectors});
    }

    let mut partition_names = std::collections::HashSet::new();
    let mut partitions = Vec::with_capacity(partitions_desc.num_entries as usize);
    let mut claimed_extent_indices = vec![false; extents.len()];
    let mut group_totals = vec![0_u64; groups.len()];
    for i in 0..partitions_desc.num_entries as usize {
        let p = partitions_range.start + i * partitions_desc.entry_size as usize;
        let row = &tables[p..p+partitions_desc.entry_size as usize];
        let name = fixed_name(&row[..36])?;
        let attributes = u32le(row,36)?;
        let first_extent_index = u32le(row,40)?;
        let num_extents = u32le(row,44)?;
        let group_index = u32le(row,48)?;
        let end_index = first_extent_index.checked_add(num_extents).ok_or(ParseError::IntegerOverflow)?;
        if attributes & !LP_PARTITION_ATTRIBUTE_MASK != 0 || num_extents == 0
            || end_index > extents_desc.num_entries || group_index as usize >= groups.len()
        { return Err(ParseError::OutOfBounds); }
        if !partition_names.insert(name.clone()) { return Err(ParseError::InvalidBootHeader); }
        for index in first_extent_index..end_index {
            let claimed = claimed_extent_indices.get_mut(index as usize).ok_or(ParseError::OutOfBounds)?;
            if *claimed { return Err(ParseError::InvalidBootHeader); }
            *claimed = true;
        }
        let mut logical_sectors = 0_u64;
        for extent in &extents[first_extent_index as usize..end_index as usize] {
            logical_sectors = logical_sectors.checked_add(extent.num_sectors).ok_or(ParseError::IntegerOverflow)?;
        }
        let logical_bytes = logical_sectors.checked_mul(LP_SECTOR_SIZE).ok_or(ParseError::IntegerOverflow)?;
        group_totals[group_index as usize] = group_totals[group_index as usize].checked_add(logical_bytes).ok_or(ParseError::IntegerOverflow)?;
        partitions.push(LpPartition{name,attributes,first_extent_index,num_extents,group_index,logical_sectors});
    }
    if claimed_extent_indices.iter().any(|claimed| !claimed) { return Err(ParseError::InvalidBootHeader); }
    for (index, group) in groups.iter().enumerate() {
        if group.maximum_size != 0 && group_totals[index] > group.maximum_size { return Err(ParseError::ResourceLimit); }
    }
    let metadata_region_end = LP_RESERVED_BYTES
        .checked_add((LP_GEOMETRY_SIZE as u64).checked_mul(2).ok_or(ParseError::IntegerOverflow)?)
        .and_then(|v| v.checked_add(u64::from(geometry.metadata_max_size)
            .checked_mul(u64::from(geometry.metadata_slot_count))?.checked_mul(2)?))
        .ok_or(ParseError::IntegerOverflow)?;
    let first_device = block_devices.first().ok_or(ParseError::InvalidBootHeader)?;
    let first_logical_byte = first_device.first_logical_sector.checked_mul(LP_SECTOR_SIZE).ok_or(ParseError::IntegerOverflow)?;
    if metadata_region_end > first_logical_byte { return Err(ParseError::OutOfBounds); }

    // Physical extents may not alias the same sectors on a block device.
    let mut physical = Vec::new();
    for (extent_index, extent) in extents.iter().enumerate() {
        if let Some(range) = &extent.physical_range_sectors {
            physical.push((extent.target_source,range.start,range.end,extent_index));
        }
    }
    physical.sort_unstable_by_key(|entry|(entry.0,entry.1));
    if physical.windows(2).any(|pair|pair[0].0==pair[1].0 && pair[0].2>pair[1].1) {
        return Err(ParseError::InvalidBootHeader);
    }

    let header_range = offset..header_end;
    let tables_range = tables_start..tables_end;
    let metadata_copy_end = offset.checked_add(geometry.metadata_max_size as usize).ok_or(ParseError::IntegerOverflow)?;
    if metadata_copy_end > data.len() { return Err(ParseError::OutOfBounds); }
    Ok(LpMetadataIndex{
        geometry,slot_number,major_version:major,minor_version:minor,
        header_flags:if minor>=2 {u32le(header,128)?} else {0},
        header_range,tables_range,partitions,extents,groups,block_devices,metadata_copy_range:offset..metadata_copy_end,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put_u16(data:&mut [u8], offset:usize, value:u16){data[offset..offset+2].copy_from_slice(&value.to_le_bytes());}
    fn put_u32(data:&mut [u8], offset:usize, value:u32){data[offset..offset+4].copy_from_slice(&value.to_le_bytes());}
    fn put_u64(data:&mut [u8], offset:usize, value:u64){data[offset..offset+8].copy_from_slice(&value.to_le_bytes());}
    fn put_name(data:&mut [u8], offset:usize, value:&str){data[offset..offset+value.len()].copy_from_slice(value.as_bytes());}
    fn budget()->ParseBudget{ParseBudget{max_input_bytes:1024*1024,max_items:128,max_nesting_depth:8}}

    fn valid_super() -> Vec<u8> {
        let mut data=vec![0_u8;128*1024];
        let geometry_offset=4096;
        put_u32(&mut data,geometry_offset,LP_GEOMETRY_MAGIC);
        put_u32(&mut data,geometry_offset+4,LP_GEOMETRY_STRUCT_SIZE as u32);
        put_u32(&mut data,geometry_offset+40,4096);
        put_u32(&mut data,geometry_offset+44,1);
        put_u32(&mut data,geometry_offset+48,4096);
        let geometry_hash=sha256_zeroed(&data[geometry_offset..geometry_offset+LP_GEOMETRY_STRUCT_SIZE],8..40).unwrap();
        data[geometry_offset+8..geometry_offset+40].copy_from_slice(&geometry_hash);

        let metadata_offset=12288;
        let tables_offset=metadata_offset+LP_HEADER_V1_0_SIZE;
        // Partition, extent, group and block-device tables.
        put_name(&mut data,tables_offset,"system");
        put_u32(&mut data,tables_offset+36,0);
        put_u32(&mut data,tables_offset+40,0);
        put_u32(&mut data,tables_offset+44,1);
        put_u32(&mut data,tables_offset+48,0);
        let extent_offset=tables_offset+52;
        put_u64(&mut data,extent_offset,128);
        put_u32(&mut data,extent_offset+8,LP_TARGET_LINEAR);
        put_u64(&mut data,extent_offset+12,64);
        put_u32(&mut data,extent_offset+20,0);
        let group_offset=extent_offset+24;
        put_name(&mut data,group_offset,"default");
        put_u64(&mut data,group_offset+40,0);
        let device_offset=group_offset+48;
        put_u64(&mut data,device_offset,40);
        put_u32(&mut data,device_offset+8,4096);
        put_u64(&mut data,device_offset+16,128*1024);
        put_name(&mut data,device_offset+24,"super");
        let tables_size=52+24+48+64;
        put_u32(&mut data,metadata_offset,LP_METADATA_MAGIC);
        put_u16(&mut data,metadata_offset+4,10); put_u16(&mut data,metadata_offset+6,0);
        put_u32(&mut data,metadata_offset+8,LP_HEADER_V1_0_SIZE as u32);
        put_u32(&mut data,metadata_offset+44,tables_size as u32);
        let tables_hash=sha256(&data[tables_offset..tables_offset+tables_size]);
        data[metadata_offset+48..metadata_offset+80].copy_from_slice(&tables_hash);
        put_u32(&mut data,metadata_offset+80,0);put_u32(&mut data,metadata_offset+84,1);put_u32(&mut data,metadata_offset+88,52);
        put_u32(&mut data,metadata_offset+92,52);put_u32(&mut data,metadata_offset+96,1);put_u32(&mut data,metadata_offset+100,24);
        put_u32(&mut data,metadata_offset+104,76);put_u32(&mut data,metadata_offset+108,1);put_u32(&mut data,metadata_offset+112,48);
        put_u32(&mut data,metadata_offset+116,124);put_u32(&mut data,metadata_offset+120,1);put_u32(&mut data,metadata_offset+124,64);
        let header_hash=sha256_zeroed(&data[metadata_offset..metadata_offset+LP_HEADER_V1_0_SIZE],12..44).unwrap();
        data[metadata_offset+12..metadata_offset+44].copy_from_slice(&header_hash);
        data
    }

    #[test]
    fn sha256_matches_standard_vectors(){
        assert_eq!(sha256(b"abc"),[0xba,0x78,0x16,0xbf,0x8f,0x01,0xcf,0xea,0x41,0x41,0x40,0xde,0x5d,0xae,0x22,0x23,0xb0,0x03,0x61,0xa3,0x96,0x17,0x7a,0x9c,0xb4,0x10,0xff,0x61,0xf2,0x00,0x15,0xad]);
    }

    #[test]
    fn parses_geometry_header_tables_and_linear_mapping(){
        let data=valid_super();
        let parsed=parse_lp_metadata(&data,0,budget()).unwrap();
        assert_eq!(parsed.partitions[0].name,"system");
        assert_eq!(parsed.partitions[0].logical_sectors,128);
        assert_eq!(parsed.extents[0].physical_range_sectors,Some(64..192));
        assert_eq!(parsed.block_devices[0].partition_name,"super");
    }

    #[test]
    fn falls_back_to_backup_geometry_and_metadata_copy(){
        let mut data=valid_super();
        let geometry=data[4096..4096+LP_GEOMETRY_STRUCT_SIZE].to_vec();
        data[8192..8192+LP_GEOMETRY_STRUCT_SIZE].copy_from_slice(&geometry);
        data[4096+8]^=1;
        let backup_metadata_offset=12288+4096;
        let primary_metadata=data[12288..12288+4096].to_vec();
        data[backup_metadata_offset..backup_metadata_offset+4096].copy_from_slice(&primary_metadata);
        data[12288+12]^=1;
        let parsed=parse_lp_metadata(&data,0,budget()).unwrap();
        assert_eq!(parsed.partitions[0].name,"system");
        assert_eq!(parsed.metadata_copy_range.start,backup_metadata_offset);
    }

    #[test]
    fn rejects_bad_geometry_and_header_checksums(){
        let mut data=valid_super(); data[4096+8]^=1;
        assert_eq!(parse_lp_metadata(&data,0,budget()),Err(ParseError::ChecksumMismatch));
        let mut data=valid_super(); data[12288+12]^=1;
        assert_eq!(parse_lp_metadata(&data,0,budget()),Err(ParseError::ChecksumMismatch));
    }

    #[test]
    fn rejects_table_checksum_and_out_of_bounds_extent(){
        let mut data=valid_super(); data[12288+128+3]^=1;
        assert_eq!(parse_lp_metadata(&data,0,budget()),Err(ParseError::ChecksumMismatch));
        let mut data=valid_super(); put_u64(&mut data,12288+128+52+12,65000);
        let table_hash=sha256(&data[12288+128..12288+128+188]);
        data[12288+48..12288+80].copy_from_slice(&table_hash);
        let header_hash=sha256_zeroed(&data[12288..12288+128],12..44).unwrap();
        data[12288+12..12288+44].copy_from_slice(&header_hash);
        assert_eq!(parse_lp_metadata(&data,0,budget()),Err(ParseError::OutOfBounds));
    }

    #[test]
    fn rejects_partition_group_and_physical_extent_overlaps(){
        let mut data=valid_super();
        // Group maximum is smaller than the partition's 128 * 512 bytes.
        put_u64(&mut data,12288+128+52+24+40,512);
        let tables_hash=sha256(&data[12288+128..12288+128+188]);
        data[12288+48..12288+80].copy_from_slice(&tables_hash);
        let header_hash=sha256_zeroed(&data[12288..12288+128],12..44).unwrap();
        data[12288+12..12288+44].copy_from_slice(&header_hash);
        assert_eq!(parse_lp_metadata(&data,0,budget()),Err(ParseError::ResourceLimit));
    }
}
