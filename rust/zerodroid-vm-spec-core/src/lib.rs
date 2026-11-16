#![forbid(unsafe_code)]

pub mod boot_plan;

//! Safe, bounded validation of the ARM64 Guest machine shape.
//!
//! This crate validates data only. It does not open KVM, allocate host memory,
//! create vCPUs, or claim that a host backend is available.

pub const PAGE_SIZE_4K: u64 = 4096;
pub const HARD_MAX_VCPUS: u32 = 64;
pub const HARD_MAX_MEMORY_BYTES: u64 = 64 * 1024 * 1024 * 1024;
pub const HARD_MAX_MEMORY_REGIONS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum GuestArchitecture {
    Aarch64 = 1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryRegionKind {
    Ram,
    Reserved,
    Shared,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuestMemoryRegion {
    pub guest_phys_addr: u64,
    pub size_bytes: u64,
    pub kind: MemoryRegionKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestMachineSpec {
    pub architecture: GuestArchitecture,
    pub vcpu_count: u32,
    pub memory_bytes: u64,
    pub page_size: u64,
    pub memory_regions: Vec<GuestMemoryRegion>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecError {
    InvalidVcpuCount,
    InvalidMemorySize,
    InvalidPageSize,
    InvalidMemoryRegionCount,
    EmptyMemoryRegion,
    UnalignedMemoryRegion,
    AddressOverflow,
    OverlappingMemoryRegions,
    MemoryTotalMismatch,
    NoRamRegion,
}

impl GuestMachineSpec {
    pub fn validate(&self) -> Result<(), SpecError> {
        if self.vcpu_count == 0 || self.vcpu_count > HARD_MAX_VCPUS {
            return Err(SpecError::InvalidVcpuCount);
        }
        if self.memory_bytes < PAGE_SIZE_4K
            || self.memory_bytes > HARD_MAX_MEMORY_BYTES
            || self.memory_bytes % PAGE_SIZE_4K != 0
        {
            return Err(SpecError::InvalidMemorySize);
        }
        if self.page_size != PAGE_SIZE_4K {
            return Err(SpecError::InvalidPageSize);
        }
        if self.memory_regions.is_empty()
            || self.memory_regions.len() > HARD_MAX_MEMORY_REGIONS
        {
            return Err(SpecError::InvalidMemoryRegionCount);
        }

        let mut total_ram = 0_u64;
        let mut has_ram = false;
        for (index, region) in self.memory_regions.iter().enumerate() {
            if region.size_bytes == 0 {
                return Err(SpecError::EmptyMemoryRegion);
            }
            if region.guest_phys_addr % self.page_size != 0
                || region.size_bytes % self.page_size != 0
            {
                return Err(SpecError::UnalignedMemoryRegion);
            }
            let end = region
                .guest_phys_addr
                .checked_add(region.size_bytes)
                .ok_or(SpecError::AddressOverflow)?;

            if region.kind == MemoryRegionKind::Ram {
                has_ram = true;
                total_ram = total_ram
                    .checked_add(region.size_bytes)
                    .ok_or(SpecError::AddressOverflow)?;
            }

            for other in self.memory_regions.iter().take(index) {
                let other_end = other
                    .guest_phys_addr
                    .checked_add(other.size_bytes)
                    .ok_or(SpecError::AddressOverflow)?;
                if region.guest_phys_addr < other_end
                    && other.guest_phys_addr < end
                {
                    return Err(SpecError::OverlappingMemoryRegions);
                }
            }
        }

        if !has_ram {
            return Err(SpecError::NoRamRegion);
        }
        if total_ram != self.memory_bytes {
            return Err(SpecError::MemoryTotalMismatch);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_spec() -> GuestMachineSpec {
        GuestMachineSpec {
            architecture: GuestArchitecture::Aarch64,
            vcpu_count: 4,
            memory_bytes: 1024 * 1024 * 1024,
            page_size: PAGE_SIZE_4K,
            memory_regions: vec![GuestMemoryRegion {
                guest_phys_addr: 0x4000_0000,
                size_bytes: 1024 * 1024 * 1024,
                kind: MemoryRegionKind::Ram,
            }],
        }
    }

    #[test]
    fn accepts_bounded_aligned_arm64_guest_layout() {
        assert_eq!(valid_spec().validate(), Ok(()));
    }

    #[test]
    fn rejects_empty_memory_region_list() {
        let mut spec = valid_spec();
        spec.memory_regions.clear();
        assert_eq!(spec.validate(), Err(SpecError::InvalidMemoryRegionCount));
    }

    #[test]
    fn rejects_vcpu_count_outside_hard_limit() {
        let mut spec = valid_spec();
        spec.vcpu_count = HARD_MAX_VCPUS + 1;
        assert_eq!(spec.validate(), Err(SpecError::InvalidVcpuCount));
    }

    #[test]
    fn rejects_overlapping_guest_physical_ranges() {
        let mut spec = valid_spec();
        spec.memory_regions.push(GuestMemoryRegion {
            guest_phys_addr: 0x5000_0000,
            size_bytes: PAGE_SIZE_4K,
            kind: MemoryRegionKind::Reserved,
        });
        assert_eq!(spec.validate(), Err(SpecError::OverlappingMemoryRegions));
    }

    #[test]
    fn rejects_guest_physical_address_overflow() {
        let mut spec = valid_spec();
        spec.memory_regions[0].guest_phys_addr = u64::MAX - PAGE_SIZE_4K + 1;
        assert_eq!(spec.validate(), Err(SpecError::AddressOverflow));
    }

    #[test]
    fn rejects_ram_total_mismatch() {
        let mut spec = valid_spec();
        spec.memory_regions[0].size_bytes -= PAGE_SIZE_4K;
        assert_eq!(spec.validate(), Err(SpecError::MemoryTotalMismatch));
    }

    #[test]
    fn rejects_non_four_kib_page_size() {
        let mut spec = valid_spec();
        spec.page_size = 16 * 1024;
        assert_eq!(spec.validate(), Err(SpecError::InvalidPageSize));
    }
}
