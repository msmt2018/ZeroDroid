//! Validated ARM64 Linux direct-boot plan.
//!
//! The plan describes borrowed image ranges and destination GPAs. It performs no
//! image copying, host allocation, KVM ioctl, or vCPU execution.

use crate::{GuestMachineSpec, MemoryRegionKind, SpecError, PAGE_SIZE_4K};

pub const HARD_MAX_BOOT_SEGMENTS: usize = 16;
pub const ARM64_LINUX_ENTRY_ALIGNMENT: u64 = 4;
pub const ARM64_DTB_ALIGNMENT: u64 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootSegmentKind {
    Kernel,
    Initramfs,
    DeviceTree,
    Firmware,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootSegment {
    /// Byte range in the original image/container.
    pub source_offset: u64,
    pub source_length: u64,
    /// Guest physical destination.
    pub guest_phys_addr: u64,
    pub kind: BootSegmentKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Arm64BootRegisters {
    /// Linux arm64 boot protocol: x0 points to the DTB; x1-x3 are zero.
    pub x0_dtb_gpa: u64,
    pub x1: u64,
    pub x2: u64,
    pub x3: u64,
    pub program_counter: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestBootPlan {
    pub source_image_bytes: u64,
    pub kernel_entry_gpa: u64,
    pub dtb_gpa: u64,
    pub segments: Vec<BootSegment>,
    pub registers: Arm64BootRegisters,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootPlanError {
    InvalidMachineSpec(SpecError),
    InvalidSourceImageSize,
    TooManySegments,
    MissingKernel,
    MissingDeviceTree,
    DuplicateKernel,
    DuplicateDeviceTree,
    EmptySegment,
    SourceRangeOverflow,
    SourceRangeOutOfBounds,
    GuestAddressOverflow,
    SegmentOutsideGuestRam,
    SegmentOverlapsAnother,
    InvalidKernelEntry,
    InvalidDtbAddress,
    InvalidBootRegisters,
}

impl GuestBootPlan {
    /// Validates a complete direct-kernel boot plan against the machine RAM map.
    /// Segment bytes remain in the source image and are not copied by this operation.
    pub fn validate(&self, machine: &GuestMachineSpec) -> Result<(), BootPlanError> {
        machine.validate().map_err(BootPlanError::InvalidMachineSpec)?;
        if self.source_image_bytes == 0 {
            return Err(BootPlanError::InvalidSourceImageSize);
        }
        if self.segments.is_empty() || self.segments.len() > HARD_MAX_BOOT_SEGMENTS {
            return Err(BootPlanError::TooManySegments);
        }

        let mut kernel: Option<BootSegment> = None;
        let mut dtb: Option<BootSegment> = None;
        let mut ranges: Vec<(u64, u64)> = Vec::with_capacity(self.segments.len());

        for segment in &self.segments {
            if segment.source_length == 0 {
                return Err(BootPlanError::EmptySegment);
            }
            let source_end = segment.source_offset
                .checked_add(segment.source_length)
                .ok_or(BootPlanError::SourceRangeOverflow)?;
            if source_end > self.source_image_bytes {
                return Err(BootPlanError::SourceRangeOutOfBounds);
            }

            let guest_end = segment.guest_phys_addr
                .checked_add(segment.source_length)
                .ok_or(BootPlanError::GuestAddressOverflow)?;
            let covered_by_ram = machine.memory_regions.iter().any(|region| {
                if region.kind != MemoryRegionKind::Ram { return false; }
                let Some(region_end) = region.guest_phys_addr.checked_add(region.size_bytes) else {
                    return false;
                };
                segment.guest_phys_addr >= region.guest_phys_addr && guest_end <= region_end
            });
            if !covered_by_ram {
                return Err(BootPlanError::SegmentOutsideGuestRam);
            }

            if ranges.iter().any(|(start, end)| segment.guest_phys_addr < *end && *start < guest_end) {
                return Err(BootPlanError::SegmentOverlapsAnother);
            }
            ranges.push((segment.guest_phys_addr, guest_end));

            match segment.kind {
                BootSegmentKind::Kernel => {
                    if kernel.replace(*segment).is_some() { return Err(BootPlanError::DuplicateKernel); }
                }
                BootSegmentKind::DeviceTree => {
                    if dtb.replace(*segment).is_some() { return Err(BootPlanError::DuplicateDeviceTree); }
                }
                BootSegmentKind::Initramfs | BootSegmentKind::Firmware => {}
            }
        }

        let kernel = kernel.ok_or(BootPlanError::MissingKernel)?;
        let dtb = dtb.ok_or(BootPlanError::MissingDeviceTree)?;
        if self.kernel_entry_gpa < kernel.guest_phys_addr
            || self.kernel_entry_gpa >= kernel.guest_phys_addr + kernel.source_length
            || self.kernel_entry_gpa % ARM64_LINUX_ENTRY_ALIGNMENT != 0
        {
            return Err(BootPlanError::InvalidKernelEntry);
        }
        if self.dtb_gpa != dtb.guest_phys_addr || self.dtb_gpa % ARM64_DTB_ALIGNMENT != 0 {
            return Err(BootPlanError::InvalidDtbAddress);
        }
        if self.registers.x0_dtb_gpa != self.dtb_gpa
            || self.registers.x1 != 0 || self.registers.x2 != 0 || self.registers.x3 != 0
            || self.registers.program_counter != self.kernel_entry_gpa
        {
            return Err(BootPlanError::InvalidBootRegisters);
        }
        let _ = PAGE_SIZE_4K;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GuestArchitecture, GuestMemoryRegion};

    fn machine() -> GuestMachineSpec {
        GuestMachineSpec {
            architecture: GuestArchitecture::Aarch64,
            vcpu_count: 4,
            memory_bytes: 64 * 1024 * 1024,
            page_size: PAGE_SIZE_4K,
            memory_regions: vec![GuestMemoryRegion {
                guest_phys_addr: 0x4000_0000,
                size_bytes: 64 * 1024 * 1024,
                kind: MemoryRegionKind::Ram,
            }],
        }
    }

    fn plan() -> GuestBootPlan {
        GuestBootPlan {
            source_image_bytes: 16 * 1024 * 1024,
            kernel_entry_gpa: 0x4008_0000,
            dtb_gpa: 0x43f0_0000,
            segments: vec![
                BootSegment { source_offset: 4096, source_length: 8 * 1024 * 1024, guest_phys_addr: 0x4008_0000, kind: BootSegmentKind::Kernel },
                BootSegment { source_offset: 8 * 1024 * 1024, source_length: 4 * 1024 * 1024, guest_phys_addr: 0x4100_0000, kind: BootSegmentKind::Initramfs },
                BootSegment { source_offset: 12 * 1024 * 1024, source_length: 64 * 1024, guest_phys_addr: 0x43f0_0000, kind: BootSegmentKind::DeviceTree },
            ],
            registers: Arm64BootRegisters { x0_dtb_gpa: 0x43f0_0000, x1: 0, x2: 0, x3: 0, program_counter: 0x4008_0000 },
        }
    }

    #[test]
    fn accepts_valid_arm64_direct_boot_plan() {
        assert_eq!(plan().validate(&machine()), Ok(()));
    }

    #[test]
    fn rejects_source_range_outside_image() {
        let mut value = plan();
        value.segments[0].source_offset = u64::MAX - 3;
        assert_eq!(value.validate(&machine()), Err(BootPlanError::SourceRangeOverflow));
    }

    #[test]
    fn rejects_guest_destination_outside_ram() {
        let mut value = plan();
        value.segments[0].guest_phys_addr = 0x5000_0000;
        value.kernel_entry_gpa = 0x5000_0000;
        value.registers.program_counter = 0x5000_0000;
        assert_eq!(value.validate(&machine()), Err(BootPlanError::SegmentOutsideGuestRam));
    }

    #[test]
    fn rejects_overlapping_guest_segments() {
        let mut value = plan();
        value.segments[1].guest_phys_addr = 0x4008_1000;
        assert_eq!(value.validate(&machine()), Err(BootPlanError::SegmentOverlapsAnother));
    }

    #[test]
    fn enforces_arm64_register_convention() {
        let mut value = plan();
        value.registers.x1 = 1;
        assert_eq!(value.validate(&machine()), Err(BootPlanError::InvalidBootRegisters));
    }

    #[test]
    fn rejects_entry_outside_kernel_and_unaligned_dtb() {
        let mut value = plan();
        value.kernel_entry_gpa = 0x4200_0000;
        value.registers.program_counter = value.kernel_entry_gpa;
        assert_eq!(value.validate(&machine()), Err(BootPlanError::InvalidKernelEntry));
        let mut value = plan();
        value.dtb_gpa += 1;
        value.registers.x0_dtb_gpa = value.dtb_gpa;
        assert_eq!(value.validate(&machine()), Err(BootPlanError::InvalidDtbAddress));
    }
}
