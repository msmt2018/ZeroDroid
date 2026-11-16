//! ARM64 Linux Image header validation used by the Guest boot pipeline.
use crate::{ParseBudget, ParseError};
use core::ops::Range;

const ARM64_IMAGE_HEADER_BYTES: usize = 64;
const ARM64_IMAGE_MAGIC: u32 = 0x644d_5241;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arm64KernelPageSize {
    Unspecified,
    KiB4,
    KiB16,
    KiB64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Arm64KernelImageHeader {
    pub text_offset: u64,
    /// Zero in the Image header means that the kernel does not declare its size.
    pub declared_image_size: u64,
    pub effective_image_size: u64,
    pub flags: u64,
    pub page_size: Arm64KernelPageSize,
    pub big_endian: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arm64KernelImageIndex {
    pub header: Arm64KernelImageHeader,
    pub image: Range<usize>,
}

/// Validates an uncompressed ARM64 Linux Image header without copying its payload.
pub fn parse_arm64_linux_image(
    data: &[u8],
    budget: ParseBudget,
    max_image_bytes: u64,
) -> Result<Arm64KernelImageIndex, ParseError> {
    budget.check_input(u64::try_from(data.len()).map_err(|_| ParseError::IntegerOverflow)?)?;
    if data.len() < ARM64_IMAGE_HEADER_BYTES {
        return Err(ParseError::OutOfBounds);
    }
    let magic = u32le(data, 56)?;
    if magic != ARM64_IMAGE_MAGIC {
        return Err(ParseError::InvalidMagic);
    }

    let text_offset = u64le(data, 8)?;
    let declared_image_size = u64le(data, 16)?;
    let flags = u64le(data, 24)?;
    let actual_size = u64::try_from(data.len()).map_err(|_| ParseError::IntegerOverflow)?;
    let effective_image_size = if declared_image_size == 0 {
        actual_size
    } else {
        declared_image_size
    };
    if effective_image_size < ARM64_IMAGE_HEADER_BYTES as u64
        || effective_image_size > actual_size
        || effective_image_size > max_image_bytes
    {
        return Err(ParseError::InvalidBootHeader);
    }

    let big_endian = flags & 1 != 0;
    if big_endian {
        // ZeroDroid's Android ARM64 Guest ABI is little-endian.
        return Err(ParseError::InvalidBootHeader);
    }
    let page_size = match (flags >> 1) & 0b11 {
        0 => Arm64KernelPageSize::Unspecified,
        1 => Arm64KernelPageSize::KiB4,
        2 => Arm64KernelPageSize::KiB16,
        3 => Arm64KernelPageSize::KiB64,
        _ => unreachable!(),
    };

    Ok(Arm64KernelImageIndex {
        header: Arm64KernelImageHeader {
            text_offset,
            declared_image_size,
            effective_image_size,
            flags,
            page_size,
            big_endian,
        },
        image: 0..usize::try_from(effective_image_size).map_err(|_| ParseError::IntegerOverflow)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(size: usize, declared_size: u64, flags: u64) -> Vec<u8> {
        let mut data = vec![0_u8; size];
        data[8..16].copy_from_slice(&0x80000_u64.to_le_bytes());
        data[16..24].copy_from_slice(&declared_size.to_le_bytes());
        data[24..32].copy_from_slice(&flags.to_le_bytes());
        data[56..60].copy_from_slice(&ARM64_IMAGE_MAGIC.to_le_bytes());
        data
    }

    fn budget() -> ParseBudget {
        ParseBudget { max_input_bytes: 1 << 20, max_items: 128, max_nesting_depth: 8 }
    }

    #[test]
    fn indexes_aarch64_image_and_page_size_flags() {
        let data = image(8192, 8192, 2);
        let parsed = parse_arm64_linux_image(&data, budget(), 16384).unwrap();
        assert_eq!(parsed.header.text_offset, 0x80000);
        assert_eq!(parsed.header.page_size, Arm64KernelPageSize::KiB4);
        assert_eq!(parsed.image, 0..8192);
    }

    #[test]
    fn accepts_unspecified_image_size_using_bounded_input_length() {
        let data = image(4096, 0, 0);
        let parsed = parse_arm64_linux_image(&data, budget(), 4096).unwrap();
        assert_eq!(parsed.header.declared_image_size, 0);
        assert_eq!(parsed.header.effective_image_size, 4096);
    }

    #[test]
    fn rejects_wrong_magic_big_endian_and_invalid_sizes() {
        let mut data = image(4096, 4096, 0);
        data[56] = 0;
        assert_eq!(parse_arm64_linux_image(&data, budget(), 8192), Err(ParseError::InvalidMagic));
        let data = image(4096, 4096, 1);
        assert_eq!(parse_arm64_linux_image(&data, budget(), 8192), Err(ParseError::InvalidBootHeader));
        let data = image(4096, 8192, 0);
        assert_eq!(parse_arm64_linux_image(&data, budget(), 8192), Err(ParseError::InvalidBootHeader));
        let data = image(4096, 4096, 0);
        assert_eq!(parse_arm64_linux_image(&data, budget(), 2048), Err(ParseError::InvalidBootHeader));
    }

    #[test]
    fn enforces_input_budget_and_minimum_header_size() {
        let data = image(4096, 4096, 0);
        let mut limited = budget();
        limited.max_input_bytes = 1024;
        assert_eq!(parse_arm64_linux_image(&data, limited, 8192), Err(ParseError::ResourceLimit));
        assert_eq!(parse_arm64_linux_image(&data[..63], budget(), 8192), Err(ParseError::OutOfBounds));
    }
}
