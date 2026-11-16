#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use zerodroid_image_core::lp_metadata::{LpMetadataIndex, LP_TARGET_LINEAR, LP_TARGET_ZERO};

const SECTOR_BYTES: u64 = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DynamicPartitionError {
    DuplicatePartitionName,
    InvalidPartitionExtentRange,
    InvalidExtent,
    InvalidBlockDevice,
    ExtentOutOfBounds,
    IntegerOverflow,
    ZeroLengthRequest,
    OutOfPartitionRange,
    TooManySegments,
    PartitionNotFound,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtentTarget {
    Linear { block_device_index: u32, block_device_name: String, byte_offset: u64 },
    Zero,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtentSegment {
    pub logical_offset: u64,
    pub length: u64,
    pub target: ExtentTarget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DynamicPartition {
    pub name: String,
    pub attributes: u32,
    pub group_name: String,
    pub logical_size: u64,
    extents: Vec<ExtentSegment>,
}

impl DynamicPartition {
    pub fn extents(&self) -> &[ExtentSegment] { &self.extents }

    /// Maps a partition-relative byte request into bounded linear/zero segments.
    /// A request crossing extent boundaries returns multiple segments.
    pub fn map_range(&self, offset: u64, length: u64, max_segments: usize) -> Result<Vec<ExtentSegment>, DynamicPartitionError> {
        if length == 0 { return Err(DynamicPartitionError::ZeroLengthRequest); }
        let end = offset.checked_add(length).ok_or(DynamicPartitionError::IntegerOverflow)?;
        if end > self.logical_size { return Err(DynamicPartitionError::OutOfPartitionRange); }
        let mut cursor = offset;
        let mut result = Vec::new();
        for extent in &self.extents {
            let extent_end = extent.logical_offset.checked_add(extent.length).ok_or(DynamicPartitionError::IntegerOverflow)?;
            if cursor >= extent_end || end <= extent.logical_offset { continue; }
            if cursor < extent.logical_offset { return Err(DynamicPartitionError::InvalidPartitionExtentRange); }
            let segment_end = end.min(extent_end);
            let delta = cursor - extent.logical_offset;
            let target = match &extent.target {
                ExtentTarget::Zero => ExtentTarget::Zero,
                ExtentTarget::Linear { block_device_index, block_device_name, byte_offset } => {
                    ExtentTarget::Linear {
                        block_device_index: *block_device_index,
                        block_device_name: block_device_name.clone(),
                        byte_offset: byte_offset.checked_add(delta).ok_or(DynamicPartitionError::IntegerOverflow)?,
                    }
                }
            };
            if result.len() >= max_segments { return Err(DynamicPartitionError::TooManySegments); }
            result.push(ExtentSegment { logical_offset: cursor, length: segment_end - cursor, target });
            cursor = segment_end;
            if cursor == end { break; }
        }
        if cursor != end { return Err(DynamicPartitionError::InvalidPartitionExtentRange); }
        Ok(result)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DynamicPartitionLayout {
    pub slot_number: u32,
    pub block_device_names: Vec<String>,
    partitions: Vec<DynamicPartition>,
}

impl DynamicPartitionLayout {
    pub fn from_metadata(index: &LpMetadataIndex) -> Result<Self, DynamicPartitionError> {
        let mut device_names = Vec::with_capacity(index.block_devices.len());
        for device in &index.block_devices {
            if device.size == 0 || device.size % SECTOR_BYTES != 0 {
                return Err(DynamicPartitionError::InvalidBlockDevice);
            }
            device_names.push(device.partition_name.clone());
        }

        let mut names = BTreeSet::new();
        let mut partitions = Vec::with_capacity(index.partitions.len());
        for partition in &index.partitions {
            if !names.insert(partition.name.clone()) {
                return Err(DynamicPartitionError::DuplicatePartitionName);
            }
            let group = index.groups.get(partition.group_index as usize)
                .ok_or(DynamicPartitionError::InvalidPartitionExtentRange)?;
            let first = usize::try_from(partition.first_extent_index)
                .map_err(|_| DynamicPartitionError::IntegerOverflow)?;
            let count = usize::try_from(partition.num_extents)
                .map_err(|_| DynamicPartitionError::IntegerOverflow)?;
            let end = first.checked_add(count).ok_or(DynamicPartitionError::IntegerOverflow)?;
            let source = index.extents.get(first..end)
                .ok_or(DynamicPartitionError::InvalidPartitionExtentRange)?;
            let mut logical_offset = 0_u64;
            let mut extents = Vec::with_capacity(source.len());
            for extent in source {
                if extent.num_sectors == 0 { return Err(DynamicPartitionError::InvalidExtent); }
                let length = extent.num_sectors.checked_mul(SECTOR_BYTES)
                    .ok_or(DynamicPartitionError::IntegerOverflow)?;
                let target = match extent.target_type {
                    LP_TARGET_LINEAR => {
                        let device = index.block_devices.get(extent.target_source as usize)
                            .ok_or(DynamicPartitionError::InvalidBlockDevice)?;
                        let physical_sector_end = extent.target_data.checked_add(extent.num_sectors)
                            .ok_or(DynamicPartitionError::IntegerOverflow)?;
                        let device_sector_end = device.size / SECTOR_BYTES;
                        if extent.target_data < device.first_logical_sector || physical_sector_end > device_sector_end {
                            return Err(DynamicPartitionError::ExtentOutOfBounds);
                        }
                        let byte_offset = extent.target_data.checked_mul(SECTOR_BYTES)
                            .ok_or(DynamicPartitionError::IntegerOverflow)?;
                        ExtentTarget::Linear {
                            block_device_index: extent.target_source,
                            block_device_name: device.partition_name.clone(),
                            byte_offset,
                        }
                    }
                    LP_TARGET_ZERO => ExtentTarget::Zero,
                    _ => return Err(DynamicPartitionError::InvalidExtent),
                };
                extents.push(ExtentSegment { logical_offset, length, target });
                logical_offset = logical_offset.checked_add(length)
                    .ok_or(DynamicPartitionError::IntegerOverflow)?;
            }
            let expected = partition.logical_sectors.checked_mul(SECTOR_BYTES)
                .ok_or(DynamicPartitionError::IntegerOverflow)?;
            if logical_offset != expected || logical_offset == 0 {
                return Err(DynamicPartitionError::InvalidPartitionExtentRange);
            }
            partitions.push(DynamicPartition {
                name: partition.name.clone(),
                attributes: partition.attributes,
                group_name: group.name.clone(),
                logical_size: logical_offset,
                extents,
            });
        }
        Ok(Self { slot_number: index.slot_number, block_device_names: device_names, partitions })
    }

    pub fn partitions(&self) -> &[DynamicPartition] { &self.partitions }

    pub fn partition(&self, name: &str) -> Result<&DynamicPartition, DynamicPartitionError> {
        self.partitions.iter().find(|p| p.name == name).ok_or(DynamicPartitionError::PartitionNotFound)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerodroid_image_core::lp_metadata::{
        LpBlockDevice, LpExtent, LpGeometry, LpGroup, LpPartition,
    };
    use std::ops::Range;

    fn metadata() -> LpMetadataIndex {
        LpMetadataIndex {
            geometry: LpGeometry { metadata_max_size: 4096, metadata_slot_count: 2, logical_block_size: 4096, source_range: 4096..4148 },
            slot_number: 1, major_version: 10, minor_version: 2, header_flags: 0,
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
        }
    }

    #[test]
    fn builds_logical_partition_and_maps_linear_and_zero_extents() {
        let layout = DynamicPartitionLayout::from_metadata(&metadata()).unwrap();
        assert_eq!(layout.slot_number, 1);
        let p = layout.partition("system_a").unwrap();
        assert_eq!(p.logical_size, 24 * 512);
        let segments = p.map_range(6 * 512, 8 * 512, 4).unwrap();
        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0].target, ExtentTarget::Linear { block_device_index: 0, block_device_name: "super".into(), byte_offset: 46 * 512 });
        assert_eq!(segments[0].length, 2 * 512);
        assert_eq!(segments[1].target, ExtentTarget::Zero);
        assert_eq!(segments[2].target, ExtentTarget::Linear { block_device_index: 0, block_device_name: "super".into(), byte_offset: 64 * 512 });
    }

    #[test]
    fn rejects_range_overrun_zero_length_and_segment_budget() {
        let layout = DynamicPartitionLayout::from_metadata(&metadata()).unwrap();
        let p = layout.partition("system_a").unwrap();
        assert_eq!(p.map_range(0, 0, 8), Err(DynamicPartitionError::ZeroLengthRequest));
        assert_eq!(p.map_range(0, 25 * 512, 8), Err(DynamicPartitionError::OutOfPartitionRange));
        assert_eq!(p.map_range(0, 24 * 512, 2), Err(DynamicPartitionError::TooManySegments));
    }

    #[test]
    fn rejects_metadata_partition_extent_length_mismatch() {
        let mut index = metadata();
        index.partitions[0].logical_sectors = 23;
        assert_eq!(DynamicPartitionLayout::from_metadata(&index).err(), Some(DynamicPartitionError::InvalidPartitionExtentRange));
    }

    #[test]
    fn rejects_extent_outside_super_block_device() {
        let mut index = metadata();
        index.extents[0].target_data = 250;
        assert_eq!(DynamicPartitionLayout::from_metadata(&index).err(), Some(DynamicPartitionError::ExtentOutOfBounds));
    }
}
