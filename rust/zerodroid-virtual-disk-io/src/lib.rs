#![forbid(unsafe_code)]

use zerodroid_block_core::{BlockBackend, BlockError, VmId, HARD_MAX_TRANSFER_BYTES};
use zerodroid_dynamic_partition_core::{DynamicPartition, DynamicPartitionError, ExtentTarget};

pub const HARD_MAX_EXTENT_SEGMENTS: usize = 256;

fn map_error(_: DynamicPartitionError) -> BlockError { BlockError::OutOfRange }

/// Storage-facing operations for one VM's virtual super backing objects.
/// Implementations must provide VM/generation isolation, bounded I/O,
/// ordering around flush, and sparse/COW semantics for zero extents.
pub trait VirtualDiskExtentBackend {
    fn check_generation(&self, vm: VmId, generation: u64) -> Result<(), BlockError>;
    fn read_linear(&mut self, vm: VmId, generation: u64, device_index: u32, device_name: &str, byte_offset: u64, output: &mut [u8]) -> Result<(), BlockError>;
    fn write_linear(&mut self, vm: VmId, generation: u64, device_index: u32, device_name: &str, byte_offset: u64, input: &[u8]) -> Result<(), BlockError>;
    /// Read from the sparse zero-extent overlay (zero-filled when no COW page exists).
    fn read_zero(&mut self, vm: VmId, generation: u64, partition: &str, byte_offset: u64, output: &mut [u8]) -> Result<(), BlockError>;
    /// Write into the sparse zero-extent overlay; never discard guest writes.
    fn write_zero(&mut self, vm: VmId, generation: u64, partition: &str, byte_offset: u64, input: &[u8]) -> Result<(), BlockError>;
    fn discard_linear(&mut self, vm: VmId, generation: u64, device_index: u32, device_name: &str, byte_offset: u64, byte_length: u64) -> Result<(), BlockError>;
    fn discard_zero(&mut self, vm: VmId, generation: u64, partition: &str, byte_offset: u64, byte_length: u64) -> Result<(), BlockError>;
    fn flush(&mut self, vm: VmId, generation: u64) -> Result<(), BlockError>;
}

/// Adapts a validated liblp logical partition into the existing GuestBlockDevice
/// backend contract. It routes only within the selected partition's extents.
pub struct MappedPartitionBackend<B: VirtualDiskExtentBackend> {
    backend: B,
    vm: VmId,
    generation: u64,
    partition_name: String,
    partition: DynamicPartition,
    max_segments: usize,
}

impl<B: VirtualDiskExtentBackend> MappedPartitionBackend<B> {
    pub fn new(
        backend: B,
        vm: VmId,
        generation: u64,
        partition_name: impl Into<String>,
        partition: &DynamicPartition,
        max_segments: usize,
    ) -> Result<Self, BlockError> {
        if max_segments == 0 || max_segments > HARD_MAX_EXTENT_SEGMENTS {
            return Err(BlockError::InvalidGeometry);
        }
        Ok(Self {
            backend, vm, generation, partition_name: partition_name.into(),
            partition: partition.clone(), max_segments,
        })
    }

    pub fn into_backend(self) -> B { self.backend }

    fn validate_request(&self, offset: u64, length: usize) -> Result<(), BlockError> {
        if length == 0 { return Err(BlockError::ZeroLengthRequest); }
        if length > HARD_MAX_TRANSFER_BYTES as usize { return Err(BlockError::TransferTooLarge); }
        let end = offset.checked_add(u64::try_from(length).map_err(|_| BlockError::OutOfRange)?)
            .ok_or(BlockError::OutOfRange)?;
        if end > self.partition.logical_size { return Err(BlockError::OutOfRange); }
        Ok(())
    }

    fn mapped(&self, offset: u64, length: usize) -> Result<Vec<zerodroid_dynamic_partition_core::ExtentSegment>, BlockError> {
        self.partition.map_range(offset, length as u64, self.max_segments).map_err(map_error)
    }
}

impl<B: VirtualDiskExtentBackend> BlockBackend for MappedPartitionBackend<B> {
    fn check_generation(&self, vm: VmId, generation: u64) -> Result<(), BlockError> {
        if vm != self.vm || generation != self.generation { return Err(BlockError::StaleVmGeneration); }
        self.backend.check_generation(vm, generation)
    }

    fn read_at(&mut self, vm: VmId, generation: u64, byte_offset: u64, output: &mut [u8]) -> Result<(), BlockError> {
        self.check_generation(vm, generation)?;
        self.validate_request(byte_offset, output.len())?;
        let segments = self.mapped(byte_offset, output.len())?;
        let mut cursor = 0usize;
        for segment in segments {
            let length = usize::try_from(segment.length).map_err(|_| BlockError::OutOfRange)?;
            let end = cursor.checked_add(length).ok_or(BlockError::OutOfRange)?;
            let target = output.get_mut(cursor..end).ok_or(BlockError::OutOfRange)?;
            match segment.target {
                ExtentTarget::Linear { block_device_index, block_device_name, byte_offset } =>
                    self.backend.read_linear(vm, generation, block_device_index, &block_device_name, byte_offset, target)?,
                ExtentTarget::Zero =>
                    self.backend.read_zero(vm, generation, &self.partition_name, segment.logical_offset, target)?,
            }
            cursor = end;
        }
        if cursor != output.len() { return Err(BlockError::BackendFailure); }
        Ok(())
    }

