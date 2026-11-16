#![forbid(unsafe_code)]

//! Bounded, sequential image-to-Guest-memory loading primitives.
//!
//! This crate does not allocate Guest RAM or map host pages. A platform
//! backend supplies the source reader and GPA writer. Loading is chunked and
//! reports partial progress because generic backend writes cannot be rolled back.

use zerodroid_vm_spec_core::{GuestMachineSpec, GuestBootPlan};

pub const DEFAULT_CHUNK_BYTES: usize = 1024 * 1024;
pub const MAX_CHUNK_BYTES: usize = 4 * 1024 * 1024;
pub const DEFAULT_TRANSFER_BUDGET_BYTES: u64 = 100 * 1024 * 1024;
pub const HARD_MAX_TRANSFER_BUDGET_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageReadError { OutOfBounds, BackendFailure }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuestMemoryWriteError { UnmappedGuestAddress, BackendFailure }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuestLoadError {
    InvalidConfiguration,
    SourceSizeMismatch,
    InvalidBootPlan,
    TransferBudgetExceeded,
    SourceReadFailed(ImageReadError),
    GuestWriteFailed(GuestMemoryWriteError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuestLoadConfig {
    pub chunk_bytes: usize,
    pub max_transfer_bytes: u64,
}

impl Default for GuestLoadConfig {
    fn default() -> Self {
        Self { chunk_bytes: DEFAULT_CHUNK_BYTES, max_transfer_bytes: DEFAULT_TRANSFER_BUDGET_BYTES }
    }
}

impl GuestLoadConfig {
    pub fn validate(self) -> Result<Self, GuestLoadError> {
        if self.chunk_bytes == 0 || self.chunk_bytes > MAX_CHUNK_BYTES
            || self.max_transfer_bytes == 0
            || self.max_transfer_bytes > HARD_MAX_TRANSFER_BUDGET_BYTES
        {
            return Err(GuestLoadError::InvalidConfiguration);
        }
        Ok(self)
    }
}

pub trait ImageSource {
    fn len(&self) -> Result<u64, ImageReadError>;
    fn read_exact_at(&mut self, offset: u64, output: &mut [u8]) -> Result<(), ImageReadError>;
}

pub trait GuestMemoryWriter {
    fn write_exact_at(&mut self, guest_phys_addr: u64, data: &[u8])
        -> Result<(), GuestMemoryWriteError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GuestLoadProgress {
    pub completed_bytes: u64,
    pub completed_segments: usize,
}

pub fn load_boot_plan<S, M>(
    source: &mut S,
    memory: &mut M,
    machine: &GuestMachineSpec,
    plan: &GuestBootPlan,
    config: GuestLoadConfig,
) -> Result<GuestLoadProgress, GuestLoadError>
where
    S: ImageSource,
    M: GuestMemoryWriter,
{
    let config = config.validate()?;
    plan.validate(machine).map_err(|_| GuestLoadError::InvalidBootPlan)?;
    if source.len().map_err(GuestLoadError::SourceReadFailed)? != plan.source_image_bytes {
        return Err(GuestLoadError::SourceSizeMismatch);
    }

    let mut total = 0_u64;
    for segment in &plan.segments {
        total = total.checked_add(segment.source_length)
            .ok_or(GuestLoadError::TransferBudgetExceeded)?;
    }
    if total > config.max_transfer_bytes {
        return Err(GuestLoadError::TransferBudgetExceeded);
    }

    let mut buffer = vec![0_u8; config.chunk_bytes];
    let mut progress = GuestLoadProgress::default();
    for segment in &plan.segments {
        let mut offset = 0_u64;
        while offset < segment.source_length {
            let count = usize::try_from(
                (segment.source_length - offset).min(config.chunk_bytes as u64)
            ).map_err(|_| GuestLoadError::InvalidConfiguration)?;
            let source_offset = segment.source_offset.checked_add(offset)
                .ok_or(GuestLoadError::InvalidBootPlan)?;
            let guest_address = segment.guest_phys_addr.checked_add(offset)
                .ok_or(GuestLoadError::InvalidBootPlan)?;
            source.read_exact_at(source_offset, &mut buffer[..count])
                .map_err(GuestLoadError::SourceReadFailed)?;
            memory.write_exact_at(guest_address, &buffer[..count])
                .map_err(GuestLoadError::GuestWriteFailed)?;
            offset += count as u64;
            progress.completed_bytes += count as u64;
        }
        progress.completed_segments += 1;
    }
    debug_assert_eq!(progress.completed_bytes, total);
    Ok(progress)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerodroid_vm_spec_core::{
        Arm64BootRegisters, BootSegment, BootSegmentKind, GuestArchitecture,
        GuestMemoryRegion, MemoryRegionKind, PAGE_SIZE_4K,
    };

    struct VecSource(Vec<u8>);
    impl ImageSource for VecSource {
        fn len(&self) -> Result<u64, ImageReadError> { Ok(self.0.len() as u64) }
        fn read_exact_at(&mut self, offset: u64, output: &mut [u8]) -> Result<(), ImageReadError> {
            let start = usize::try_from(offset).map_err(|_| ImageReadError::OutOfBounds)?;
            let end = start.checked_add(output.len()).ok_or(ImageReadError::OutOfBounds)?;
            output.copy_from_slice(self.0.get(start..end).ok_or(ImageReadError::OutOfBounds)?);
            Ok(())
        }
    }

    #[derive(Default)]
    struct VecGuestMemory { base: u64, bytes: Vec<u8>, writes: Vec<usize> }
    impl GuestMemoryWriter for VecGuestMemory {
        fn write_exact_at(&mut self, address: u64, data: &[u8]) -> Result<(), GuestMemoryWriteError> {
            let start = usize::try_from(address.checked_sub(self.base)
                .ok_or(GuestMemoryWriteError::UnmappedGuestAddress)?)
                .map_err(|_| GuestMemoryWriteError::UnmappedGuestAddress)?;
            let end = start.checked_add(data.len()).ok_or(GuestMemoryWriteError::UnmappedGuestAddress)?;
            self.bytes.get_mut(start..end)
                .ok_or(GuestMemoryWriteError::UnmappedGuestAddress)?.copy_from_slice(data);
            self.writes.push(data.len());
            Ok(())
        }
    }

    fn machine() -> GuestMachineSpec {
        GuestMachineSpec {
            architecture: GuestArchitecture::Aarch64, vcpu_count: 2,
            memory_bytes: 64 * 1024, page_size: PAGE_SIZE_4K,
            memory_regions: vec![GuestMemoryRegion {
                guest_phys_addr: 0x4000_0000, size_bytes: 64 * 1024, kind: MemoryRegionKind::Ram,
            }],
        }
    }

    fn plan() -> GuestBootPlan {
        GuestBootPlan {
            source_image_bytes: 8192, kernel_entry_gpa: 0x4000_0000, dtb_gpa: 0x4000_3000,
            segments: vec![
                BootSegment { source_offset: 0, source_length: 8192, guest_phys_addr: 0x4000_0000, kind: BootSegmentKind::Kernel },
                BootSegment { source_offset: 4096, source_length: 4096, guest_phys_addr: 0x4000_3000, kind: BootSegmentKind::DeviceTree },
            ],
            registers: Arm64BootRegisters {
                x0_dtb_gpa: 0x4000_3000, x1: 0, x2: 0, x3: 0, program_counter: 0x4000_0000,
            },
        }
    }

    #[test]
    fn loads_in_bounded_chunks() {
        let data = (0..8192).map(|v| (v % 251) as u8).collect::<Vec<_>>();
        let mut source = VecSource(data.clone());
        let mut memory = VecGuestMemory { base: 0x4000_0000, bytes: vec![0; 64 * 1024], writes: vec![] };
        let result = load_boot_plan(&mut source, &mut memory, &machine(), &plan(),
            GuestLoadConfig { chunk_bytes: 1024, max_transfer_bytes: 16 * 1024 }).unwrap();
        assert_eq!(result.completed_bytes, 12 * 1024);
        assert_eq!(result.completed_segments, 2);
        assert!(memory.writes.iter().all(|size| *size <= 1024));
        assert_eq!(&memory.bytes[..8192], &data);
    }

    #[test]
    fn rejects_budget_before_backend_writes() {
        let mut source = VecSource(vec![0; 8192]);
        let mut memory = VecGuestMemory { base: 0x4000_0000, bytes: vec![0; 64 * 1024], writes: vec![] };
        let result = load_boot_plan(&mut source, &mut memory, &machine(), &plan(),
            GuestLoadConfig { chunk_bytes: 1024, max_transfer_bytes: 4096 });
        assert_eq!(result, Err(GuestLoadError::TransferBudgetExceeded));
        assert!(memory.writes.is_empty());
    }

    #[test]
    fn rejects_source_size_mismatch_before_writes() {
        let mut source = VecSource(vec![0; 4096]);
        let mut memory = VecGuestMemory { base: 0x4000_0000, bytes: vec![0; 64 * 1024], writes: vec![] };
        let result = load_boot_plan(&mut source, &mut memory, &machine(), &plan(), GuestLoadConfig::default());
        assert_eq!(result, Err(GuestLoadError::SourceSizeMismatch));
        assert!(memory.writes.is_empty());
    }

    #[test]
    fn rejects_chunks_above_hard_limit() {
        assert_eq!(GuestLoadConfig { chunk_bytes: MAX_CHUNK_BYTES + 1, max_transfer_bytes: 4096 }.validate(),
            Err(GuestLoadError::InvalidConfiguration));
    }
}
