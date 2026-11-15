use core::ops::Range;

use super::{ParseBudget, ParseError};

const GPT_SIGNATURE: &[u8; 8] = b"EFI PART";
const GPT_MIN_HEADER_SIZE: usize = 92;
const GPT_MIN_ENTRY_SIZE: u32 = 128;
const GPT_ENTRY_TYPE_GUID_OFFSET: usize = 0;
const GPT_ENTRY_UNIQUE_GUID_OFFSET: usize = 16;
const GPT_ENTRY_FIRST_LBA_OFFSET: usize = 32;
const GPT_ENTRY_LAST_LBA_OFFSET: usize = 40;
const GPT_ENTRY_ATTRIBUTES_OFFSET: usize = 48;
const GPT_ENTRY_NAME_OFFSET: usize = 56;
const GPT_NAME_BYTES: usize = 72;

fn read_u32(data: &[u8], offset: usize) -> Result<u32, ParseError> {
    let end = offset.checked_add(4).ok_or(ParseError::IntegerOverflow)?;
    let b = data.get(offset..end).ok_or(ParseError::OutOfBounds)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn read_u64(data: &[u8], offset: usize) -> Result<u64, ParseError> {
    let end = offset.checked_add(8).ok_or(ParseError::IntegerOverflow)?;
    let b = data.get(offset..end).ok_or(ParseError::OutOfBounds)?;
    Ok(u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0_u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn crc32_header_with_zeroed_checksum(header: &[u8]) -> u32 {
    let mut crc = !0_u32;
    for (index, &byte) in header.iter().enumerate() {
        let value = if (16..20).contains(&index) { 0 } else { byte };
        crc ^= u32::from(value);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn is_zero(bytes: &[u8]) -> bool {
    bytes.iter().all(|byte| *byte == 0)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GptHeader {
    pub revision: u32,
    pub header_size: u32,
    pub current_lba: u64,
    pub backup_lba: u64,
    pub first_usable_lba: u64,
    pub last_usable_lba: u64,
    pub disk_guid: [u8; 16],
    pub partition_entry_lba: u64,
    pub partition_entry_count: u32,
    pub partition_entry_size: u32,
    pub partition_entry_crc32: u32,
    pub header_range: Range<usize>,
    pub partition_array_range: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GptPartition {
    pub index: u32,
    pub type_guid: [u8; 16],
    pub unique_guid: [u8; 16],
    pub first_lba: u64,
    pub last_lba: u64,
    pub attributes: u64,
    pub name_utf16le: [u8; GPT_NAME_BYTES],
    pub byte_range: Range<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GptImageIndex {
    pub header: GptHeader,
    pub partitions: Vec<GptPartition>,
    pub disk_size_bytes: u64,
    pub sector_size: u32,
}

/// Parses a primary GPT (header at LBA 1) and its partition array.
/// The caller must provide the complete disk image and its logical sector size.
/// Backup-header consistency and protective-MBR validation are separate checks.
pub fn parse_primary_gpt(
    data: &[u8],
    sector_size: u32,
    budget: ParseBudget,
) -> Result<GptImageIndex, ParseError> {
    let disk_size = u64::try_from(data.len()).map_err(|_| ParseError::IntegerOverflow)?;
    budget.check_input(disk_size)?;
    if sector_size < 512 || !sector_size.is_power_of_two() {
        return Err(ParseError::InvalidAlignment);
    }
    let sector = u64::from(sector_size);
    if disk_size < sector.checked_mul(3).ok_or(ParseError::IntegerOverflow)? || disk_size % sector != 0 {
        return Err(ParseError::OutOfBounds);
    }
    let disk_sectors = disk_size / sector;
    let header_offset = usize::try_from(sector).map_err(|_| ParseError::IntegerOverflow)?;
    let sector_end = header_offset.checked_add(sector_size as usize).ok_or(ParseError::IntegerOverflow)?;
    let sector_bytes = data.get(header_offset..sector_end).ok_or(ParseError::OutOfBounds)?;
    if sector_bytes.get(..8) != Some(GPT_SIGNATURE.as_slice()) {
        return Err(ParseError::InvalidMagic);
    }

    let revision = read_u32(sector_bytes, 8)?;
    let header_size = read_u32(sector_bytes, 12)?;
    let header_size_usize = usize::try_from(header_size).map_err(|_| ParseError::IntegerOverflow)?;
    if header_size_usize < GPT_MIN_HEADER_SIZE || header_size_usize > sector_size as usize {
        return Err(ParseError::InvalidHeaderSize);
    }
    if read_u32(sector_bytes, 20)? != 0 {
        return Err(ParseError::InvalidBootHeader);
    }
    let current_lba = read_u64(sector_bytes, 24)?;
    let backup_lba = read_u64(sector_bytes, 32)?;
    let first_usable_lba = read_u64(sector_bytes, 40)?;
    let last_usable_lba = read_u64(sector_bytes, 48)?;
    let mut disk_guid = [0_u8; 16];
    disk_guid.copy_from_slice(&sector_bytes[56..72]);
    let partition_entry_lba = read_u64(sector_bytes, 72)?;
    let partition_entry_count = read_u32(sector_bytes, 80)?;
    let partition_entry_size = read_u32(sector_bytes, 84)?;
    let partition_entry_crc32 = read_u32(sector_bytes, 88)?;

    if current_lba != 1
        || backup_lba == current_lba
        || backup_lba >= disk_sectors
        || first_usable_lba > last_usable_lba
        || last_usable_lba >= disk_sectors
        || is_zero(&disk_guid)
        || revision < 0x0001_0000
        || partition_entry_count == 0
        || partition_entry_size < GPT_MIN_ENTRY_SIZE
        || partition_entry_size % 8 != 0
    {
        return Err(ParseError::InvalidBootHeader);
    }
    budget.check_items(partition_entry_count)?;

    let header_bytes = &sector_bytes[..header_size_usize];
    if crc32_header_with_zeroed_checksum(header_bytes) != read_u32(sector_bytes, 16)? {
        return Err(ParseError::ChecksumMismatch);
    }

    let array_size = u64::from(partition_entry_count)
        .checked_mul(u64::from(partition_entry_size))
        .ok_or(ParseError::IntegerOverflow)?;
    let array_offset = partition_entry_lba.checked_mul(sector).ok_or(ParseError::IntegerOverflow)?;
    let array_end = array_offset.checked_add(array_size).ok_or(ParseError::IntegerOverflow)?;
    let first_usable_byte = first_usable_lba.checked_mul(sector).ok_or(ParseError::IntegerOverflow)?;
    if partition_entry_lba < 2 || array_end > disk_size || array_end > first_usable_byte || array_end > backup_lba.checked_mul(sector).ok_or(ParseError::IntegerOverflow)? {
        return Err(ParseError::OutOfBounds);
    }
    let array_start_usize = usize::try_from(array_offset).map_err(|_| ParseError::IntegerOverflow)?;
    let array_end_usize = usize::try_from(array_end).map_err(|_| ParseError::IntegerOverflow)?;
    let entries_bytes = data.get(array_start_usize..array_end_usize).ok_or(ParseError::OutOfBounds)?;
    if crc32(entries_bytes) != partition_entry_crc32 {
        return Err(ParseError::ChecksumMismatch);
    }

    let mut partitions = Vec::new();
    for index in 0..partition_entry_count {
        let offset = usize::try_from(index)
            .map_err(|_| ParseError::IntegerOverflow)?
            .checked_mul(partition_entry_size as usize)
            .ok_or(ParseError::IntegerOverflow)?;
        let entry_end = offset.checked_add(partition_entry_size as usize).ok_or(ParseError::IntegerOverflow)?;
        let entry = entries_bytes.get(offset..entry_end).ok_or(ParseError::OutOfBounds)?;
        let type_bytes = &entry[GPT_ENTRY_TYPE_GUID_OFFSET..GPT_ENTRY_TYPE_GUID_OFFSET + 16];
        if is_zero(type_bytes) {
            continue;
        }
        let unique_bytes = &entry[GPT_ENTRY_UNIQUE_GUID_OFFSET..GPT_ENTRY_UNIQUE_GUID_OFFSET + 16];
        if is_zero(unique_bytes) {
            return Err(ParseError::InvalidBootHeader);
        }
        let first_lba = read_u64(entry, GPT_ENTRY_FIRST_LBA_OFFSET)?;
        let last_lba = read_u64(entry, GPT_ENTRY_LAST_LBA_OFFSET)?;
        if first_lba > last_lba || first_lba < first_usable_lba || last_lba > last_usable_lba {
            return Err(ParseError::OutOfBounds);
        }
        let start = first_lba.checked_mul(sector).ok_or(ParseError::IntegerOverflow)?;
        let end = last_lba.checked_add(1).and_then(|v| v.checked_mul(sector)).ok_or(ParseError::IntegerOverflow)?;
        let mut type_guid = [0_u8; 16];
        type_guid.copy_from_slice(type_bytes);
        let mut unique_guid = [0_u8; 16];
        unique_guid.copy_from_slice(unique_bytes);
        let mut name_utf16le = [0_u8; GPT_NAME_BYTES];
        name_utf16le.copy_from_slice(&entry[GPT_ENTRY_NAME_OFFSET..GPT_ENTRY_NAME_OFFSET + GPT_NAME_BYTES]);
        partitions.push(GptPartition {
            index,
            type_guid,
            unique_guid,
            first_lba,
            last_lba,
            attributes: read_u64(entry, GPT_ENTRY_ATTRIBUTES_OFFSET)?,
            name_utf16le,
            byte_range: start..end,
        });
    }

    let mut ranges: Vec<(u64, u64)> = partitions.iter().map(|p| (p.first_lba, p.last_lba)).collect();
    ranges.sort_unstable_by_key(|range| range.0);
    if ranges.windows(2).any(|pair| pair[0].1 >= pair[1].0) {
        return Err(ParseError::InvalidBootHeader);
    }

    Ok(GptImageIndex {
        header: GptHeader {
            revision,
            header_size,
            current_lba,
            backup_lba,
            first_usable_lba,
            last_usable_lba,
            disk_guid,
            partition_entry_lba,
            partition_entry_count,
            partition_entry_size,
            partition_entry_crc32,
            header_range: header_offset..header_offset + header_size_usize,
            partition_array_range: array_start_usize..array_end_usize,
        },
        partitions,
        disk_size_bytes: disk_size,
        sector_size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put_u32(data: &mut [u8], offset: usize, value: u32) {
        data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    fn put_u64(data: &mut [u8], offset: usize, value: u64) {
        data[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }
    fn budget() -> ParseBudget {
        ParseBudget { max_input_bytes: 1 << 20, max_items: 128, max_nesting_depth: 8 }
    }
    fn valid_disk() -> Vec<u8> {
        let mut disk = vec![0_u8; 100 * 512];
        let entries_offset = 2 * 512;
        let entry_size = 128_usize;
        let entry = &mut disk[entries_offset..entries_offset + entry_size];
        entry[0] = 0x28;
        entry[16] = 0x11;
        put_u64(entry, 32, 40);
        put_u64(entry, 40, 49);
        put_u64(entry, 48, 0x100);
        entry[56..64].copy_from_slice(&[b's', 0, b'y', 0, b's', 0, 0, 0]);
        let entries_crc = crc32(&disk[entries_offset..entries_offset + entry_size]);
        let header = &mut disk[512..1024];
        header[..8].copy_from_slice(GPT_SIGNATURE);
        put_u32(header, 8, 0x0001_0000);
        put_u32(header, 12, GPT_MIN_HEADER_SIZE as u32);
        put_u64(header, 24, 1);
        put_u64(header, 32, 99);
        put_u64(header, 40, 34);
        put_u64(header, 48, 98);
        header[56] = 0x22;
        put_u64(header, 72, 2);
        put_u32(header, 80, 1);
        put_u32(header, 84, 128);
        put_u32(header, 88, entries_crc);
        let checksum = crc32_header_with_zeroed_checksum(&header[..GPT_MIN_HEADER_SIZE]);
        put_u32(header, 16, checksum);
        disk
    }

    #[test]
    fn parses_primary_header_and_partition_ranges() {
        let disk = valid_disk();
        let parsed = parse_primary_gpt(&disk, 512, budget()).unwrap();
        assert_eq!(parsed.header.current_lba, 1);
        assert_eq!(parsed.header.backup_lba, 99);
        assert_eq!(parsed.partitions.len(), 1);
        assert_eq!(parsed.partitions[0].byte_range, 40 * 512..50 * 512);
        assert_eq!(parsed.partitions[0].attributes, 0x100);
    }

    #[test]
    fn rejects_bad_signature_and_header_checksum() {
        let mut disk = valid_disk();
        disk[512] = 0;
        assert_eq!(parse_primary_gpt(&disk, 512, budget()), Err(ParseError::InvalidMagic));
        let mut disk = valid_disk();
        disk[512 + 16] ^= 1;
        assert_eq!(parse_primary_gpt(&disk, 512, budget()), Err(ParseError::ChecksumMismatch));
    }

    #[test]
    fn rejects_partition_array_checksum_and_overlap() {
        let mut disk = valid_disk();
        disk[2 * 512 + 56] ^= 1;
        assert_eq!(parse_primary_gpt(&disk, 512, budget()), Err(ParseError::ChecksumMismatch));

        let mut disk = valid_disk();
        let entry = 2 * 512 + 128;
        disk[entry] = 0x28;
        disk[entry + 16] = 0x12;
        put_u64(&mut disk, entry + 32, 45);
        put_u64(&mut disk, entry + 40, 55);
        put_u32(&mut disk, 512 + 80, 2);
        let crc = crc32(&disk[2 * 512..2 * 512 + 256]);
        put_u32(&mut disk, 512 + 88, crc);
        let checksum = crc32_header_with_zeroed_checksum(&disk[512..512 + GPT_MIN_HEADER_SIZE]);
        put_u32(&mut disk, 512 + 16, checksum);
        assert_eq!(parse_primary_gpt(&disk, 512, budget()), Err(ParseError::InvalidBootHeader));
    }

    #[test]
    fn rejects_duplicate_unique_partition_guids() {
        let mut disk = valid_disk();
        let second = 2 * 512 + 128;
        let duplicate_guid = disk[2 * 512 + 16..2 * 512 + 32].to_vec();
        disk[second..second + 16].copy_from_slice(&duplicate_guid);
        disk[second + 32..second + 40].copy_from_slice(&60_u64.to_le_bytes());
        disk[second + 40..second + 48].copy_from_slice(&69_u64.to_le_bytes());
        put_u32(&mut disk, 512 + 80, 2);
        let entries_crc = crc32(&disk[2 * 512..2 * 512 + 256]);
        put_u32(&mut disk, 512 + 88, entries_crc);
        let checksum = crc32_header_with_zeroed_checksum(&disk[512..512 + GPT_MIN_HEADER_SIZE]);
        put_u32(&mut disk, 512 + 16, checksum);
        assert_eq!(parse_primary_gpt(&disk, 512, budget()), Err(ParseError::InvalidBootHeader));
    }

    #[test]
    fn enforces_disk_sector_and_entry_budgets() {
        let disk = valid_disk();
        assert_eq!(parse_primary_gpt(&disk, 512, ParseBudget { max_input_bytes: 1024, ..budget() }), Err(ParseError::ResourceLimit));
        assert_eq!(parse_primary_gpt(&disk, 4096, budget()), Err(ParseError::OutOfBounds));
        let mut disk = valid_disk();
        put_u32(&mut disk, 512 + 80, 129);
        assert_eq!(parse_primary_gpt(&disk, 512, budget()), Err(ParseError::ResourceLimit));
    }
}