    fn write_at(&mut self, vm: VmId, generation: u64, byte_offset: u64, input: &[u8]) -> Result<(), BlockError> {
        self.check_generation(vm, generation)?;
        self.validate_request(byte_offset, input.len())?;
        let segments = self.mapped(byte_offset, input.len())?;
        let mut cursor = 0usize;
        for segment in segments {
            let length = usize::try_from(segment.length).map_err(|_| BlockError::OutOfRange)?;
            let end = cursor.checked_add(length).ok_or(BlockError::OutOfRange)?;
            let source = input.get(cursor..end).ok_or(BlockError::OutOfRange)?;
            match segment.target {
                ExtentTarget::Linear { block_device_index, block_device_name, byte_offset } =>
                    self.backend.write_linear(vm, generation, block_device_index, &block_device_name, byte_offset, source)?,
                ExtentTarget::Zero =>
                    self.backend.write_zero(vm, generation, &self.partition_name, segment.logical_offset, source)?,
            }
            cursor = end;
        }
        if cursor != input.len() { return Err(BlockError::BackendFailure); }
        Ok(())
    }

    fn flush(&mut self, vm: VmId, generation: u64) -> Result<(), BlockError> {
        self.check_generation(vm, generation)?;
        self.backend.flush(vm, generation)
    }

