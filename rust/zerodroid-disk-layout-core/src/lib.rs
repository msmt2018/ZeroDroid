#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use zerodroid_image_core::gpt::{GptImageIndex, GptPartition};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutError {
    InvalidSectorSize,
    InvalidDiskGeometry,
    InvalidPartitionRange,
    InvalidPartitionName,
    DuplicatePartitionName,
    DuplicatePartitionGuid,
    PartitionNotFound,
    ZeroLengthRequest,
    UnalignedRequest,
    OutOfPartitionRange,
    IntegerOverflow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartitionView {
    pub index: u32,
    pub name: String,
    pub type_guid: [u8; 16],
    pub unique_guid: [u8; 16],
    pub attributes: u64,
    pub first_lba: u64,
    pub last_lba: u64,
    pub start_byte: u64,
    pub length_bytes: u64,
}

impl PartitionView {
    pub fn end_byte_exclusive(&self) -> Result<u64, LayoutError> {
        self.start_byte.checked_add(self.length_bytes).ok_or(LayoutError::IntegerOverflow)
    }

    /// Maps a partition-relative Guest LBA request into a byte range on the
    /// parent virtual disk. It never resolves to a host physical device.
    pub fn map_relative_lba(
        &self,
        relative_lba: u64,
        block_count: u64,
        logical_block_size: u32,
    ) -> Result<DiskByteRange, LayoutError> {
        if block_count == 0 {
            return Err(LayoutError::ZeroLengthRequest);
        }
        if logical_block_size < 512 || !logical_block_size.is_power_of_two() {
            return Err(LayoutError::InvalidSectorSize);
        }
        let relative_offset = relative_lba
            .checked_mul(u64::from(logical_block_size))
            .ok_or(LayoutError::IntegerOverflow)?;
        let length = block_count
            .checked_mul(u64::from(logical_block_size))
            .ok_or(LayoutError::IntegerOverflow)?;
        let end = relative_offset.checked_add(length).ok_or(LayoutError::IntegerOverflow)?;
        if end > self.length_bytes {
            return Err(LayoutError::OutOfPartitionRange);
        }
        let absolute = self.start_byte.checked_add(relative_offset).ok_or(LayoutError::IntegerOverflow)?;
        Ok(DiskByteRange { offset: absolute, length })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskByteRange {
    pub offset: u64,
    pub length: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GptDiskLayout {
    disk_size_bytes: u64,
    sector_size: u32,
    partitions: Vec<PartitionView>,
}

impl GptDiskLayout {
    pub fn from_index(index: &GptImageIndex) -> Result<Self, LayoutError> {
        if index.sector_size < 512 || !index.sector_size.is_power_of_two() {
            return Err(LayoutError::InvalidSectorSize);
        }
        if index.disk_size_bytes == 0
            || index.disk_size_bytes % u64::from(index.sector_size) != 0
        {
            return Err(LayoutError::InvalidDiskGeometry);
        }
        let disk_sectors = index.disk_size_bytes / u64::from(index.sector_size);
        let mut names = BTreeSet::new();
        let mut guids = BTreeSet::new();
        let mut partitions = Vec::with_capacity(index.partitions.len());
        for entry in &index.partitions {
            let name = decode_name(&entry.name_utf16le)?;
            if !names.insert(name.clone()) {
                return Err(LayoutError::DuplicatePartitionName);
            }
            if !guids.insert(entry.unique_guid) {
                return Err(LayoutError::DuplicatePartitionGuid);
            }
            let first = entry.first_lba;
            let last_exclusive = entry.last_lba.checked_add(1).ok_or(LayoutError::IntegerOverflow)?;
            if first >= last_exclusive || last_exclusive > disk_sectors {
                return Err(LayoutError::InvalidPartitionRange);
            }
            let start_byte = first.checked_mul(u64::from(index.sector_size)).ok_or(LayoutError::IntegerOverflow)?;
            let end_byte = last_exclusive.checked_mul(u64::from(index.sector_size)).ok_or(LayoutError::IntegerOverflow)?;
            let length_bytes = end_byte.checked_sub(start_byte).ok_or(LayoutError::InvalidPartitionRange)?;
            if entry.byte_range.start != start_byte || entry.byte_range.end != end_byte {
                return Err(LayoutError::InvalidPartitionRange);
            }
            partitions.push(PartitionView {
                index: entry.index,
                name,
                type_guid: entry.type_guid,
                unique_guid: entry.unique_guid,
                attributes: entry.attributes,
                first_lba: first,
                last_lba: entry.last_lba,
                start_byte,
                length_bytes,
            });
        }
        partitions.sort_unstable_by_key(|p| p.first_lba);
        if partitions.windows(2).any(|pair| pair[0].last_lba >= pair[1].first_lba) {
            return Err(LayoutError::InvalidPartitionRange);
        }
        Ok(Self {
            disk_size_bytes: index.disk_size_bytes,
            sector_size: index.sector_size,
            partitions,
        })
    }

    pub fn disk_size_bytes(&self) -> u64 { self.disk_size_bytes }
    pub fn sector_size(&self) -> u32 { self.sector_size }
    pub fn partitions(&self) -> &[PartitionView] { &self.partitions }

    pub fn partition_by_name(&self, name: &str) -> Result<&PartitionView, LayoutError> {
        self.partitions.iter().find(|p| p.name == name).ok_or(LayoutError::PartitionNotFound)
    }

    pub fn partition_by_guid(&self, guid: &[u8; 16]) -> Result<&PartitionView, LayoutError> {
        self.partitions.iter().find(|p| &p.unique_guid == guid).ok_or(LayoutError::PartitionNotFound)
    }

    /// Maps an absolute disk request only if the complete range lies inside
    /// one declared partition. GPT metadata and unallocated gaps are excluded.
    pub fn map_partition_range(
        &self,
        name: &str,
        relative_lba: u64,
        block_count: u64,
        logical_block_size: u32,
    ) -> Result<DiskByteRange, LayoutError> {
        self.partition_by_name(name)?.map_relative_lba(relative_lba, block_count, logical_block_size)
    }
}

fn decode_name(bytes: &[u8; 72]) -> Result<String, LayoutError> {
    let units: Vec<u16> = bytes.chunks_exact(2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .collect();
    let end = units.iter().position(|u| *u == 0).unwrap_or(units.len());
    if end == 0 || units[end..].iter().any(|u| *u != 0) {
        return Err(LayoutError::InvalidPartitionName);
    }
    let name = String::from_utf16(&units[..end]).map_err(|_| LayoutError::InvalidPartitionName)?;
    if name.chars().any(|c| c.is_control() || c == '/' || c == '\\') {
        return Err(LayoutError::InvalidPartitionName);
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ops::Range;

    fn entry(index: u32, name: &str, first: u64, last: u64, guid_byte: u8) -> GptPartition {
        let mut name_utf16le = [0_u8; 72];
        for (i, unit) in name.encode_utf16().enumerate() {
            name_utf16le[i * 2..i * 2 + 2].copy_from_slice(&unit.to_le_bytes());
        }
        let mut type_guid = [0; 16];
        type_guid[0] = 0xaa;
        let mut unique_guid = [0; 16];
        unique_guid[0] = guid_byte;
        GptPartition {
            index, type_guid, unique_guid, first_lba: first, last_lba: last,
            attributes: 0, name_utf16le,
            byte_range: first * 512..(last + 1) * 512,
        }
    }

    fn layout(entries: Vec<GptPartition>) -> GptDiskLayout {
        let index = GptImageIndex {
            header: zerodroid_image_core::gpt::GptHeader {
                revision: 0x10000, header_size: 92, current_lba: 1, backup_lba: 4095,
                first_usable_lba: 34, last_usable_lba: 4094, disk_guid: [1; 16],
                partition_entry_lba: 2, partition_entry_count: 128, partition_entry_size: 128,
                partition_entry_crc32: 0, header_range: 512..604, partition_array_range: 1024..17408,
            },
            partitions: entries, disk_size_bytes: 4096 * 512, sector_size: 512,
        };
        GptDiskLayout::from_index(&index).unwrap()
    }

    #[test]
    fn builds_sorted_partition_views_and_maps_relative_lba() {
        let l = layout(vec![entry(1, "userdata", 2048, 3071, 1), entry(0, "boot_a", 34, 2047, 2)]);
        assert_eq!(l.partitions()[0].name, "boot_a");
        assert_eq!(l.partition_by_name("userdata").unwrap().length_bytes, 1024 * 512);
        assert_eq!(l.map_partition_range("userdata", 4, 2, 512), Ok(DiskByteRange { offset: 2052 * 512, length: 1024 }));
    }

    #[test]
    fn refuses_partition_overrun_and_unknown_name() {
        let l = layout(vec![entry(0, "boot", 34, 100, 1)]);
        assert_eq!(l.map_partition_range("boot", 67, 1, 512), Err(LayoutError::OutOfPartitionRange));
        assert_eq!(l.partition_by_name("missing"), Err(LayoutError::PartitionNotFound));
    }

    #[test]
    fn rejects_duplicate_names_guids_and_overlapping_ranges() {
        let index = |partitions| GptImageIndex {
            header: zerodroid_image_core::gpt::GptHeader {
                revision: 0x10000, header_size: 92, current_lba: 1, backup_lba: 4095,
                first_usable_lba: 34, last_usable_lba: 4094, disk_guid: [1; 16],
                partition_entry_lba: 2, partition_entry_count: 128, partition_entry_size: 128,
                partition_entry_crc32: 0, header_range: 512..604, partition_array_range: 1024..17408,
            },
            partitions, disk_size_bytes: 4096 * 512, sector_size: 512,
        };
        assert_eq!(GptDiskLayout::from_index(&index(vec![entry(0, "same", 34, 40, 1), entry(1, "same", 50, 60, 2)])), Err(LayoutError::DuplicatePartitionName));
        assert_eq!(GptDiskLayout::from_index(&index(vec![entry(0, "a", 34, 40, 1), entry(1, "b", 50, 60, 1)])), Err(LayoutError::DuplicatePartitionGuid));
        assert_eq!(GptDiskLayout::from_index(&index(vec![entry(0, "a", 34, 60, 1), entry(1, "b", 50, 70, 2)])), Err(LayoutError::InvalidPartitionRange));
    }

    #[test]
    fn validates_utf16_and_sector_geometry() {
        let mut bad = entry(0, "boot", 34, 40, 1);
        bad.name_utf16le[0] = 0;
        bad.name_utf16le[1] = 0xd8;
        assert_eq!(GptDiskLayout::from_index(&GptImageIndex {
            header: zerodroid_image_core::gpt::GptHeader {
                revision: 0x10000, header_size: 92, current_lba: 1, backup_lba: 4095,
                first_usable_lba: 34, last_usable_lba: 4094, disk_guid: [1; 16],
                partition_entry_lba: 2, partition_entry_count: 128, partition_entry_size: 128,
                partition_entry_crc32: 0, header_range: 512..604, partition_array_range: 1024..17408,
            },
            partitions: vec![bad], disk_size_bytes: 4096 * 512, sector_size: 512,
        }).err(), Some(LayoutError::InvalidPartitionName));
    }
}
