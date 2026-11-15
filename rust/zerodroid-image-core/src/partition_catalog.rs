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
    pub duplicate_gpt_names: Vec<String>,
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
    let mut seen_gpt_names = BTreeSet::new();
    let mut duplicate_gpt_names = BTreeSet::new();
    for partition in &gpt.partitions {
        let name = gpt_name(&partition.name_utf16le)?;
        if !seen_gpt_names.insert(name.clone()) { duplicate_gpt_names.insert(name.clone()); }
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
        duplicate_gpt_names: duplicate_gpt_names.into_iter().collect(),
        missing_avb_partitions: missing.into_iter().collect(),
        verification: vbmeta.verification,
    })
}


/// A caller-supplied VBMeta image. The byte slice and parsed index must refer to
/// the same immutable image; the catalog checks all descriptor ranges again.
#[derive(Debug, Clone, Copy)]
pub struct VbMetaInput<'a> {
    pub partition_name: &'a str,
    pub data: &'a [u8],
    pub index: &'a VbMetaImageIndex,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VbMetaChainNode {
    pub partition_name: String,
    pub normalized_name: String,
    pub rollback_index: u64,
    pub rollback_index_location: u32,
    pub descriptor_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VbMetaChainEdge {
    pub from_partition: String,
    pub to_partition: String,
    pub normalized_target: String,
    pub target_loaded: bool,
    pub rollback_index_location: u32,
    /// None means the child image was not supplied.
    pub public_key_matches: Option<bool>,
    /// None means the child image was not supplied.
    pub rollback_location_matches: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VbMetaChainGraph {
    pub slot: SlotSelection,
    pub nodes: Vec<VbMetaChainNode>,
    pub edges: Vec<VbMetaChainEdge>,
    pub missing_targets: Vec<String>,
    pub duplicate_nodes: Vec<String>,
    pub cyclic_edges: Vec<(String, String)>,
    /// Structural parsing only; this graph is never a signature/trust result.
    pub verification: crate::avb::AvbVerificationState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainGraphError {
    EmptyPartitionName,
    ResourceLimit,
    InvalidImageIndex,
    Avb(AvbParseError),
}

impl From<AvbParseError> for ChainGraphError {
    fn from(value: AvbParseError) -> Self { Self::Avb(value) }
}

/// Builds a bounded multi-VBMeta dependency graph from already parsed VBMeta images.
/// Chain descriptor public keys are compared byte-for-byte with a supplied child's
/// embedded public-key field, but no key trust or signature verification is implied.
pub fn build_vbmeta_chain_graph(
    inputs: &[VbMetaInput<'_>],
    slot: SlotSelection,
    max_nodes: usize,
    max_edges: usize,
) -> Result<VbMetaChainGraph, ChainGraphError> {
    if inputs.len() > max_nodes { return Err(ChainGraphError::ResourceLimit); }
    let mut nodes = Vec::with_capacity(inputs.len());
    let mut names = BTreeMap::<String, usize>::new();
    let mut duplicate_nodes = BTreeSet::new();

    for input in inputs {
        if input.partition_name.is_empty() { return Err(ChainGraphError::EmptyPartitionName); }
        let normalized = normalize_slot(input.partition_name, slot);
        if names.contains_key(&normalized) {
            duplicate_nodes.insert(normalized);
            continue;
        }
        let node_index = nodes.len();
        names.insert(normalized.clone(), node_index);
        nodes.push(VbMetaChainNode {
            partition_name: input.partition_name.to_owned(),
            normalized_name: normalized,
            rollback_index: input.index.header.rollback_index,
            rollback_index_location: input.index.header.rollback_index_location,
            descriptor_count: input.index.descriptors.len(),
        });
    }

    // Keep the first occurrence for a duplicate name, but report the ambiguity.
    let mut edges = Vec::new();
    let mut missing_targets = BTreeSet::new();
    let mut adjacency = vec![Vec::<usize>::new(); nodes.len()];

    let mut processed_sources = BTreeSet::new();
    for input in inputs {
        let source_normalized = normalize_slot(input.partition_name, slot);
        if !processed_sources.insert(source_normalized.clone()) { continue; }
        let Some(&from_index) = names.get(&source_normalized) else { continue };
        if nodes[from_index].partition_name != input.partition_name { continue; }

        for descriptor in &input.index.descriptors {
            if descriptor.tag != 4 { continue; }
            let chain = parse_chain_partition_descriptor(input.data, descriptor)?;
            if edges.len() >= max_edges { return Err(ChainGraphError::ResourceLimit); }
            let target_normalized = normalize_slot(&chain.partition_name, slot);
            let target_index = names.get(&target_normalized).copied();
            let child = target_index.and_then(|idx| {
                inputs.iter().find(|candidate| {
                    normalize_slot(candidate.partition_name, slot) == target_normalized
                })
            });
            let public_key_matches = if let Some(candidate) = child {
                let child_key = candidate.data.get(candidate.index.header.public_key.clone())
                    .ok_or(ChainGraphError::InvalidImageIndex)?;
                let expected_key = input.data.get(chain.public_key.clone())
                    .ok_or(ChainGraphError::InvalidImageIndex)?;
                Some(child_key == expected_key)
            } else {
                None
            };
            let rollback_location_matches = child.map(|candidate| {
                candidate.index.header.rollback_index_location == chain.rollback_index_location
            });
            if let Some(target) = target_index {
                adjacency[from_index].push(target);
            } else {
                missing_targets.insert(target_normalized.clone());
            }
            edges.push(VbMetaChainEdge {
                from_partition: nodes[from_index].partition_name.clone(),
                to_partition: chain.partition_name,
                normalized_target: target_normalized,
                target_loaded: target_index.is_some(),
                rollback_index_location: chain.rollback_index_location,
                public_key_matches,
                rollback_location_matches,
            });
        }
    }

    // Iterative three-colour DFS avoids recursion on attacker-controlled graphs.
    let mut color = vec![0_u8; nodes.len()];
    let mut cyclic_edges = BTreeSet::<(String, String)>::new();
    for root in 0..nodes.len() {
        if color[root] != 0 { continue; }
        color[root] = 1;
        let mut stack = vec![(root, 0_usize)];
        while let Some((node, next_edge)) = stack.last_mut() {
            if *next_edge >= adjacency[*node].len() {
                color[*node] = 2;
                stack.pop();
                continue;
            }
            let target = adjacency[*node][*next_edge];
            *next_edge += 1;
            match color[target] {
                0 => {
                    color[target] = 1;
                    stack.push((target, 0));
                }
                1 => {
                    cyclic_edges.insert((
                        nodes[*node].partition_name.clone(),
                        nodes[target].partition_name.clone(),
                    ));
                }
                _ => {}
            }
        }
    }

    Ok(VbMetaChainGraph {
        slot,
        nodes,
        edges,
        missing_targets: missing_targets.into_iter().collect(),
        duplicate_nodes: duplicate_nodes.into_iter().collect(),
        cyclic_edges: cyclic_edges.into_iter().collect(),
        verification: crate::avb::AvbVerificationState::NotPerformed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_vbmeta(target: Option<&str>, key: [u8; 4], location: u32) -> (Vec<u8>, VbMetaImageIndex) {
        let mut data = vec![0_u8; 104];
        let mut descriptors = Vec::new();
        if let Some(target) = target {
            let name = target.as_bytes();
            data[0..8].copy_from_slice(&4_u64.to_be_bytes());
            data[8..16].copy_from_slice(&84_u64.to_be_bytes());
            data[16..20].copy_from_slice(&location.to_be_bytes());
            data[20..24].copy_from_slice(&(name.len() as u32).to_be_bytes());
            data[24..28].copy_from_slice(&4_u32.to_be_bytes());
            data[92..92 + name.len()].copy_from_slice(name);
            data[96..100].copy_from_slice(&key);
            descriptors.push(crate::avb::AvbDescriptor {
                tag: 4, body: 16..100, encoded: 0..100, vbmeta_minor: 2,
            });
        }
        data[100..104].copy_from_slice(&key);
        let header = crate::avb::VbMetaHeader {
            required_major: 1, required_minor: 2,
            authentication_block_size: 0, auxiliary_block_size: 0, algorithm_type: 0,
            hash: 0..0, signature: 0..0, public_key: 100..104,
            public_key_metadata: 0..0, descriptors: 0..100,
            rollback_index: 9, flags: 0, rollback_index_location: location,
            release_string: String::new(),
        };
        let index = VbMetaImageIndex {
            header, authentication_block: 0..0, auxiliary_block: 0..104,
            descriptors, verification: crate::avb::AvbVerificationState::NotPerformed,
            total_indexed_bytes: 104,
        };
        (data, index)
    }

    #[test]
    fn chain_graph_resolves_child_and_checks_key_and_location() {
        let (root_data, root_index) = synthetic_vbmeta(Some("vbmeta_system"), [1,2,3,4], 3);
        let (child_data, child_index) = synthetic_vbmeta(None, [1,2,3,4], 3);
        let inputs = [
            VbMetaInput { partition_name: "vbmeta", data: &root_data, index: &root_index },
            VbMetaInput { partition_name: "vbmeta_system", data: &child_data, index: &child_index },
        ];
        let graph = build_vbmeta_chain_graph(&inputs, SlotSelection::None, 8, 8).unwrap();
        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.edges.len(), 1);
        assert!(graph.edges[0].target_loaded);
        assert_eq!(graph.edges[0].public_key_matches, Some(true));
        assert_eq!(graph.edges[0].rollback_location_matches, Some(true));
        assert!(graph.missing_targets.is_empty());
        assert_eq!(graph.verification, crate::avb::AvbVerificationState::NotPerformed);
    }

    #[test]
    fn chain_graph_reports_child_key_and_rollback_location_mismatch() {
        let (root_data, root_index) = synthetic_vbmeta(Some("vbmeta_system"), [1,2,3,4], 3);
        let (child_data, child_index) = synthetic_vbmeta("vbmeta_system", None, [9,8,7,6], 4);
        let inputs = [
            VbMetaInput { partition_name: "vbmeta", data: &root_data, index: &root_index },
            VbMetaInput { partition_name: "vbmeta_system", data: &child_data, index: &child_index },
        ];
        let graph = build_vbmeta_chain_graph(&inputs, SlotSelection::None, 8, 8).unwrap();
        assert_eq!(graph.edges[0].public_key_matches, Some(false));
        assert_eq!(graph.edges[0].rollback_location_matches, Some(false));
    }

    #[test]
    fn chain_graph_reports_duplicate_normalized_nodes() {
        let (data_a, index_a) = synthetic_vbmeta(None, [1,2,3,4], 0);
        let (data_b, index_b) = synthetic_vbmeta("vbmeta", None, [5,6,7,8], 0);
        let inputs = [
            VbMetaInput { partition_name: "vbmeta", data: &data_a, index: &index_a },
            VbMetaInput { partition_name: "vbmeta", data: &data_b, index: &index_b },
        ];
        let graph = build_vbmeta_chain_graph(&inputs, SlotSelection::None, 8, 8).unwrap();
        assert_eq!(graph.nodes.len(), 1);
        assert_eq!(graph.duplicate_nodes, vec!["vbmeta".to_owned()]);
    }

    #[test]
    fn chain_graph_reports_missing_child_and_cycle() {
        let (root_data, root_index) = synthetic_vbmeta(Some("vbmeta"), [1,2,3,4], 2);
        let inputs = [VbMetaInput { partition_name: "vbmeta", data: &root_data, index: &root_index }];
        let graph = build_vbmeta_chain_graph(&inputs, SlotSelection::None, 4, 4).unwrap();
        assert_eq!(graph.cyclic_edges, vec![("vbmeta".to_owned(), "vbmeta".to_owned())]);

        let (other_data, other_index) = synthetic_vbmeta("vbmeta", Some("vbmeta_system"), [1,2,3,4], 2);
        let missing = [VbMetaInput { partition_name: "vbmeta", data: &other_data, index: &other_index }];
        let graph = build_vbmeta_chain_graph(&missing, SlotSelection::None, 4, 4).unwrap();
        assert_eq!(graph.missing_targets, vec!["vbmeta_system".to_owned()]);
        assert!(!graph.edges[0].target_loaded);
    }

    #[test]
    fn chain_graph_enforces_node_and_edge_budgets() {
        let (data, index) = synthetic_vbmeta(Some("child"), [1,2,3,4], 1);
        let inputs = [VbMetaInput { partition_name: "vbmeta", data: &data, index: &index }];
        assert_eq!(build_vbmeta_chain_graph(&inputs, SlotSelection::None, 0, 4), Err(ChainGraphError::ResourceLimit));
        assert_eq!(build_vbmeta_chain_graph(&inputs, SlotSelection::None, 4, 0), Err(ChainGraphError::ResourceLimit));
    }

    #[test]
    fn slot_suffix_is_only_applied_when_explicitly_selected() {
        assert_eq!(normalize_slot("system", SlotSelection::None), "system");
        assert_eq!(normalize_slot("system", SlotSelection::A), "system_a");
        assert_eq!(normalize_slot("vendor_b", SlotSelection::A), "vendor_b");
        assert_eq!(normalize_slot("product_a", SlotSelection::B), "product_a");
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
