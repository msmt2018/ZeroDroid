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
            VendorRamdiskFragmentType::try_from(ramdisk_type)?;
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



/// Semantic role of a range borrowed from one of the boot-chain images.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootMaterialKind {
    Kernel,
    BootRamdisk,
    BootSignature,
    InitRamdisk,
    InitBootSignature,
    VendorRamdisk,
    VendorRamdiskFragment,
    DeviceTreeBlob,
    VendorRamdiskTable,
    Bootconfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootMaterialSource {
    BootImage,
    InitBootImage,
    VendorBootImage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootMaterialEntry {
    pub kind: BootMaterialKind,
    pub source: BootMaterialSource,
    /// Byte range into the caller-owned source image; no payload is copied.
    pub range: Range<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootMaterialError {
    ResourceLimit,
    InvalidImageIndex,
    MissingKernel,
    MissingBootRamdisk,
    MissingRamdisk,
    InvalidInitBootSemantics,
}

/// A bounded, range-only join of already parsed Android boot-chain images.
/// This is an inventory, not a signature, AVB, DTB, or bootability verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootMaterialSet {
    pub entries: Vec<BootMaterialEntry>,
    pub has_init_boot: bool,
    pub has_vendor_boot: bool,
    pub has_device_tree: bool,
    /// Always false until the complete boot chain and trust policy are verified.
    pub boot_chain_verified: bool,
}

fn push_material(
    entries: &mut Vec<BootMaterialEntry>,
    kind: BootMaterialKind,
    source: BootMaterialSource,
    range: Range<usize>,
    image_len: usize,
    max_entries: usize,
) -> Result<(), BootMaterialError> {
    if range.start > range.end || range.end > image_len {
        return Err(BootMaterialError::InvalidImageIndex);
    }
    if entries.len() >= max_entries {
        return Err(BootMaterialError::ResourceLimit);
    }
    entries.push(BootMaterialEntry { kind, source, range });
    Ok(())
}

/// Joins boot.img with optional init_boot.img and vendor_boot.img indexes.
/// Every range is rechecked against the exact source byte slice supplied by the caller.
/// Callers must keep those immutable slices alive while consuming the returned ranges.
pub fn build_boot_material_set(
    boot_data: &[u8],
    boot: &crate::AndroidBootImageIndex,
    init_boot: Option<(&[u8], &crate::InitBootImageIndex)>,
    vendor_boot: Option<(&[u8], &VendorBootImageIndex)>,
    max_entries: usize,
) -> Result<BootMaterialSet, BootMaterialError> {
    if boot.header.kernel_size == 0 {
        return Err(BootMaterialError::MissingKernel);
    }
    let has_boot_ramdisk = boot.header.ramdisk_size != 0;
    if boot.total_indexed_bytes > boot_data.len() as u64 {
        return Err(BootMaterialError::InvalidImageIndex);
    }

    let mut entries = Vec::new();
    push_material(&mut entries, BootMaterialKind::Kernel, BootMaterialSource::BootImage,
        boot.kernel.clone(), boot_data.len(), max_entries)?;
    if has_boot_ramdisk {
        push_material(&mut entries, BootMaterialKind::BootRamdisk, BootMaterialSource::BootImage,
            boot.ramdisk.clone(), boot_data.len(), max_entries)?;
    }
    if let Some(range) = &boot.boot_signature {
        push_material(&mut entries, BootMaterialKind::BootSignature, BootMaterialSource::BootImage,
            range.clone(), boot_data.len(), max_entries)?;
    }

    if let Some((data, index)) = init_boot {
        if index.header.header_version != 4 || index.header.kernel_size != 0 || index.header.ramdisk_size == 0 {
            return Err(BootMaterialError::InvalidInitBootSemantics);
        }
        if index.total_indexed_bytes > data.len() as u64 {
            return Err(BootMaterialError::InvalidImageIndex);
        }
        push_material(&mut entries, BootMaterialKind::InitRamdisk, BootMaterialSource::InitBootImage,
            index.ramdisk.clone(), data.len(), max_entries)?;
        if let Some(range) = &index.boot_signature {
            push_material(&mut entries, BootMaterialKind::InitBootSignature, BootMaterialSource::InitBootImage,
                range.clone(), data.len(), max_entries)?;
        }
    }

    let has_device_tree = if let Some((data, index)) = vendor_boot {
        if index.total_indexed_bytes > data.len() as u64 {
            return Err(BootMaterialError::InvalidImageIndex);
        }
        push_material(&mut entries, BootMaterialKind::VendorRamdisk, BootMaterialSource::VendorBootImage,
            index.vendor_ramdisk.clone(), data.len(), max_entries)?;
        push_material(&mut entries, BootMaterialKind::DeviceTreeBlob, BootMaterialSource::VendorBootImage,
            index.dtb.clone(), data.len(), max_entries)?;
        if let Some(range) = &index.vendor_ramdisk_table {
            push_material(&mut entries, BootMaterialKind::VendorRamdiskTable, BootMaterialSource::VendorBootImage,
                range.clone(), data.len(), max_entries)?;
        }
        if let Some(range) = &index.bootconfig {
            push_material(&mut entries, BootMaterialKind::Bootconfig, BootMaterialSource::VendorBootImage,
                range.clone(), data.len(), max_entries)?;
        }
        true
    } else {
        false
    };

    let has_init_ramdisk = init_boot.is_some();
    let has_vendor_ramdisk = vendor_boot
        .map(|(_, index)| index.header.vendor_ramdisk_size != 0)
        .unwrap_or(false);
    if !has_boot_ramdisk && !has_init_ramdisk && !has_vendor_ramdisk {
        return Err(BootMaterialError::MissingRamdisk);
    }

    Ok(BootMaterialSet {
        entries,
        has_init_boot: init_boot.is_some(),
        has_vendor_boot: vendor_boot.is_some(),
        has_device_tree,
        boot_chain_verified: false,
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

    fn synthetic_boot_index() -> crate::AndroidBootImageIndex {
        crate::AndroidBootImageIndex {
            header: crate::AndroidBootHeaderV3V4 {
                header_version: 3, header_size: 1580, kernel_size: 4096,
                ramdisk_size: 1024, os_version: 0, signature_size: 0,
            },
            header_page: 0..4096,
            kernel: 4096..8192,
            ramdisk: 8192..9216,
            boot_signature: None,
            total_indexed_bytes: 9216,
        }
    }

    #[test]
    fn boot_material_set_joins_boot_and_vendor_ranges_without_copying() {
        let boot_data = vec![0_u8; 9216];
        let vendor_data = base_image(3, 1024, 2048);
        let vendor_index = parse_vendor_boot_image_v3_v4(&vendor_data, budget(), 4096).unwrap();
        let set = build_boot_material_set(
            &boot_data, &synthetic_boot_index(), None,
            Some((&vendor_data, &vendor_index)), 8,
        ).unwrap();
        assert_eq!(set.entries.len(), 4);
        assert!(set.has_vendor_boot);
        assert!(set.has_device_tree);
        assert!(!set.has_init_boot);
        assert!(!set.boot_chain_verified);
        assert_eq!(set.entries[0].kind, BootMaterialKind::Kernel);
        assert_eq!(set.entries[3].kind, BootMaterialKind::DeviceTreeBlob);
    }

    #[test]
    fn boot_material_set_checks_source_ranges_and_entry_budget() {
        let mut invalid = synthetic_boot_index();
        invalid.kernel = 9000..10000;
        let boot_data = vec![0_u8; 9216];
        assert_eq!(
            build_boot_material_set(&boot_data, &invalid, None, None, 8),
            Err(BootMaterialError::InvalidImageIndex)
        );
        assert_eq!(
            build_boot_material_set(&boot_data, &synthetic_boot_index(), None, None, 1),
            Err(BootMaterialError::ResourceLimit)
        );
    }

    #[test]
    fn boot_material_set_accepts_init_boot_when_boot_ramdisk_is_empty() {
        let boot_data = vec![0_u8; 8192];
        let mut boot = synthetic_boot_index();
        boot.header.ramdisk_size = 0;
        boot.ramdisk = 8192..8192;
        boot.total_indexed_bytes = 8192;

        let init_data = vec![0_u8; 8192];
        let init = crate::InitBootImageIndex {
            header: crate::AndroidBootHeaderV3V4 {
                header_version: 4, header_size: 1584, kernel_size: 0,
                ramdisk_size: 4096, os_version: 0, signature_size: 0,
            },
            header_page: 0..4096,
            ramdisk: 4096..8192,
            boot_signature: None,
            total_indexed_bytes: 8192,
        };
        let set = build_boot_material_set(
            &boot_data, &boot, Some((&init_data, &init)), None, 8,
        ).unwrap();
        assert!(set.has_init_boot);
        assert!(!set.entries.iter().any(|entry| entry.kind == BootMaterialKind::BootRamdisk));
        assert!(set.entries.iter().any(|entry| entry.kind == BootMaterialKind::InitRamdisk));
    }

    #[test]
    fn boot_material_set_rejects_invalid_init_boot_semantics() {
        let boot_data = vec![0_u8; 9216];
        let init_data = vec![0_u8; 8192];
        let mut init = crate::InitBootImageIndex {
            header: crate::AndroidBootHeaderV3V4 {
                header_version: 4, header_size: 1584, kernel_size: 1,
                ramdisk_size: 1024, os_version: 0, signature_size: 0,
            },
            header_page: 0..4096,
            ramdisk: 4096..5120,
            boot_signature: None,
            total_indexed_bytes: 5120,
        };
        assert_eq!(
            build_boot_material_set(&boot_data, &synthetic_boot_index(),
                Some((&init_data, &init)), None, 8),
            Err(BootMaterialError::InvalidInitBootSemantics)
        );
        init.header.kernel_size = 0;
        let set = build_boot_material_set(&boot_data, &synthetic_boot_index(),
            Some((&init_data, &init)), None, 8).unwrap();
        assert!(set.has_init_boot);
        assert!(set.entries.iter().any(|entry| entry.kind == BootMaterialKind::InitRamdisk));
    }

}


/// Boot mode used when selecting v4 vendor ramdisk fragments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VendorRamdiskBootMode {
    Normal,
    Recovery,
}

/// Android vendor ramdisk fragment types from the v4 table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum VendorRamdiskFragmentType {
    None = 0,
    Platform = 1,
    Recovery = 2,
    Dlkm = 3,
}

impl TryFrom<u32> for VendorRamdiskFragmentType {
    type Error = ParseError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::None),
            1 => Ok(Self::Platform),
            2 => Ok(Self::Recovery),
            3 => Ok(Self::Dlkm),
            _ => Err(ParseError::InvalidBootHeader),
        }
    }
}

