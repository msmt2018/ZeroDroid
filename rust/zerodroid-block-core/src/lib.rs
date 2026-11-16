#![forbid(unsafe_code)]

use core::fmt;

pub const MIN_LOGICAL_BLOCK_SIZE: u32 = 512;
pub const MAX_LOGICAL_BLOCK_SIZE: u32 = 65_536;
pub const HARD_MAX_TRANSFER_BYTES: u32 = 16 * 1024 * 1024;
pub const HARD_MAX_QUEUE_DEPTH: u16 = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VmId(pub u128);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockGeometry {
    pub capacity_blocks: u64,
    pub logical_block_size: u32,
    pub max_transfer_bytes: u32,
    pub queue_depth: u16,
    pub read_only: bool,
    pub discard_supported: bool,
    pub discard_granularity_blocks: u32,
}

impl BlockGeometry {
    pub fn validate(self) -> Result<Self, BlockError> {
        if self.capacity_blocks == 0
            || !(MIN_LOGICAL_BLOCK_SIZE..=MAX_LOGICAL_BLOCK_SIZE)
                .contains(&self.logical_block_size)
            || !self.logical_block_size.is_power_of_two()
            || self.max_transfer_bytes == 0
            || self.max_transfer_bytes > HARD_MAX_TRANSFER_BYTES
            || self.max_transfer_bytes < self.logical_block_size
            || self.max_transfer_bytes % self.logical_block_size != 0
            || self.queue_depth == 0
            || self.queue_depth > HARD_MAX_QUEUE_DEPTH
        {
            return Err(BlockError::InvalidGeometry);
        }
        if self.discard_supported
            && (self.discard_granularity_blocks == 0
                || !self.discard_granularity_blocks.is_power_of_two())
        {
            return Err(BlockError::InvalidDiscardGeometry);
        }
        Ok(self)
    }

