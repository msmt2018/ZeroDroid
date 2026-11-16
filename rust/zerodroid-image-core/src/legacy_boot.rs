//! Bounded indexing for Android legacy boot image header versions 0, 1 and 2.
//! Payload ranges borrow the input; this module never expands or copies image data.
use crate::{checked_align_up, ParseBudget, ParseError};
use core::ops::Range;

const MAGIC: &[u8; 8] = b"ANDROID!";
const V0_HEADER_BYTES: usize = 1632;
const V1_HEADER_BYTES: usize = 1648;
const V2_HEADER_BYTES: usize = 1660;

fn u32le(data: &[u8], offset: usize) -> Result<u32, ParseError> {
    let end = offset.checked_add(4).ok_or(ParseError::IntegerOverflow)?;
    let bytes = data.get(offset..end).ok_or(ParseError::OutOfBounds)?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn u64le(data: &[u8], offset: usize) -> Result<u64, ParseError> {
    let end = offset.checked_add(8).ok_or(ParseError::IntegerOverflow)?;
    let bytes = data.get(offset..end).ok_or(ParseError::OutOfBounds)?;
    Ok(u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5], bytes[6], bytes[7],
    ]))
}

fn as_range(start: u64, end: u64) -> Result<Range<usize>, ParseError> {
    if end < start {
        return Err(ParseError::IntegerOverflow);
    }
    Ok(
        usize::try_from(start).map_err(|_| ParseError::IntegerOverflow)?
            ..usize::try_from(end).map_err(|_| ParseError::IntegerOverflow)?,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LegacyBootHeader {
    pub header_version: u32,
    pub header_size: u32,
    pub page_size: u32,
    pub kernel_size: u32,
    pub ramdisk_size: u32,
    pub second_size: u32,
    pub recovery_dtbo_size: u32,
    pub dtb_size: u32,
    pub recovery_dtbo_offset: u64,
    pub dtb_address: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyBootImageIndex {
    pub header: LegacyBootHeader,
    pub header_page: Range<usize>,
    pub kernel: Range<usize>,
    pub ramdisk: Range<usize>,
    pub second: Option<Range<usize>>,
    pub recovery_dtbo: Option<Range<usize>>,
    pub dtb: Option<Range<usize>>,
    pub total_indexed_bytes: u64,
}

fn second_range_end(second_size: u32, second_end: u64, ramdisk_end: u64) -> u64 {
    if second_size == 0 { ramdisk_end } else { second_end }
}

/// Parses legacy boot.img v0-v2 framing and indexes its payloads without copying.
pub fn parse_legacy_boot_image(
    data: &[u8],
    budget: ParseBudget,
    max_payload_bytes: u64,
) -> Result<LegacyBootImageIndex, ParseError> {
    budget.check_input(u64::try_from(data.len()).map_err(|_| ParseError::IntegerOverflow)?)?;
    if data.len() < V0_HEADER_BYTES {
        return Err(ParseError::OutOfBounds);
    }
    if data.get(..8) != Some(MAGIC.as_slice()) {
        return Err(ParseError::InvalidMagic);
    }

    let kernel_size = u32le(data, 8)?;
    let ramdisk_size = u32le(data, 16)?;
    let second_size = u32le(data, 24)?;
    let page_size = u32le(data, 36)?;
    let header_version = u32le(data, 40)?;
    if kernel_size == 0 || page_size < 2048 || page_size > 65536 || !page_size.is_power_of_two() {
        return Err(ParseError::InvalidBootHeader);
    }

    let (header_size, recovery_dtbo_size, recovery_dtbo_offset, dtb_size, dtb_address) =
        match header_version {
            0 => (V0_HEADER_BYTES as u32, 0, 0, 0, 0),
            1 => {
                if data.len() < V1_HEADER_BYTES {
                    return Err(ParseError::OutOfBounds);
                }
                let size = u32le(data, 1644)?;
                if size as usize != V1_HEADER_BYTES {
                    return Err(ParseError::InvalidBootHeader);
                }
                (size, u32le(data, 1632)?, u64le(data, 1636)?, 0, 0)
            }
            2 => {
                if data.len() < V2_HEADER_BYTES {
                    return Err(ParseError::OutOfBounds);
                }
                let size = u32le(data, 1644)?;
                if size as usize != V2_HEADER_BYTES {
                    return Err(ParseError::InvalidBootHeader);
                }
                (
                    size,
                    u32le(data, 1632)?,
                    u64le(data, 1636)?,
                    u32le(data, 1648)?,
                    u64le(data, 1652)?,
                )
            }
            _ => return Err(ParseError::UnsupportedBootHeaderVersion),
        };

    let payload_total = u64::from(kernel_size)
        .checked_add(u64::from(ramdisk_size))
        .and_then(|n| n.checked_add(u64::from(second_size)))
        .and_then(|n| n.checked_add(u64::from(recovery_dtbo_size)))
        .and_then(|n| n.checked_add(u64::from(dtb_size)))
        .ok_or(ParseError::IntegerOverflow)?;
    if payload_total > max_payload_bytes {
        return Err(ParseError::ResourceLimit);
    }

    let page = u64::from(page_size);
    let kernel_start = page;
    let kernel_end = kernel_start
        .checked_add(u64::from(kernel_size))
        .ok_or(ParseError::IntegerOverflow)?;
    let ramdisk_start = checked_align_up(kernel_end, page)?;
    let ramdisk_end = ramdisk_start
        .checked_add(u64::from(ramdisk_size))
        .ok_or(ParseError::IntegerOverflow)?;
    let second_start = checked_align_up(ramdisk_end, page)?;
    let second_end = second_start
        .checked_add(u64::from(second_size))
        .ok_or(ParseError::IntegerOverflow)?;

    let recovery_range = if header_version >= 1 && recovery_dtbo_size != 0 {
        let expected_offset = checked_align_up(second_end, page)?;
        if recovery_dtbo_offset != expected_offset {
            return Err(ParseError::InvalidBootHeader);
        }
        let end = recovery_dtbo_offset
            .checked_add(u64::from(recovery_dtbo_size))
            .ok_or(ParseError::IntegerOverflow)?;
        Some(as_range(recovery_dtbo_offset, end)?)
    } else {
        if recovery_dtbo_size != 0 || recovery_dtbo_offset != 0 {
            return Err(ParseError::InvalidBootHeader);
        }
        None
    };
    let after_recovery = recovery_range
        .as_ref()
        .map(|range| range.end as u64)
        .unwrap_or(second_end);

    let dtb_range = if header_version >= 2 && dtb_size != 0 {
        let start = checked_align_up(after_recovery, page)?;
        let end = start
            .checked_add(u64::from(dtb_size))
            .ok_or(ParseError::IntegerOverflow)?;
        Some(as_range(start, end)?)
    } else {
        if dtb_size != 0 || dtb_address != 0 {
            return Err(ParseError::InvalidBootHeader);
        }
        None
    };

    let payload_end = second_range_end(second_size, second_end, ramdisk_end);
    let total_end = dtb_range
        .as_ref()
        .map(|range| range.end as u64)
        .or_else(|| recovery_range.as_ref().map(|range| range.end as u64))
        .unwrap_or(payload_end)
        .max(ramdisk_end)
        .max(kernel_end);
    if total_end > u64::try_from(data.len()).map_err(|_| ParseError::IntegerOverflow)? {
        return Err(ParseError::OutOfBounds);
    }

    let second_range = if second_size == 0 {
        None
    } else {
        Some(as_range(second_start, second_end)?)
    };
    Ok(LegacyBootImageIndex {
        header: LegacyBootHeader {
            header_version,
            header_size,
            page_size,
            kernel_size,
            ramdisk_size,
            second_size,
            recovery_dtbo_size,
            dtb_size,
            recovery_dtbo_offset,
            dtb_address,
        },
        header_page: as_range(0, page)?,
        kernel: as_range(kernel_start, kernel_end)?,
        ramdisk: as_range(ramdisk_start, ramdisk_end)?,
        second: second_range,
        recovery_dtbo: recovery_range,
        dtb: dtb_range,
        total_indexed_bytes: total_end,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_image(version: u32, page_size: u32, total_size: usize) -> Vec<u8> {
        let mut data = vec![0_u8; total_size];
        data[..8].copy_from_slice(MAGIC);
        data[8..12].copy_from_slice(&4096_u32.to_le_bytes());
        data[16..20].copy_from_slice(&2048_u32.to_le_bytes());
        data[24..28].copy_from_slice(&1024_u32.to_le_bytes());
        data[36..40].copy_from_slice(&page_size.to_le_bytes());
        data[40..44].copy_from_slice(&version.to_le_bytes());
        data
    }

    fn budget() -> ParseBudget {
        ParseBudget { max_input_bytes: 1 << 20, max_items: 128, max_nesting_depth: 8 }
    }

    #[test]
    fn indexes_v0_kernel_ramdisk_and_second() {
        let data = base_image(0, 4096, 16384);
        let parsed = parse_legacy_boot_image(&data, budget(), 8192).unwrap();
        assert_eq!(parsed.header.header_size, V0_HEADER_BYTES as u32);
        assert_eq!(parsed.kernel, 4096..8192);
        assert_eq!(parsed.ramdisk, 8192..10240);
        assert_eq!(parsed.second, Some(12288..13312));
        assert_eq!(parsed.recovery_dtbo, None);
        assert_eq!(parsed.dtb, None);
    }

    #[test]
    fn indexes_v1_recovery_dtbo_at_declared_offset() {
        let mut data = base_image(1, 4096, 20480);
        data[1632..1636].copy_from_slice(&512_u32.to_le_bytes());
        data[1636..1644].copy_from_slice(&16384_u64.to_le_bytes());
        data[1644..1648].copy_from_slice(&(V1_HEADER_BYTES as u32).to_le_bytes());
        let parsed = parse_legacy_boot_image(&data, budget(), 8192).unwrap();
        assert_eq!(parsed.recovery_dtbo, Some(16384..16896));
    }

    #[test]
    fn indexes_v2_dtb_after_recovery_section() {
        let mut data = base_image(2, 4096, 24576);
        data[1632..1636].copy_from_slice(&512_u32.to_le_bytes());
        data[1636..1644].copy_from_slice(&16384_u64.to_le_bytes());
        data[1644..1648].copy_from_slice(&(V2_HEADER_BYTES as u32).to_le_bytes());
        data[1648..1652].copy_from_slice(&1024_u32.to_le_bytes());
        data[1652..1660].copy_from_slice(&0x8800_0000_u64.to_le_bytes());
        let parsed = parse_legacy_boot_image(&data, budget(), 16384).unwrap();
        assert_eq!(parsed.recovery_dtbo, Some(16384..16896));
        assert_eq!(parsed.dtb, Some(20480..21504));
        assert_eq!(parsed.header.dtb_address, 0x8800_0000);
    }

    #[test]
    fn rejects_invalid_header_size_page_size_and_recovery_offset() {
        let mut data = base_image(1, 4096, 16384);
        data[1644..1648].copy_from_slice(&1_u32.to_le_bytes());
        assert_eq!(parse_legacy_boot_image(&data, budget(), 8192), Err(ParseError::InvalidBootHeader));
        let mut data = base_image(0, 3000, 16384);
        assert_eq!(parse_legacy_boot_image(&data, budget(), 8192), Err(ParseError::InvalidBootHeader));
        let mut data = base_image(1, 4096, 20480);
        data[1632..1636].copy_from_slice(&512_u32.to_le_bytes());
        data[1636..1644].copy_from_slice(&12288_u64.to_le_bytes());
        data[1644..1648].copy_from_slice(&(V1_HEADER_BYTES as u32).to_le_bytes());
        assert_eq!(parse_legacy_boot_image(&data, budget(), 8192), Err(ParseError::InvalidBootHeader));
    }

    #[test]
    fn enforces_payload_and_input_budgets() {
        let data = base_image(0, 4096, 16384);
        assert_eq!(parse_legacy_boot_image(&data, budget(), 7167), Err(ParseError::ResourceLimit));
        let mut limited = budget();
        limited.max_input_bytes = 1024;
        assert_eq!(parse_legacy_boot_image(&data, limited, 8192), Err(ParseError::ResourceLimit));
    }
}