/// A selected fragment keeps its table index and source range; payload bytes are never copied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedVendorRamdiskFragment {
    pub table_index: usize,
    pub fragment_type: VendorRamdiskFragmentType,
    pub name: [u8; VENDOR_RAMDISK_NAME_SIZE],
    pub payload: Range<usize>,
}

/// Matches a fragment board ID against the target board ID.
/// A zero fragment word is a wildcard; every non-zero word must match.
fn board_id_matches(
    fragment_board_id: &[u32; VENDOR_RAMDISK_BOARD_ID_WORDS],
    target_board_id: &[u32; VENDOR_RAMDISK_BOARD_ID_WORDS],
) -> bool {
    fragment_board_id
        .iter()
        .zip(target_board_id.iter())
        .all(|(required, actual)| *required == 0 || required == actual)
}

/// Selects v4 fragments for a boot mode and target board.
///
/// Normal mode selects matching PLATFORM and DLKM fragments. Recovery mode selects
/// matching RECOVERY fragments. NONE fragments are deliberately not selected because
/// their semantics are not sufficient to infer a safe boot-mode policy. Unknown fragment
/// types are rejected by the parser/selector. The caller remains responsible for the
/// Android bootloader's exact platform-ID source and final ramdisk assembly order.
pub fn select_vendor_ramdisk_fragments(
    index: &VendorBootImageIndex,
    target_board_id: &[u32; VENDOR_RAMDISK_BOARD_ID_WORDS],
    mode: VendorRamdiskBootMode,
    max_selected: usize,
) -> Result<Vec<SelectedVendorRamdiskFragment>, BootMaterialError> {
    if index.header.header_version != 4 {
        return Ok(Vec::new());
    }
    let mut selected = Vec::new();
    for (table_index, entry) in index.ramdisk_entries.iter().enumerate() {
        let fragment_type = VendorRamdiskFragmentType::try_from(entry.ramdisk_type)
            .map_err(|_| BootMaterialError::InvalidImageIndex)?;
        let mode_matches = match mode {
            VendorRamdiskBootMode::Normal => matches!(
                fragment_type,
                VendorRamdiskFragmentType::Platform | VendorRamdiskFragmentType::Dlkm
            ),
            VendorRamdiskBootMode::Recovery => {
                fragment_type == VendorRamdiskFragmentType::Recovery
            }
        };
        if !mode_matches || !board_id_matches(&entry.board_id, target_board_id) {
            continue;
        }
        if selected.len() >= max_selected {
            return Err(BootMaterialError::ResourceLimit);
        }
        selected.push(SelectedVendorRamdiskFragment {
            table_index,
            fragment_type,
            name: entry.ramdisk_name,
            payload: entry.payload.clone(),
        });
    }
    Ok(selected)
}