    fn discard(&mut self, vm: VmId, generation: u64, byte_offset: u64, byte_length: u64) -> Result<(), BlockError> {
        self.check_generation(vm, generation)?;
        let length = usize::try_from(byte_length).map_err(|_| BlockError::TransferTooLarge)?;
        self.validate_request(byte_offset, length)?;
        let segments = self.partition.map_range(byte_offset, byte_length, self.max_segments).map_err(map_error)?;
        for segment in segments {
            match segment.target {
                ExtentTarget::Linear { block_device_index, block_device_name, byte_offset } =>
                    self.backend.discard_linear(vm, generation, block_device_index, &block_device_name, byte_offset, segment.length)?,
                ExtentTarget::Zero =>
                    self.backend.discard_zero(vm, generation, &self.partition_name, segment.logical_offset, segment.length)?,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerodroid_dynamic_partition_core::{DynamicPartitionLayout, DynamicPartition};
    use zerodroid_image_core::lp_metadata::{
        LpBlockDevice, LpExtent, LpGeometry, LpGroup, LpMetadataIndex, LpPartition,
        LP_TARGET_LINEAR, LP_TARGET_ZERO,
    };

    #[derive(Default)]
    struct MemoryExtentBackend {
        generation: u64,
        linear: Vec<u8>,
        zero: Vec<u8>,
        flushes: u32,
    }
    impl VirtualDiskExtentBackend for MemoryExtentBackend {
        fn check_generation(&self, vm: VmId, generation: u64) -> Result<(), BlockError> {
            if vm == VmId(9) && generation == self.generation { Ok(()) } else { Err(BlockError::StaleVmGeneration) }
        }
        fn read_linear(&mut self, _: VmId, _: u64, _: u32, _: &str, offset: u64, out: &mut [u8]) -> Result<(), BlockError> {
            let start = usize::try_from(offset).map_err(|_| BlockError::OutOfRange)?;
            let end = start.checked_add(out.len()).ok_or(BlockError::OutOfRange)?;
            out.copy_from_slice(self.linear.get(start..end).ok_or(BlockError::OutOfRange)?);
            Ok(())
        }
        fn write_linear(&mut self, _: VmId, _: u64, _: u32, _: &str, offset: u64, data: &[u8]) -> Result<(), BlockError> {
            let start = usize::try_from(offset).map_err(|_| BlockError::OutOfRange)?;
            let end = start.checked_add(data.len()).ok_or(BlockError::OutOfRange)?;
            self.linear.get_mut(start..end).ok_or(BlockError::OutOfRange)?.copy_from_slice(data);
            Ok(())
        }
        fn read_zero(&mut self, _: VmId, _: u64, _: &str, offset: u64, out: &mut [u8]) -> Result<(), BlockError> {
            let start = usize::try_from(offset).map_err(|_| BlockError::OutOfRange)?;
            let end = start.checked_add(out.len()).ok_or(BlockError::OutOfRange)?;
            out.copy_from_slice(self.zero.get(start..end).ok_or(BlockError::OutOfRange)?);
            Ok(())
        }
        fn write_zero(&mut self, _: VmId, _: u64, _: &str, offset: u64, data: &[u8]) -> Result<(), BlockError> {
            let start = usize::try_from(offset).map_err(|_| BlockError::OutOfRange)?;
            let end = start.checked_add(data.len()).ok_or(BlockError::OutOfRange)?;
            self.zero.get_mut(start..end).ok_or(BlockError::OutOfRange)?.copy_from_slice(data);
            Ok(())
        }
        fn discard_linear(&mut self, _: VmId, _: u64, _: u32, _: &str, offset: u64, length: u64) -> Result<(), BlockError> {
            let start = usize::try_from(offset).map_err(|_| BlockError::OutOfRange)?;
            let end = usize::try_from(offset.checked_add(length).ok_or(BlockError::OutOfRange)?).map_err(|_| BlockError::OutOfRange)?;
            self.linear.get_mut(start..end).ok_or(BlockError::OutOfRange)?.fill(0);
            Ok(())
        }
        fn discard_zero(&mut self, _: VmId, _: u64, _: &str, offset: u64, length: u64) -> Result<(), BlockError> {
            let start = usize::try_from(offset).map_err(|_| BlockError::OutOfRange)?;
            let end = usize::try_from(offset.checked_add(length).ok_or(BlockError::OutOfRange)?).map_err(|_| BlockError::OutOfRange)?;
            self.zero.get_mut(start..end).ok_or(BlockError::OutOfRange)?.fill(0);
            Ok(())
        }
        fn flush(&mut self, _: VmId, _: u64) -> Result<(), BlockError> { self.flushes += 1; Ok(()) }
    }

    fn partition() -> DynamicPartition {
        let index = LpMetadataIndex {
            geometry: LpGeometry { metadata_max_size: 4096, metadata_slot_count: 2, logical_block_size: 4096, source_range: 4096..4148 },
            slot_number: 0, major_version: 10, minor_version: 2, header_flags: 0,
            header_range: 12288..12416, tables_range: 12416..12500,
            partitions: vec![LpPartition { name: "system_a".into(), attributes: 0, first_extent_index: 0, num_extents: 3, group_index: 0, logical_sectors: 24 }],
            extents: vec![
                LpExtent { num_sectors: 8, target_type: LP_TARGET_LINEAR, target_data: 40, target_source: 0, physical_range_sectors: Some(40..48) },
                LpExtent { num_sectors: 4, target_type: LP_TARGET_ZERO, target_data: 0, target_source: 0, physical_range_sectors: None },
                LpExtent { num_sectors: 12, target_type: LP_TARGET_LINEAR, target_data: 64, target_source: 0, physical_range_sectors: Some(64..76) },
            ],
            groups: vec![LpGroup { name: "default".into(), flags: 0, maximum_size: 0 }],
            block_devices: vec![LpBlockDevice { first_logical_sector: 40, alignment: 4096, alignment_offset: 0, size: 128 * 1024, partition_name: "super".into(), flags: 0 }],
            metadata_copy_range: 12288..16384,
        };
        DynamicPartitionLayout::from_metadata(&index).unwrap().partition("system_a").unwrap().clone()
    }

    #[test]
    fn routes_cross_extent_reads_and_writes_including_zero_cow_overlay() {
        let p = partition();
        let mut backend = MemoryExtentBackend { generation: 3, linear: vec![0x11; 128 * 1024], zero: vec![0; 24 * 512], ..Default::default() };
        let mut adapter = MappedPartitionBackend::new(backend, VmId(9), 3, "system_a", &p, 8).unwrap();
        let mut read = vec![0; 8 * 512];
        adapter.read_at(VmId(9), 3, 6 * 512, &mut read).unwrap();
        assert_eq!(&read[..2 * 512], &vec![0x11; 2 * 512]);
        assert_eq!(&read[2 * 512..6 * 512], &vec![0; 4 * 512]);
        assert_eq!(&read[6 * 512..], &vec![0x11; 2 * 512]);
        adapter.write_at(VmId(9), 3, 8 * 512, &[0x5a; 4 * 512]).unwrap();
        let mut zero_read = vec![0; 4 * 512];
        adapter.read_at(VmId(9), 3, 8 * 512, &mut zero_read).unwrap();
        assert_eq!(zero_read, vec![0x5a; 4 * 512]);
    }

    #[test]
    fn rejects_wrong_vm_and_generation() {
        let p = partition();
        let backend = MemoryExtentBackend { generation: 3, linear: vec![0; 128 * 1024], zero: vec![0; 24 * 512], ..Default::default() };
        let mut adapter = MappedPartitionBackend::new(backend, VmId(9), 3, "system_a", &p, 8).unwrap();
        assert_eq!(adapter.flush(VmId(8), 3), Err(BlockError::StaleVmGeneration));
        assert_eq!(adapter.flush(VmId(9), 4), Err(BlockError::StaleVmGeneration));
    }
}
