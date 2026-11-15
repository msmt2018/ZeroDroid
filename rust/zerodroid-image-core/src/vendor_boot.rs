use core::ops::Range;

use super::{checked_align_up, checked_range, ParseBudget, ParseError};

const VENDOR_BOOT_MAGIC: &[u8; 8] = b"VNDRBOOT";
const VENDOR_BOOT_V3_HEADER_SIZE: usize = 2112;
const VENDOR_BOOT_V4_HEADER_SIZE: usize = 2128;
const VENDOR_RAMDISK_TABLE_ENTRY_V4_SIZE: usize = 108;
const VENDOR_RAMDISK_NAME_SIZE: usize = 32;
const VENDOR_RAMDISK_BOARD_ID_WORDS: usize = 16;

fn read_u32(data: &[u8], offset: usize) -> Result<u32, ParseError> {
    let end = offset.checked_add(4).ok_or(ParseError::IntegerOverflow)?;
    let bytes = data.get(offset..end).ok_or(ParseError::OutOfBounds)?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn read_u64(data: &[u8], offset: usize) -> Result<u64, ParseError> {
    let end = offset.checked_add(8).ok_or(ParseError::IntegerOverflow)?;
    let bytes = data.get(offset..end).ok_or(ParseError::OutOfBounds)?;
    Ok(u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5], bytes[6], bytes[7],
    ]))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VendorBootHeaderV3V4 {
    pub header_version: u32,
    pub page_size: u32,
    pub kernel_addr: u32,
    pub ramdisk_addr: u32,
    pub vendor_ramdisk_size: u32,
    pub header_size: u32,
    pub dtb_size: u32,
    pub dtb_addr: u64,
    pub vendor_ramdisk_table_size: u32,
    pub vendor_ramdisk_table_entry_num: u32,
    pub vendor_ramdisk_table_entry_size: u32,
    pub bootconfig_size: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VendorRamdiskTableEntryV4 {
    pub ramdisk_size: u32,
    pub ramdisk_offset: u32,
    pub ramdisk_type: u32,
    pub ramdisk_name: [u8; VENDOR_RAMDISK_NAME_SIZE],
    pub board_id: [u32; VENDOR_RAMDISK_BOARD_ID_WORDS],
    pub payload: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VendorBootImageIndex {
    pub header: VendorBootHeaderV3V4,
    pub header_page: Range<usize>,
    pub vendor_ramdisk: Range<usize>,
    pub dtb: Range<usize>,
    pub vendor_ramdisk_table: Option<Range<usize>>,
    pub bootconfig: Option<Range<usize>>,
    pub ramdisk_entries: Vec<VendorRamdiskTableEntryV4>,
    pub total_indexed_bytes: u64,
}

/// Parses vendor_boot v3/v4 framing and indexes all payloads without copying them.
/// This validates table bounds and fragment ranges, not DTB semantics or bootconfig content.
pub fn parse_vendor_boot_image_v3_v4(
    data: &[u8],
    budget: ParseBudget,
    max_payload_bytes: u64,
) -> Result<VendorBootImageIndex, ParseError> {
    budget.check_input(u64::try_from(data.len()).map_err(|_| ParseError::IntegerOverflow)?)?;
    if data.len() < VENDOR_BOOT_V3_HEADER_SIZE {
        return Err(ParseError::OutOfBounds);
    }
    if data.get(..8) != Some(VENDOR_BOOT_MAGIC.as_slice()) {
        return Err(ParseError::InvalidMagic);
    }

    let header_version = read_u32(data, 8)?;
    let expected_header_size = match header_version {
        3 => VENDOR_BOOT_V3_HEADER_SIZE,
        4 => VENDOR_BOOT_V4_HEADER_SIZE,
        _ => return Err(ParseError::UnsupportedBootHeaderVersion),
    };
    if data.len() < expected_header_size {
        return Err(ParseError::OutOfBounds);
    }

    let page_size = read_u32(data, 12)?;
    let page_size_u64 = u64::from(page_size);
    if page_size < 512 || !page_size.is_power_of_two() || page_size > 65536 {
        return Err(ParseError::InvalidAlignment);
    }

    let kernel_addr = read_u32(data, 16)?;
    let ramdisk_addr = read_u32(data, 20)?;
    let vendor_ramdisk_size = read_u32(data, 24)?;
    let header_size = read_u32(data, 2096)?;
    let dtb_size = read_u32(data, 2100)?;
    let dtb_addr = read_u64(data, 2104)?;

    if usize::try_from(header_size).map_err(|_| ParseError::IntegerOverflow)?
        != expected_header_size
        || vendor_ramdisk_size == 0
        || dtb_size == 0
    {
        return Err(ParseError::InvalidBootHeader);
    }

    let (table_size, entry_count, entry_size, bootconfig_size) = if header_version == 4 {
        (
            read_u32(data, 2112)?,
            read_u32(data, 2116)?,
            read_u32(data, 2120)?,
            read_u32(data, 2124)?,
        )
    } else {
        (0, 0, 0, 0)
    };

    if header_version == 3 && data.len() < expected_header_size {
        return Err(ParseError::OutOfBounds);
    }
    if header_version == 4 {
        budget.check_items(entry_count)?;
        if entry_size < VENDOR_RAMDISK_TABLE_ENTRY_V4_SIZE as u32 {
            return Err(ParseError::InvalidHeaderSize);
        }
        let expected_table_size = u64::from(entry_count)
            .checked_mul(u64::from(entry_size))
            .ok_or(ParseError::IntegerOverflow)?;
        if expected_table_size != u64::from(table_size) {
            return Err(ParseError::InvalidHeaderSize);
        }
        if entry_count == 0 && table_size != 0 {
            return Err(ParseError::InvalidHeaderSize);
        }
    }

    let payload_total = u64::from(vendor_ramdisk_size)
        .checked_add(u64::from(dtb_size))
        .and_then(|n| n.checked_add(u64::from(table_size)))
        .and_then(|n| n.checked_add(u64::from(bootconfig_size)))
        .ok_or(ParseError::IntegerOverflow)?;
    if payload_total > max_payload_bytes {
        return Err(ParseError::ResourceLimit);
    }

    let header_page_end = checked_align_up(u64::from(header_size), page_size_u64)?;
    let ramdisk_start = header_page_end;
    let ramdisk_end = ramdisk_start
        .checked_add(u64::from(vendor_ramdisk_size))
        .ok_or(ParseError::IntegerOverflow)?;
    let dtb_start = checked_align_up(ramdisk_end, page_size_u64)?;
    let dtb_end = dtb_start
        .checked_add(u64::from(dtb_size))
        .ok_or(ParseError::IntegerOverflow)?;

    let (table_range, bootconfig_range, indexed_end) = if header_version == 4 {
        let table_start = checked_align_up(dtb_end, page_size_u64)?;
        let table_end = table_start
            .checked_add(u64::from(table_size))
            .ok_or(ParseError::IntegerOverflow)?;
        let bootconfig_start = checked_align_up(table_end, page_size_u64)?;
        let bootconfig_end = bootconfig_start
            .checked_add(u64::from(bootconfig_size))
            .ok_or(ParseError::IntegerOverflow)?;
        (
            Some(checked_range(table_start, u64::from(table_size), u64::MAX)?),
            Some(checked_range(
                bootconfig_start,
                u64::from(bootconfig_size),
                u64::MAX,
            )?),
            bootconfig_end,
        )
    } else {
        (None, None, dtb_end)
    };

    if indexed_end > u64::try_from(data.len()).map_err(|_| ParseError::IntegerOverflow)? {
        return Err(ParseError::OutOfBounds);
    }

    let header_page = checked_range(0, header_page_end, indexed_end)?;
    let vendor_ramdisk = checked_range(ramdisk_start, u64::from(vendor_ramdisk_size), indexed_end)?;
    let dtb = checked_range(dtb_start, u64::from(dtb_size), indexed_end)?;

    let mut ramdisk_entries = Vec::with_capacity(entry_count as usize);
    let mut fragment_ranges = Vec::with_capacity(entry_count as usize);
    if let Some(table) = &table_range {
        for index in 0..entry_count {
            let entry_offset = usize::try_from(table.start)
                .map_err(|_| ParseError::IntegerOverflow)?
                .checked_add(
                    usize::try_from(index)
                        .map_err(|_| ParseError::IntegerOverflow)?
                        .checked_mul(entry_size as usize)
                        .ok_or(ParseError::IntegerOverflow)?,
                )
                .ok_or(ParseError::IntegerOverflow)?;
            let entry_end = entry_offset
                .checked_add(VENDOR_RAMDISK_TABLE_ENTRY_V4_SIZE)
                .ok_or(ParseError::IntegerOverflow)?;
            let bytes = data.get(entry_offset..entry_end).ok_or(ParseError::OutOfBounds)?;
            let ramdisk_size = read_u32(bytes, 0)?;
            let ramdisk_offset = read_u32(bytes, 4)?;
            let ramdisk_type = read_u32(bytes, 8)?;
            if ramdisk_size == 0 {
                return Err(ParseError::InvalidBootHeader);
            }
            let fragment_end = u64::from(ramdisk_offset)
                .checked_add(u64::from(ramdisk_size))
                .ok_or(ParseError::IntegerOverflow)?;
            if fragment_end > u64::from(vendor_ramdisk_size) {
                return Err(ParseError::OutOfBounds);
            }
            let mut ramdisk_name = [0_u8; VENDOR_RAMDISK_NAME_SIZE];
            ramdisk_name.copy_from_slice(&bytes[12..44]);
            let mut board_id = [0_u32; VENDOR_RAMDISK_BOARD_ID_WORDS];
            for (word_index, word) in board_id.iter_mut().enumerate() {
                *word = read_u32(bytes, 44 + word_index * 4)?;
            }
            let payload_start = ramdisk_start
                .checked_add(u64::from(ramdisk_offset))
                .ok_or(ParseError::IntegerOverflow)?;
            let payload = checked_range(payload_start, u64::from(ramdisk_size), ramdisk_end)?;
            fragment_ranges.push(payload.clone());
            ramdisk_entries.push(VendorRamdiskTableEntryV4 {
                ramdisk_size,
                ramdisk_offset,
                ramdisk_type,
                ramdisk_name,
                board_id,
                payload,
            });
        }
    }
    // Sort a separate range list so the public entries retain their on-disk table order.
    // This keeps overlap validation O(n log n), rather than quadratic for attacker-controlled tables.
    fragment_ranges.sort_unstable_by_key(|range| range.start);
    if fragment_ranges.windows(2).any(|pair| pair[0].end > pair[1].start) {
        return Err(ParseError::InvalidBootHeader);
    }

    Ok(VendorBootImageIndex {
        header: VendorBootHeaderV3V4 {
            header_version,
            page_size,
            kernel_addr,
            ramdisk_addr,
            vendor_ramdisk_size,
            header_size,
            dtb_size,
            dtb_addr,
            vendor_ramdisk_table_size: table_size,
            vendor_ramdisk_table_entry_num: entry_count,
            vendor_ramdisk_table_entry_size: entry_size,
            bootconfig_size,
        },
        header_page,
        vendor_ramdisk,
        dtb,
        vendor_ramdisk_table: table_range,
        bootconfig: bootconfig_range,
        ramdisk_entries,
        total_indexed_bytes: indexed_end,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budget() -> ParseBudget {
        ParseBudget {
            max_input_bytes: 1 << 20,
            max_items: 128,
            max_nesting_depth: 8,
        }
    }

    fn base_image(version: u32, ramdisk_size: u32, dtb_size: u32) -> Vec<u8> {
        let header_size = if version == 3 { 2112 } else { 2128 };
        let page_size = 4096_usize;
        let mut image = vec![0_u8; page_size * 3];
        image[..8].copy_from_slice(b"VNDRBOOT");
        image[8..12].copy_from_slice(&version.to_le_bytes());
        image[12..16].copy_from_slice(&(page_size as u32).to_le_bytes());
        image[16..20].copy_from_slice(&0x8000_0000_u32.to_le_bytes());
        image[20..24].copy_from_slice(&0x8100_0000_u32.to_le_bytes());
        image[24..28].copy_from_slice(&ramdisk_size.to_le_bytes());
        image[2096..2100].copy_from_slice(&(header_size as u32).to_le_bytes());
        image[2100..2104].copy_from_slice(&dtb_size.to_le_bytes());
        image[2104..2112].copy_from_slice(&0x8200_0000_u64.to_le_bytes());
        image
    }

    #[test]
    fn v3_indexes_vendor_ramdisk_and_dtb() {
        let image = base_image(3, 1024, 2048);
        let parsed = parse_vendor_boot_image_v3_v4(&image, budget(), 4096).unwrap();
        assert_eq!(parsed.header_page, 0..4096);
        assert_eq!(parsed.vendor_ramdisk, 4096..5120);
        assert_eq!(parsed.dtb, 8192..10240);
        assert!(parsed.vendor_ramdisk_table.is_none());
        assert!(parsed.bootconfig.is_none());
    }

    #[test]
    fn v4_indexes_table_entries_and_bootconfig() {
        let mut image = base_image(4, 1024, 2048);
        image[2112..2116].copy_from_slice(&108_u32.to_le_bytes());
        image[2116..2120].copy_from_slice(&1_u32.to_le_bytes());
        image[2120..2124].copy_from_slice(&108_u32.to_le_bytes());
        image[2124..2128].copy_from_slice(&512_u32.to_le_bytes());
        // The v4 table follows the page-aligned DTB at offset 12288.
        image.resize(16384 + 512, 0);
        image[12288..12292].copy_from_slice(&512_u32.to_le_bytes());
        image[12292..12296].copy_from_slice(&0_u32.to_le_bytes());
        image[12296..12300].copy_from_slice(&1_u32.to_le_bytes());
        image[12300..12305].copy_from_slice(b"init\0");
        let parsed = parse_vendor_boot_image_v3_v4(&image, budget(), 4096).unwrap();
        assert_eq!(parsed.vendor_ramdisk_table, Some(12288..12396));
        assert_eq!(parsed.bootconfig, Some(16384..16896));
        assert_eq!(parsed.ramdisk_entries.len(), 1);
        assert_eq!(parsed.ramdisk_entries[0].payload, 4096..4608);
        assert_eq!(parsed.ramdisk_entries[0].ramdisk_type, 1);
    }

    #[test]
    fn rejects_bad_magic_version_and_header_size() {
        let mut image = base_image(3, 1024, 2048);
        image[0] = 0;
        assert_eq!(parse_vendor_boot_image_v3_v4(&image, budget(), 4096), Err(ParseError::InvalidMagic));
        let mut image = base_image(3, 1024, 2048);
        image[8..12].copy_from_slice(&5_u32.to_le_bytes());
        assert_eq!(parse_vendor_boot_image_v3_v4(&image, budget(), 4096), Err(ParseError::UnsupportedBootHeaderVersion));
        let mut image = base_image(3, 1024, 2048);
        image[2096..2100].copy_from_slice(&1_u32.to_le_bytes());
        assert_eq!(parse_vendor_boot_image_v3_v4(&image, budget(), 4096), Err(ParseError::InvalidBootHeader));
    }

    #[test]
    fn rejects_table_size_mismatch_and_out_of_bounds_fragments() {
        let mut image = base_image(4, 1024, 2048);
        image[2112..2116].copy_from_slice(&108_u32.to_le_bytes());
        image[2116..2120].copy_from_slice(&1_u32.to_le_bytes());
        image[2120..2124].copy_from_slice(&107_u32.to_le_bytes());
        assert_eq!(parse_vendor_boot_image_v3_v4(&image, budget(), 4096), Err(ParseError::InvalidHeaderSize));

        image[2120..2124].copy_from_slice(&108_u32.to_le_bytes());
        image.resize(16384, 0);
        image[12288..12292].copy_from_slice(&512_u32.to_le_bytes());
        image[12292..12296].copy_from_slice(&1024_u32.to_le_bytes());
        assert_eq!(parse_vendor_boot_image_v3_v4(&image, budget(), 4096), Err(ParseError::OutOfBounds));
    }

    #[test]
    fn rejects_overlapping_ramdisk_fragments() {
        let mut image = base_image(4, 1024, 2048);
        image[2112..2116].copy_from_slice(&216_u32.to_le_bytes());
        image[2116..2120].copy_from_slice(&2_u32.to_le_bytes());
        image[2120..2124].copy_from_slice(&108_u32.to_le_bytes());
        image.resize(16384, 0);
        for entry in 0..2 {
            let offset = 12288 + entry * 108;
            image[offset..offset + 4].copy_from_slice(&512_u32.to_le_bytes());
            image[offset + 4..offset + 8].copy_from_slice(&(entry as u32 * 256).to_le_bytes());
        }
        assert_eq!(
            parse_vendor_boot_image_v3_v4(&image, budget(), 4096),
            Err(ParseError::InvalidBootHeader)
        );
    }

    #[test]
    fn enforces_input_item_and_payload_budgets() {
        let image = base_image(3, 1024, 2048);
        assert_eq!(parse_vendor_boot_image_v3_v4(&image, budget(), 3071), Err(ParseError::ResourceLimit));
        let mut image = base_image(4, 1024, 2048);
        image[2112..2116].copy_from_slice(&108_u32.to_le_bytes());
        image[2116..2120].copy_from_slice(&129_u32.to_le_bytes());
        image[2120..2124].copy_from_slice(&1_u32.to_le_bytes());
        assert_eq!(parse_vendor_boot_image_v3_v4(&image, budget(), u64::MAX), Err(ParseError::ResourceLimit));
    }
}