#[cfg(test)]
mod fragment_selection_tests {
    use super::*;

    fn v4_image() -> Vec<u8> {
        let mut image = vec![0_u8; 16_384];
        image[..8].copy_from_slice(b"VNDRBOOT");
        image[8..12].copy_from_slice(&4_u32.to_le_bytes());
        image[12..16].copy_from_slice(&4096_u32.to_le_bytes());
        image[24..28].copy_from_slice(&1024_u32.to_le_bytes());
        image[2096..2100].copy_from_slice(&2128_u32.to_le_bytes());
        image[2100..2104].copy_from_slice(&2048_u32.to_le_bytes());
        image[2104..2112].copy_from_slice(&0x8200_0000_u64.to_le_bytes());
        image[2112..2116].copy_from_slice(&216_u32.to_le_bytes());
        image[2116..2120].copy_from_slice(&2_u32.to_le_bytes());
        image[2120..2124].copy_from_slice(&108_u32.to_le_bytes());
        image[2124..2128].copy_from_slice(&0_u32.to_le_bytes());

        // Two non-overlapping 256-byte fragments within the 1 KiB vendor ramdisk.
        image[4096..4100].copy_from_slice(&[1, 2, 3, 4]);
        let table = 12_288;
        for (slot, kind, board_word) in [(0_usize, 1_u32, 0x1234_u32), (1, 2, 0_u32)] {
            let offset = table + slot * 108;
            image[offset..offset + 4].copy_from_slice(&256_u32.to_le_bytes());
            image[offset + 4..offset + 8]
                .copy_from_slice(&(slot as u32 * 256).to_le_bytes());
            image[offset + 8..offset + 12].copy_from_slice(&kind.to_le_bytes());
            image[offset + 12..offset + 16].copy_from_slice(if slot == 0 {
                b"plat"
            } else {
                b"reco"
            });
            image[offset + 44..offset + 48].copy_from_slice(&board_word.to_le_bytes());
        }
        image
    }