    pub fn capacity_bytes(self) -> Result<u64, BlockError> {
        self.capacity_blocks
            .checked_mul(u64::from(self.logical_block_size))
            .ok_or(BlockError::CapacityOverflow)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockError {
    InvalidGeometry,
    InvalidDiscardGeometry,
    CapacityOverflow,
    StaleVmGeneration,
    ZeroLengthRequest,
    UnalignedBuffer,
    TransferTooLarge,
    OutOfRange,
    ReadOnly,
    DiscardUnsupported,
    DiscardMisaligned,
    BackendFailure,
}

impl fmt::Display for BlockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

/// Backend contract. Implementations must bind all operations to the VM and
/// generation, serialize overlapping writes, preserve write ordering around
/// flush, and never resolve guest LBAs to host physical partitions.
pub trait BlockBackend {
    fn check_generation(&self, vm: VmId, generation: u64) -> Result<(), BlockError>;
    fn read_at(
        &mut self,
        vm: VmId,
        generation: u64,
        byte_offset: u64,
        output: &mut [u8],
    ) -> Result<(), BlockError>;
    fn write_at(
        &mut self,
        vm: VmId,
        generation: u64,
        byte_offset: u64,
        input: &[u8],
    ) -> Result<(), BlockError>;
    fn flush(&mut self, vm: VmId, generation: u64) -> Result<(), BlockError>;
    fn discard(
        &mut self,
        vm: VmId,
        generation: u64,
        byte_offset: u64,
        byte_length: u64,
    ) -> Result<(), BlockError>;
}

pub struct GuestBlockDevice<B: BlockBackend> {
    backend: B,
    vm: VmId,
    generation: u64,
    geometry: BlockGeometry,
}

impl<B: BlockBackend> GuestBlockDevice<B> {
    pub fn new(
        backend: B,
        vm: VmId,
        generation: u64,
        geometry: BlockGeometry,
    ) -> Result<Self, BlockError> {
        let geometry = geometry.validate()?;
        geometry.capacity_bytes()?;
        Ok(Self { backend, vm, generation, geometry })
    }

    pub fn geometry(&self) -> BlockGeometry {
        self.geometry
    }

    pub fn read_blocks(&mut self, start_lba: u64, output: &mut [u8]) -> Result<(), BlockError> {
        let (offset, length) = self.validate_range(start_lba, output.len())?;
        self.backend.check_generation(self.vm, self.generation)?;
        self.backend.read_at(self.vm, self.generation, offset, output)
    }

    pub fn write_blocks(&mut self, start_lba: u64, input: &[u8]) -> Result<(), BlockError> {
        if self.geometry.read_only {
            return Err(BlockError::ReadOnly);
        }
        let (offset, _) = self.validate_range(start_lba, input.len())?;
        self.backend.check_generation(self.vm, self.generation)?;
        self.backend.write_at(self.vm, self.generation, offset, input)
    }

    pub fn flush(&mut self) -> Result<(), BlockError> {
        self.backend.check_generation(self.vm, self.generation)?;
        self.backend.flush(self.vm, self.generation)
    }

    pub fn discard_blocks(&mut self, start_lba: u64, block_count: u64) -> Result<(), BlockError> {
        if self.geometry.read_only {
            return Err(BlockError::ReadOnly);
        }
        if !self.geometry.discard_supported {
            return Err(BlockError::DiscardUnsupported);
        }
        if block_count == 0
            || start_lba % u64::from(self.geometry.discard_granularity_blocks) != 0
            || block_count % u64::from(self.geometry.discard_granularity_blocks) != 0
        {
            return Err(BlockError::DiscardMisaligned);
        }
        let end = start_lba.checked_add(block_count).ok_or(BlockError::OutOfRange)?;
        if end > self.geometry.capacity_blocks {
            return Err(BlockError::OutOfRange);
        }
        let block_size = u64::from(self.geometry.logical_block_size);
        let offset = start_lba.checked_mul(block_size).ok_or(BlockError::OutOfRange)?;
        let length = block_count.checked_mul(block_size).ok_or(BlockError::OutOfRange)?;
        if length > u64::from(self.geometry.max_transfer_bytes) {
            return Err(BlockError::TransferTooLarge);
        }
        self.backend.check_generation(self.vm, self.generation)?;
        self.backend.discard(self.vm, self.generation, offset, length)
    }

    fn validate_range(&self, start_lba: u64, byte_len: usize) -> Result<(u64, u64), BlockError> {
        if byte_len == 0 {
            return Err(BlockError::ZeroLengthRequest);
        }
        let block_size = usize::try_from(self.geometry.logical_block_size)
            .map_err(|_| BlockError::InvalidGeometry)?;
        if byte_len % block_size != 0 {
            return Err(BlockError::UnalignedBuffer);
        }
        if byte_len > self.geometry.max_transfer_bytes as usize {
            return Err(BlockError::TransferTooLarge);
        }
        let blocks = u64::try_from(byte_len / block_size).map_err(|_| BlockError::OutOfRange)?;
        let end = start_lba.checked_add(blocks).ok_or(BlockError::OutOfRange)?;
        if end > self.geometry.capacity_blocks {
            return Err(BlockError::OutOfRange);
        }
        let offset = start_lba
            .checked_mul(u64::from(self.geometry.logical_block_size))
            .ok_or(BlockError::OutOfRange)?;
        Ok((offset, byte_len as u64))
    }

    pub fn into_backend(self) -> B {
        self.backend
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct MemoryBackend {
        generation: u64,
        bytes: Vec<u8>,
        flushes: u32,
        discards: Vec<(u64, u64)>,
    }

    impl BlockBackend for MemoryBackend {
        fn check_generation(&self, _vm: VmId, generation: u64) -> Result<(), BlockError> {
            if generation == self.generation { Ok(()) } else { Err(BlockError::StaleVmGeneration) }
        }
        fn read_at(&mut self, _vm: VmId, _generation: u64, offset: u64, output: &mut [u8]) -> Result<(), BlockError> {
            let start = usize::try_from(offset).map_err(|_| BlockError::OutOfRange)?;
            let end = start.checked_add(output.len()).ok_or(BlockError::OutOfRange)?;
            output.copy_from_slice(self.bytes.get(start..end).ok_or(BlockError::OutOfRange)?);
            Ok(())
        }
        fn write_at(&mut self, _vm: VmId, _generation: u64, offset: u64, input: &[u8]) -> Result<(), BlockError> {
            let start = usize::try_from(offset).map_err(|_| BlockError::OutOfRange)?;
            let end = start.checked_add(input.len()).ok_or(BlockError::OutOfRange)?;
            self.bytes.get_mut(start..end).ok_or(BlockError::OutOfRange)?.copy_from_slice(input);
            Ok(())
        }
        fn flush(&mut self, _vm: VmId, _generation: u64) -> Result<(), BlockError> {
            self.flushes += 1;
            Ok(())
        }
        fn discard(&mut self, _vm: VmId, _generation: u64, offset: u64, length: u64) -> Result<(), BlockError> {
            self.discards.push((offset, length));
            let start = usize::try_from(offset).map_err(|_| BlockError::OutOfRange)?;
            let end = usize::try_from(offset.checked_add(length).ok_or(BlockError::OutOfRange)?).map_err(|_| BlockError::OutOfRange)?;
            self.bytes.get_mut(start..end).ok_or(BlockError::OutOfRange)?.fill(0);
            Ok(())
        }
    }

    fn geometry() -> BlockGeometry {
        BlockGeometry {
            capacity_blocks: 32,
            logical_block_size: 512,
            max_transfer_bytes: 4096,
            queue_depth: 16,
            read_only: false,
            discard_supported: true,
            discard_granularity_blocks: 1,
        }
    }

    fn device() -> GuestBlockDevice<MemoryBackend> {
        let mut backend = MemoryBackend { generation: 5, bytes: vec![0; 32 * 512], ..Default::default() };
        backend.bytes[0..512].fill(0x11);
        GuestBlockDevice::new(backend, VmId(77), 5, geometry()).unwrap()
    }

    #[test]
    fn reads_and_writes_guest_lba_ranges() {
        let mut device = device();
        device.write_blocks(2, &[0x5a; 512]).unwrap();
        let mut output = [0; 512];
        device.read_blocks(2, &mut output).unwrap();
        assert_eq!(output, [0x5a; 512]);
    }

    #[test]
    fn rejects_empty_unaligned_oversized_and_out_of_range_io() {
        let mut device = device();
        assert_eq!(device.read_blocks(0, &mut []), Err(BlockError::ZeroLengthRequest));
        assert_eq!(device.write_blocks(0, &[1; 513]), Err(BlockError::UnalignedBuffer));
        assert_eq!(device.write_blocks(0, &[1; 4097]), Err(BlockError::TransferTooLarge));
        assert_eq!(device.write_blocks(32, &[1; 512]), Err(BlockError::OutOfRange));
    }

    #[test]
    fn enforces_read_only_and_generation() {
        let mut g = geometry();
        g.read_only = true;
        let backend = MemoryBackend { generation: 5, bytes: vec![0; 32 * 512], ..Default::default() };
        let mut device = GuestBlockDevice::new(backend, VmId(1), 5, g).unwrap();
        assert_eq!(device.write_blocks(0, &[1; 512]), Err(BlockError::ReadOnly));
        assert_eq!(device.discard_blocks(0, 1), Err(BlockError::ReadOnly));

        let backend = MemoryBackend { generation: 4, bytes: vec![0; 32 * 512], ..Default::default() };
        let mut stale = GuestBlockDevice::new(backend, VmId(1), 5, geometry()).unwrap();
        assert_eq!(stale.flush(), Err(BlockError::StaleVmGeneration));
    }

    #[test]
    fn flush_and_discard_are_explicit_backend_operations() {
        let mut device = device();
        device.flush().unwrap();
        device.discard_blocks(4, 2).unwrap();
        let backend = device.into_backend();
        assert_eq!(backend.flushes, 1);
        assert_eq!(backend.discards, vec![(2048, 1024)]);
        assert_eq!(&backend.bytes[2048..3072], &[0; 1024]);
    }

    #[test]
    fn discard_alignment_and_geometry_are_checked() {
        let mut g = geometry();
        g.discard_granularity_blocks = 4;
        let backend = MemoryBackend { generation: 5, bytes: vec![0; 32 * 512], ..Default::default() };
        let mut device = GuestBlockDevice::new(backend, VmId(1), 5, g).unwrap();
        assert_eq!(device.discard_blocks(1, 4), Err(BlockError::DiscardMisaligned));
        assert_eq!(device.discard_blocks(4, 4), Ok(()));
        g.logical_block_size = 1000;
        assert_eq!(GuestBlockDevice::new(MemoryBackend::default(), VmId(1), 5, g).err(), Some(BlockError::InvalidGeometry));
    }
}
