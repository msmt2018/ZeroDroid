use std::collections::{BTreeMap, BTreeSet};

use crate::avb::{
    parse_chain_partition_descriptor, parse_hash_descriptor, parse_hashtree_descriptor,
    parse_kernel_cmdline_descriptor, parse_property_descriptor, AvbParseError,
    VbMetaImageIndex,
};
use crate::gpt::GptImageIndex;
use crate::lp_metadata::LpMetadataIndex;

/// Explicit slot selection. None deliberately means that no A/B suffix is inferred.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotSelection {
    None,
    A,
    B,
}

impl SlotSelection {
    fn suffix(self) -> Option<char> {
        match self {
            Self::None => None,
            Self::A => Some('a'),
            Self::B => Some('b'),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PartitionSource {
    PhysicalGpt { first_lba: u64, last_lba: u64, type_guid: [u8; 16] },
    LogicalLp { group: String, logical_sectors: u64, backing_devices: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogPartition {
    pub name: String,
    pub normalized_name: String,
    pub sources: Vec<PartitionSource>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvbDescriptorKind {
    Hash,
    Hashtree,
    Chain,
    Property,
    KernelCmdline,
    Unknown(u64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvbPartitionReference {
    pub kind: AvbDescriptorKind,
    pub partition_name: Option<String>,
    pub normalized_name: Option<String>,
    pub present_in_catalog: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvbChainEdge {
    pub from_vbmeta: String,
    pub to_partition: String,
    pub normalized_target: String,
    /// The target vbmeta image was not supplied to this catalog build.
    pub target_vbmeta_loaded: bool,
    pub rollback_index_location: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartitionCatalog {
    pub slot: SlotSelection,
    pub partitions: Vec<CatalogPartition>,
    pub avb_references: Vec<AvbPartitionReference>,
    pub chain_edges: Vec<AvbChainEdge>,
    pub duplicate_normalized_names: Vec<String>,
    pub missing_avb_partitions: Vec<String>,
    pub verification: crate::avb::AvbVerificationState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogError {
    InvalidGptName,
    InvalidLpReference,
    Avb(AvbParseError),
    ResourceLimit,
}

impl From<AvbParseError> for CatalogError {
    fn from(value: AvbParseError) -> Self { Self::Avb(value) }
}

fn normalize_slot(name: &str, slot: SlotSelection) -> String {
    let Some(suffix) = slot.suffix() else { return name.to_owned() };
    let selected = format!("_{suffix}");
    if name.ends_with(&selected) { name.to_owned() }
    else if name.ends_with("_a") || name.ends_with("_b") { name.to_owned() }
    else { format!("{name}{selected}") }
}

fn gpt_name(raw: &[u8; 72]) -> Result<String, CatalogError> {
    let units: Vec<u16> = raw.chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .take_while(|unit| *unit != 0)
        .collect();
    if units.is_empty() { return Err(CatalogError::InvalidGptName); }
    let decoded: Result<String, _> = char::decode_utf16(units).collect();
    let name = decoded.map_err(|_| CatalogError::InvalidGptName)?;
    if name.chars().any(|ch| ch.is_control()) { return Err(CatalogError::InvalidGptName); }
    Ok(name)
}

fn descriptor_reference(
    data: &[u8],
    descriptor: &crate::avb::AvbDescriptor,
) -> Result<(AvbDescriptorKind, Option<String>, Option<u32>), CatalogError> {
    match descriptor.tag {
        0 => { parse_property_descriptor(data, descriptor)?; Ok((AvbDescriptorKind::Property, None, None)) }
        1 => { let d = parse_hashtree_descriptor(data, descriptor)?; Ok((AvbDescriptorKind::Hashtree, Some(d.partition_name), None)) }
        2 => { let d = parse_hash_descriptor(data, descriptor)?; Ok((AvbDescriptorKind::Hash, Some(d.partition_name), None)) }
        3 => { parse_kernel_cmdline_descriptor(data, descriptor)?; Ok((AvbDescriptorKind::KernelCmdline, None, None)) }
        4 => { let d = parse_chain_partition_descriptor(data, descriptor)?; Ok((AvbDescriptorKind::Chain, Some(d.partition_name), Some(d.rollback_index_location))) }
        tag => Ok((AvbDescriptorKind::Unknown(tag), None, None)),
    }
}

/// Builds a range-only material index joining GPT physical partitions, liblp logical
/// partitions, and AVB descriptor references. It performs no image extraction or AVB crypto.
pub fn build_partition_catalog(
    gpt: &GptImageIndex,
    lp: Option<&LpMetadataIndex>,
    vbmeta_data: &[u8],
    vbmeta: &VbMetaImageIndex,
    slot: SlotSelection,
    max_items: usize,
) -> Result<PartitionCatalog, CatalogError> {
    let estimated = gpt.partitions.len()
        .checked_add(lp.map_or(0, |index| index.partitions.len()))
        .and_then(|n| n.checked_add(vbmeta.descriptors.len()))
        .ok_or(CatalogError::ResourceLimit)?;
    if estimated > max_items { return Err(CatalogError::ResourceLimit); }

    let mut entries: BTreeMap<String, CatalogPartition> = BTreeMap::new();
    for partition in &gpt.partitions {
        let name = gpt_name(&partition.name_utf16le)?;
        let normalized_name = normalize_slot(&name, slot);
        entries.entry(name.clone()).or_insert_with(|| CatalogPartition {
            name, normalized_name, sources: Vec::new(),
        }).sources.push(PartitionSource::PhysicalGpt {
            first_lba: partition.first_lba,
            last_lba: partition.last_lba,
            type_guid: partition.type_guid,
        });
    }

    if let Some(index) = lp {
        for partition in &index.partitions {
            let group = index.groups.get(partition.group_index as usize)
                .ok_or(CatalogError::InvalidLpReference)?.name.clone();
            let end = partition.first_extent_index.checked_add(partition.num_extents)
                .ok_or(CatalogError::InvalidLpReference)? as usize;
            let extents = index.extents.get(partition.first_extent_index as usize..end)
                .ok_or(CatalogError::InvalidLpReference)?;
            let mut backing = BTreeSet::new();
            for extent in extents {
                if extent.target_type == 0 {
                    let device = index.block_devices.get(extent.target_source as usize)
                        .ok_or(CatalogError::InvalidLpReference)?;
                    backing.insert(device.partition_name.clone());
                }
            }
            let name = partition.name.clone();
            let normalized_name = normalize_slot(&name, slot);
            entries.entry(name.clone()).or_insert_with(|| CatalogPartition {
                name, normalized_name, sources: Vec::new(),
            }).sources.push(PartitionSource::LogicalLp {
                group, logical_sectors: partition.logical_sectors,
                backing_devices: backing.into_iter().collect(),
            });
        }
    }

    let mut normalized_counts = BTreeMap::<String, usize>::new();
    for entry in entries.values() {
        *normalized_counts.entry(entry.normalized_name.clone()).or_default() += 1;
    }
    let duplicate_normalized_names = normalized_counts.into_iter()
        .filter_map(|(name, count)| (count > 1).then_some(name)).collect::<Vec<_>>();

    let mut references = Vec::with_capacity(vbmeta.descriptors.len());
    let mut chain_edges = Vec::new();
    let mut missing = BTreeSet::new();
    for descriptor in &vbmeta.descriptors {
        let (kind, partition_name, rollback_location) = descriptor_reference(vbmeta_data, descriptor)?;
        let normalized_name = partition_name.as_deref().map(|name| normalize_slot(name, slot));
        let present = normalized_name.as_ref().map(|name| entries.values()
            .any(|entry| entry.normalized_name == *name));
        if let (Some(name), Some(false)) = (normalized_name.as_ref(), present) {
            missing.insert(name.clone());
        }
        if kind == AvbDescriptorKind::Chain {
            if let (Some(target), Some(location)) = (partition_name.as_ref(), rollback_location) {
                chain_edges.push(AvbChainEdge {
                    from_vbmeta: "vbmeta".to_owned(),
                    to_partition: target.clone(),
                    normalized_target: normalize_slot(target, slot),
                    target_vbmeta_loaded: false,
                    rollback_index_location: location,
                });
            }
        }
        references.push(AvbPartitionReference {
            kind, partition_name, normalized_name, present_in_catalog: present,
        });
    }

    Ok(PartitionCatalog {
        slot,
        partitions: entries.into_values().collect(),
        avb_references: references,
        chain_edges,
        duplicate_normalized_names,
        missing_avb_partitions: missing.into_iter().collect(),
        verification: vbmeta.verification,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_suffix_is_only_applied_when_explicitly_selected() {
        assert_eq!(normalize_slot("system", SlotSelection::None), "system");
        assert_eq!(normalize_slot("system", SlotSelection::A), "system_a");
        assert_eq!(normalize_slot("vendor_b", SlotSelection::A), "vendor_a");
        assert_eq!(normalize_slot("product_a", SlotSelection::B), "product_b");
    }

    #[test]
    fn gpt_utf16_name_is_decoded_without_loss() {
        let mut raw = [0_u8; 72];
        raw[..4].copy_from_slice(&[0x53, 0x00, 0x79, 0x00]);
        assert_eq!(gpt_name(&raw).unwrap(), "Sy");
        raw[..4].copy_from_slice(&[0x00, 0xd8, 0, 0]);
        assert_eq!(gpt_name(&raw), Err(CatalogError::InvalidGptName));
    }
}