    fn board(id: u32) -> [u32; VENDOR_RAMDISK_BOARD_ID_WORDS] {
        let mut value = [0_u32; VENDOR_RAMDISK_BOARD_ID_WORDS];
        value[0] = id;
        value
    }

    #[test]
    fn normal_mode_selects_matching_platform_fragments_only() {
        let image = v4_image();
        let index = parse_vendor_boot_image_v3_v4(
            &image,
            ParseBudget { max_input_bytes: 1 << 20, max_items: 16, max_nesting_depth: 4 },
            4096,
        ).unwrap();
        let selected = select_vendor_ramdisk_fragments(
            &index, &board(0x1234), VendorRamdiskBootMode::Normal, 4
        ).unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].table_index, 0);
        assert_eq!(selected[0].fragment_type, VendorRamdiskFragmentType::Platform);
        assert_eq!(selected[0].payload, 4096..4352);
    }

    #[test]
    fn recovery_mode_selects_recovery_fragments_and_zero_board_id_is_wildcard() {
        let image = v4_image();
        let index = parse_vendor_boot_image_v3_v4(
            &image,
            ParseBudget { max_input_bytes: 1 << 20, max_items: 16, max_nesting_depth: 4 },
            4096,
        ).unwrap();
        let selected = select_vendor_ramdisk_fragments(
            &index, &board(0x9999), VendorRamdiskBootMode::Recovery, 1
        ).unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].table_index, 1);
        assert_eq!(selected[0].fragment_type, VendorRamdiskFragmentType::Recovery);
    }

    #[test]
    fn selection_budget_limits_matches_not_total_table_entries() {
        let image = v4_image();
        let index = parse_vendor_boot_image_v3_v4(
            &image,
            ParseBudget { max_input_bytes: 1 << 20, max_items: 16, max_nesting_depth: 4 },
            4096,
        ).unwrap();
        assert_eq!(
            select_vendor_ramdisk_fragments(
                &index, &board(0x1234), VendorRamdiskBootMode::Normal, 0
            ),
            Err(BootMaterialError::ResourceLimit)
        );
    }

    #[test]
    fn selected_materials_reference_only_selected_fragment_payloads() {
        let image = v4_image();
        let index = parse_vendor_boot_image_v3_v4(
            &image,
            ParseBudget { max_input_bytes: 1 << 20, max_items: 16, max_nesting_depth: 4 },
            4096,
        ).unwrap();
        let materials = build_selected_vendor_ramdisk_materials(
            &image, &index, &board(0x1234), VendorRamdiskBootMode::Normal, 2
        ).unwrap();
        assert_eq!(materials.len(), 1);
        assert_eq!(materials[0].kind, BootMaterialKind::VendorRamdiskFragment);
        assert_eq!(materials[0].source, BootMaterialSource::VendorBootImage);
        assert_eq!(materials[0].range, 4096..4352);
    }

    #[test]
    fn parser_rejects_unknown_fragment_type() {
        let mut image = v4_image();
        image[12_288 + 8..12_288 + 12].copy_from_slice(&99_u32.to_le_bytes());
        assert_eq!(
            parse_vendor_boot_image_v3_v4(
                &image,
                ParseBudget { max_input_bytes: 1 << 20, max_items: 16, max_nesting_depth: 4 },
                4096,
            ),
            Err(ParseError::InvalidBootHeader)
        );
    }
}


/// Builds selected vendor fragment materials without exposing the containing monolithic
/// vendor-ramdisk range as a load instruction. The returned entries still borrow source bytes.
pub fn build_selected_vendor_ramdisk_materials(
    vendor_boot_data: &[u8],
    index: &VendorBootImageIndex,
    target_board_id: &[u32; VENDOR_RAMDISK_BOARD_ID_WORDS],
    mode: VendorRamdiskBootMode,
    max_entries: usize,
) -> Result<Vec<BootMaterialEntry>, BootMaterialError> {
    if index.total_indexed_bytes > vendor_boot_data.len() as u64 {
        return Err(BootMaterialError::InvalidImageIndex);
    }
    let selected = select_vendor_ramdisk_fragments(index, target_board_id, mode, max_entries)?;
    let mut materials = Vec::with_capacity(selected.len());
    for fragment in selected {
        if fragment.payload.start > fragment.payload.end
            || fragment.payload.end > vendor_boot_data.len()
        {
            return Err(BootMaterialError::InvalidImageIndex);
        }
        materials.push(BootMaterialEntry {
            kind: BootMaterialKind::VendorRamdiskFragment,
            source: BootMaterialSource::VendorBootImage,
            range: fragment.payload,
        });
    }
    Ok(materials)
}
