#![deny(unsafe_op_in_unsafe_fn)]

//! Narrow C ABI for the Android Native runtime.
//! All parsers remain in safe Rust crates; this crate owns only pointer/slice conversion.

use core::slice;
use zerodroid_image_core::arm64_kernel::parse_arm64_linux_image;
use zerodroid_image_core::legacy_boot::parse_legacy_boot_image;
use zerodroid_image_core::{ParseBudget, ParseError};

pub const ZDROID_RUST_OK: u32 = 0;
pub const ZDROID_RUST_INVALID_ARGUMENT: u32 = 1;
pub const ZDROID_RUST_RESOURCE_LIMIT: u32 = 2;
pub const ZDROID_RUST_INVALID_IMAGE: u32 = 3;
pub const ZDROID_RUST_UNSUPPORTED: u32 = 4;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ZdroidArm64KernelImageInfo {
    pub struct_size: u32,
    pub page_size_code: u32,
    pub text_offset: u64,
    pub declared_image_size: u64,
    pub effective_image_size: u64,
    pub flags: u64,
    pub image_offset: u64,
    pub image_length: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ZdroidLegacyBootImageInfo {
    pub struct_size: u32,
    pub header_version: u32,
    pub header_size: u32,
    pub page_size: u32,
    pub kernel_offset: u64,
    pub kernel_size: u64,
    pub ramdisk_offset: u64,
    pub ramdisk_size: u64,
    pub second_offset: u64,
    pub second_size: u64,
    pub recovery_dtbo_offset: u64,
    pub recovery_dtbo_size: u64,
    pub dtb_offset: u64,
    pub dtb_size: u64,
    pub recovery_dtbo_present: u32,
    pub dtb_present: u32,
    pub dtb_address: u64,
    pub total_indexed_bytes: u64,
}

fn parse_status(error: ParseError) -> u32 {
    match error {
        ParseError::ResourceLimit => ZDROID_RUST_RESOURCE_LIMIT,
        ParseError::UnsupportedVersion | ParseError::UnsupportedBootHeaderVersion => ZDROID_RUST_UNSUPPORTED,
        _ => ZDROID_RUST_INVALID_IMAGE,
    }
}

fn make_budget(max_input_bytes: u64) -> ParseBudget {
    ParseBudget { max_input_bytes, max_items: 65_536, max_nesting_depth: 16 }
}

/// # Safety
/// data must reference length readable bytes and out_info must reference a writable
/// ZdroidArm64KernelImageInfo for the duration of this call.
#[no_mangle]
pub unsafe extern "C" fn zdroid_rust_parse_arm64_kernel_image(
    data: *const u8, length: usize, max_input_bytes: u64, max_image_bytes: u64,
    out_info: *mut ZdroidArm64KernelImageInfo,
) -> u32 {
    if data.is_null() || out_info.is_null() || length == 0 || length > isize::MAX as usize {
        return ZDROID_RUST_INVALID_ARGUMENT;
    }
    // SAFETY: The caller contract requires a writable output pointer.
    let output = unsafe { &mut *out_info };
    if (output.struct_size as usize) < core::mem::size_of::<ZdroidArm64KernelImageInfo>() {
        return ZDROID_RUST_INVALID_ARGUMENT;
    }
    if length as u64 > max_input_bytes { return ZDROID_RUST_RESOURCE_LIMIT; }
    // SAFETY: Input pointer validity and lifetime are part of this C ABI contract.
    let input = unsafe { slice::from_raw_parts(data, length) };
    match parse_arm64_linux_image(input, make_budget(max_input_bytes), max_image_bytes) {
        Ok(index) => {
            let page_size_code = match index.header.page_size {
                zerodroid_image_core::arm64_kernel::Arm64KernelPageSize::Unspecified => 0,
                zerodroid_image_core::arm64_kernel::Arm64KernelPageSize::KiB4 => 1,
                zerodroid_image_core::arm64_kernel::Arm64KernelPageSize::KiB16 => 2,
                zerodroid_image_core::arm64_kernel::Arm64KernelPageSize::KiB64 => 3,
            };
            *output = ZdroidArm64KernelImageInfo {
                struct_size: core::mem::size_of::<ZdroidArm64KernelImageInfo>() as u32,
                page_size_code, text_offset: index.header.text_offset,
                declared_image_size: index.header.declared_image_size,
                effective_image_size: index.header.effective_image_size, flags: index.header.flags,
                image_offset: index.image.start as u64,
                image_length: (index.image.end - index.image.start) as u64,
            };
            ZDROID_RUST_OK
        }
        Err(error) => parse_status(error),
    }
}

/// # Safety
/// data must reference length readable bytes and out_info must reference a writable
/// ZdroidLegacyBootImageInfo for the duration of this call.
#[no_mangle]
pub unsafe extern "C" fn zdroid_rust_parse_legacy_boot_image(
    data: *const u8, length: usize, max_input_bytes: u64, max_payload_bytes: u64,
    out_info: *mut ZdroidLegacyBootImageInfo,
) -> u32 {
    if data.is_null() || out_info.is_null() || length == 0 || length > isize::MAX as usize {
        return ZDROID_RUST_INVALID_ARGUMENT;
    }
    // SAFETY: The caller contract requires a writable output pointer.
    let output = unsafe { &mut *out_info };
    if (output.struct_size as usize) < core::mem::size_of::<ZdroidLegacyBootImageInfo>() {
        return ZDROID_RUST_INVALID_ARGUMENT;
    }
    if length as u64 > max_input_bytes { return ZDROID_RUST_RESOURCE_LIMIT; }
    // SAFETY: Input pointer validity and lifetime are part of this C ABI contract.
    let input = unsafe { slice::from_raw_parts(data, length) };
    match parse_legacy_boot_image(input, make_budget(max_input_bytes), max_payload_bytes) {
        Ok(index) => {
            let (second_offset, second_size) = index.second.as_ref()
                .map(|r| (r.start as u64, (r.end - r.start) as u64)).unwrap_or((0, 0));
            let (recovery_offset, recovery_size, recovery_present) = index.recovery_dtbo.as_ref()
                .map(|r| (r.start as u64, (r.end - r.start) as u64, 1)).unwrap_or((0, 0, 0));
            let (dtb_offset, dtb_size, dtb_present) = index.dtb.as_ref()
                .map(|r| (r.start as u64, (r.end - r.start) as u64, 1)).unwrap_or((0, 0, 0));
            *output = ZdroidLegacyBootImageInfo {
                struct_size: core::mem::size_of::<ZdroidLegacyBootImageInfo>() as u32,
                header_version: index.header.header_version, header_size: index.header.header_size,
                page_size: index.header.page_size,
                kernel_offset: index.kernel.start as u64, kernel_size: (index.kernel.end-index.kernel.start) as u64,
                ramdisk_offset: index.ramdisk.start as u64, ramdisk_size: (index.ramdisk.end-index.ramdisk.start) as u64,
                second_offset, second_size, recovery_dtbo_offset: recovery_offset, recovery_dtbo_size: recovery_size,
                dtb_offset, dtb_size, recovery_dtbo_present: recovery_present, dtb_present,
                dtb_address: index.header.dtb_address, total_indexed_bytes: index.total_indexed_bytes,
            };
            ZDROID_RUST_OK
        }
        Err(error) => parse_status(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ffi_info_structs_have_stable_nonzero_layout() {
        assert!(core::mem::size_of::<ZdroidArm64KernelImageInfo>() >= 48);
        assert!(core::mem::size_of::<ZdroidLegacyBootImageInfo>() >= 112);
    }
    #[test]
    fn parse_status_keeps_resource_and_format_failures_distinct() {
        assert_eq!(parse_status(ParseError::ResourceLimit), ZDROID_RUST_RESOURCE_LIMIT);
        assert_eq!(parse_status(ParseError::InvalidMagic), ZDROID_RUST_INVALID_IMAGE);
        assert_eq!(parse_status(ParseError::UnsupportedBootHeaderVersion), ZDROID_RUST_UNSUPPORTED);
    }
}
