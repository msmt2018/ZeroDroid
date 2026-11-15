use core::ops::Range;

const VBMETA_MAGIC: &[u8; 4] = b"AVB0";
const VBMETA_HEADER_SIZE: usize = 256;
const DESCRIPTOR_HEADER_SIZE: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvbParseError {
    OutOfBounds,
    IntegerOverflow,
    ResourceLimit,
    InvalidMagic,
    UnsupportedVersion,
    InvalidHeader,
    InvalidBlockLayout,
    InvalidDescriptor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AvbParseBudget {
    pub max_input_bytes: u64,
    pub max_descriptors: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvbVerificationState {
    NotPerformed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvbDescriptor {
    pub tag: u64,
    pub body: Range<usize>,
    pub encoded: Range<usize>,
    pub vbmeta_minor: u32,
}

/// AVB Hash descriptor (tag 2). Variable data remains indexed in the original VBMeta buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvbHashDescriptor {
    pub image_size: u64,
    pub hash_algorithm: String,
    pub partition_name: String,
    pub salt: Range<usize>,
    pub digest: Range<usize>,
    pub flags: u32,
}

/// AVB Hashtree descriptor (tag 1). Variable payload ranges reference the input buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvbHashtreeDescriptor {
    pub dm_verity_version: u32,
    pub image_size: u64,
    pub tree_offset: u64,
    pub tree_size: u64,
    pub data_block_size: u32,
    pub hash_block_size: u32,
    pub fec_num_roots: u32,
    pub fec_offset: u64,
    pub fec_size: u64,
    pub hash_algorithm: String,
    pub partition_name: String,
    pub salt: Range<usize>,
    pub root_digest: Range<usize>,
    pub flags: u32,
}

/// AVB Property descriptor (tag 0); key and value are UTF-8 strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvbPropertyDescriptor {
    pub key: String,
    pub value: String,
}

/// AVB Kernel Cmdline descriptor (tag 3); command line bytes remain in the source buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvbKernelCmdlineDescriptor {
    pub flags: u32,
    pub command_line: Range<usize>,
}

/// AVB Chain Partition descriptor (tag 4), v1.0-v1.2 layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvbChainPartitionDescriptor {
    pub rollback_index_location: u32,
    pub partition_name: String,
    pub public_key: Range<usize>,
}

/// Reads an AVB fixed-width NUL-terminated string and verifies zero padding.
fn avb_fixed_string(bytes: &[u8]) -> Result<String, AvbParseError> {
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    if bytes[end..].iter().any(|b| *b != 0)
        || !bytes[..end].iter().all(|b| b.is_ascii_alphanumeric() || *b == b'_')
        || end == 0
    {
        return Err(AvbParseError::InvalidDescriptor);
    }
    core::str::from_utf8(&bytes[..end])
        .map(str::to_owned)
        .map_err(|_| AvbParseError::InvalidDescriptor)
}

/// Parses AVB Hashtree fields and returns salt/root-digest ranges without copying payloads.
pub fn parse_hashtree_descriptor(
    data: &[u8],
    descriptor: &AvbDescriptor,
) -> Result<AvbHashtreeDescriptor, AvbParseError> {
    const TAG_HASHTREE: u64 = 1;
    const FIXED_BODY_SIZE: usize = 164;
    if descriptor.tag != TAG_HASHTREE || descriptor.body.len() < FIXED_BODY_SIZE {
        return Err(AvbParseError::InvalidDescriptor);
    }
    let body = data.get(descriptor.body.clone()).ok_or(AvbParseError::OutOfBounds)?;
    let dm_verity_version = u32be(body, 0)?;
    let image_size = u64be(body, 4)?;
    let tree_offset = u64be(body, 12)?;
    let tree_size = u64be(body, 20)?;
    let data_block_size = u32be(body, 28)?;
    let hash_block_size = u32be(body, 32)?;
    let fec_num_roots = u32be(body, 36)?;
    let fec_offset = u64be(body, 40)?;
    let fec_size = u64be(body, 48)?;
    let hash_algorithm = avb_fixed_string(body.get(56..88).ok_or(AvbParseError::InvalidDescriptor)?)?;
    let partition_len = usize::try_from(u32be(body, 88)?).map_err(|_| AvbParseError::IntegerOverflow)?;
    let salt_len = usize::try_from(u32be(body, 92)?).map_err(|_| AvbParseError::IntegerOverflow)?;
    let digest_len = usize::try_from(u32be(body, 96)?).map_err(|_| AvbParseError::IntegerOverflow)?;
    let flags = u32be(body, 100)?;
    if flags & !3 != 0 || (descriptor.vbmeta_minor == 0 && flags != 0) || body.get(104..164).ok_or(AvbParseError::InvalidDescriptor)?.iter().any(|b| *b != 0) {
        return Err(AvbParseError::InvalidDescriptor);
    }

    let variable_len = partition_len.checked_add(salt_len).and_then(|v| v.checked_add(digest_len))
        .ok_or(AvbParseError::IntegerOverflow)?;
    let padded_len = variable_len.checked_add(7).ok_or(AvbParseError::IntegerOverflow)? & !7;
    if FIXED_BODY_SIZE.checked_add(padded_len).ok_or(AvbParseError::IntegerOverflow)? != body.len() {
        return Err(AvbParseError::InvalidDescriptor);
    }
    let variable_start = descriptor.body.start.checked_add(FIXED_BODY_SIZE)
        .ok_or(AvbParseError::IntegerOverflow)?;
    let partition_end = variable_start.checked_add(partition_len).ok_or(AvbParseError::IntegerOverflow)?;
    let salt_end = partition_end.checked_add(salt_len).ok_or(AvbParseError::IntegerOverflow)?;
    let digest_end = salt_end.checked_add(digest_len).ok_or(AvbParseError::IntegerOverflow)?;
    if data.get(digest_end..descriptor.body.end).ok_or(AvbParseError::OutOfBounds)?
        .iter().any(|b| *b != 0)
    {
        return Err(AvbParseError::InvalidDescriptor);
    }
    let partition_name = core::str::from_utf8(data.get(variable_start..partition_end)
        .ok_or(AvbParseError::OutOfBounds)?)
        .map_err(|_| AvbParseError::InvalidDescriptor)?.to_owned();
    if partition_name.is_empty() || partition_name.as_bytes().contains(&0) || (descriptor.vbmeta_minor == 0 && digest_len == 0) {
        return Err(AvbParseError::InvalidDescriptor);
    }
    Ok(AvbHashtreeDescriptor {
        dm_verity_version, image_size, tree_offset, tree_size, data_block_size, hash_block_size,
        fec_num_roots, fec_offset, fec_size, hash_algorithm, partition_name,
        salt: partition_end..salt_end, root_digest: salt_end..digest_end, flags,
    })
}

/// Parses a Property descriptor, including both NUL terminators and 8-byte padding.
pub fn parse_property_descriptor(
    data: &[u8],
    descriptor: &AvbDescriptor,
) -> Result<AvbPropertyDescriptor, AvbParseError> {
    const TAG_PROPERTY: u64 = 0;
    const FIXED_BODY_SIZE: usize = 16;
    if descriptor.tag != TAG_PROPERTY || descriptor.body.len() < FIXED_BODY_SIZE {
        return Err(AvbParseError::InvalidDescriptor);
    }
    let body = data.get(descriptor.body.clone()).ok_or(AvbParseError::OutOfBounds)?;
    let key_len = usize::try_from(u64be(body, 0)?).map_err(|_| AvbParseError::IntegerOverflow)?;
    let value_len = usize::try_from(u64be(body, 8)?).map_err(|_| AvbParseError::IntegerOverflow)?;
    let variable_len = key_len.checked_add(1).and_then(|v| v.checked_add(value_len))
        .and_then(|v| v.checked_add(1)).ok_or(AvbParseError::IntegerOverflow)?;
    let padded_len = variable_len.checked_add(7).ok_or(AvbParseError::IntegerOverflow)? & !7;
    if FIXED_BODY_SIZE.checked_add(padded_len).ok_or(AvbParseError::IntegerOverflow)? != body.len() || key_len == 0 {
        return Err(AvbParseError::InvalidDescriptor);
    }
    let start = descriptor.body.start.checked_add(FIXED_BODY_SIZE).ok_or(AvbParseError::IntegerOverflow)?;
    let key_end = start.checked_add(key_len).ok_or(AvbParseError::IntegerOverflow)?;
    let value_start = key_end.checked_add(1).ok_or(AvbParseError::IntegerOverflow)?;
    let value_end = value_start.checked_add(value_len).ok_or(AvbParseError::IntegerOverflow)?;
    let terminator = data.get(key_end..value_start).ok_or(AvbParseError::OutOfBounds)?;
    if terminator != [0] || data.get(value_end..value_end + 1).ok_or(AvbParseError::OutOfBounds)? != [0]
        || data.get(value_end + 1..descriptor.body.end).ok_or(AvbParseError::OutOfBounds)?
            .iter().any(|b| *b != 0)
    {
        return Err(AvbParseError::InvalidDescriptor);
    }
    let key = core::str::from_utf8(data.get(start..key_end).ok_or(AvbParseError::OutOfBounds)?)
        .map_err(|_| AvbParseError::InvalidDescriptor)?.to_owned();
    let value = core::str::from_utf8(data.get(value_start..value_end).ok_or(AvbParseError::OutOfBounds)?)
        .map_err(|_| AvbParseError::InvalidDescriptor)?.to_owned();
    if key.as_bytes().contains(&0) || value.as_bytes().contains(&0) {
        return Err(AvbParseError::InvalidDescriptor);
    }
    Ok(AvbPropertyDescriptor { key, value })
}

/// Parses a Kernel Cmdline descriptor and validates UTF-8 and zero padding.
pub fn parse_kernel_cmdline_descriptor(
    data: &[u8],
    descriptor: &AvbDescriptor,
) -> Result<AvbKernelCmdlineDescriptor, AvbParseError> {
    const TAG_KERNEL_CMDLINE: u64 = 3;
    const FIXED_BODY_SIZE: usize = 8;
    if descriptor.tag != TAG_KERNEL_CMDLINE || descriptor.body.len() < FIXED_BODY_SIZE {
        return Err(AvbParseError::InvalidDescriptor);
    }
    let body = data.get(descriptor.body.clone()).ok_or(AvbParseError::OutOfBounds)?;
    let flags = u32be(body, 0)?;
    let command_len = usize::try_from(u32be(body, 4)?).map_err(|_| AvbParseError::IntegerOverflow)?;
    if flags & !3 != 0 || flags == 3 {
        return Err(AvbParseError::InvalidDescriptor);
    }
    let padded_len = command_len.checked_add(7).ok_or(AvbParseError::IntegerOverflow)? & !7;
    if FIXED_BODY_SIZE.checked_add(padded_len).ok_or(AvbParseError::IntegerOverflow)? != body.len() {
        return Err(AvbParseError::InvalidDescriptor);
    }
    let start = descriptor.body.start.checked_add(FIXED_BODY_SIZE).ok_or(AvbParseError::IntegerOverflow)?;
    let end = start.checked_add(command_len).ok_or(AvbParseError::IntegerOverflow)?;
    let command = data.get(start..end).ok_or(AvbParseError::OutOfBounds)?;
    core::str::from_utf8(command).map_err(|_| AvbParseError::InvalidDescriptor)?;
    if data.get(end..descriptor.body.end).ok_or(AvbParseError::OutOfBounds)?
        .iter().any(|b| *b != 0)
    {
        return Err(AvbParseError::InvalidDescriptor);
    }
    Ok(AvbKernelCmdlineDescriptor { flags, command_line: start..end })
}

/// Parses the v1.0-v1.2 Chain Partition descriptor (no v1.3 flags field).
pub fn parse_chain_partition_descriptor(
    data: &[u8],
    descriptor: &AvbDescriptor,
) -> Result<AvbChainPartitionDescriptor, AvbParseError> {
    const TAG_CHAIN_PARTITION: u64 = 4;
    const FIXED_BODY_SIZE: usize = 76;
    if descriptor.tag != TAG_CHAIN_PARTITION || descriptor.body.len() < FIXED_BODY_SIZE {
        return Err(AvbParseError::InvalidDescriptor);
    }
    let body = data.get(descriptor.body.clone()).ok_or(AvbParseError::OutOfBounds)?;
    let rollback_index_location = u32be(body, 0)?;
    let partition_len = usize::try_from(u32be(body, 4)?).map_err(|_| AvbParseError::IntegerOverflow)?;
    let public_key_len = usize::try_from(u32be(body, 8)?).map_err(|_| AvbParseError::IntegerOverflow)?;
    if body.get(12..76).ok_or(AvbParseError::InvalidDescriptor)?.iter().any(|b| *b != 0) {
        return Err(AvbParseError::InvalidDescriptor);
    }
    let variable_len = partition_len.checked_add(public_key_len).ok_or(AvbParseError::IntegerOverflow)?;
    let padded_len = variable_len.checked_add(7).ok_or(AvbParseError::IntegerOverflow)? & !7;
    if FIXED_BODY_SIZE.checked_add(padded_len).ok_or(AvbParseError::IntegerOverflow)? != body.len() {
        return Err(AvbParseError::InvalidDescriptor);
    }
    let variable_start = descriptor.body.start.checked_add(FIXED_BODY_SIZE)
        .ok_or(AvbParseError::IntegerOverflow)?;
    let partition_end = variable_start.checked_add(partition_len).ok_or(AvbParseError::IntegerOverflow)?;
    let key_end = partition_end.checked_add(public_key_len).ok_or(AvbParseError::IntegerOverflow)?;
    if data.get(key_end..descriptor.body.end).ok_or(AvbParseError::OutOfBounds)?
        .iter().any(|b| *b != 0)
    {
        return Err(AvbParseError::InvalidDescriptor);
    }
    let partition_name = core::str::from_utf8(data.get(variable_start..partition_end)
        .ok_or(AvbParseError::OutOfBounds)?)
        .map_err(|_| AvbParseError::InvalidDescriptor)?.to_owned();
    if partition_name.is_empty() || partition_name.as_bytes().contains(&0) || public_key_len == 0 {
        return Err(AvbParseError::InvalidDescriptor);
    }
    Ok(AvbChainPartitionDescriptor {
        rollback_index_location, partition_name, public_key: partition_end..key_end,
    })
}

/// Parses the typed fields of an AVB Hash descriptor without copying its salt or digest.
pub fn parse_hash_descriptor(
    data: &[u8],
    descriptor: &AvbDescriptor,
) -> Result<AvbHashDescriptor, AvbParseError> {
    const TAG_HASH: u64 = 2;
    const FIXED_BODY_SIZE: usize = 116;
    if descriptor.tag != TAG_HASH || descriptor.body.len() < FIXED_BODY_SIZE {
        return Err(AvbParseError::InvalidDescriptor);
    }
    let body = data.get(descriptor.body.clone()).ok_or(AvbParseError::OutOfBounds)?;
    let image_size = u64be(body, 0)?;
    let algorithm_bytes = body.get(8..40).ok_or(AvbParseError::InvalidDescriptor)?;
    let partition_len = usize::try_from(u32be(body, 40)?).map_err(|_| AvbParseError::IntegerOverflow)?;
    let salt_len = usize::try_from(u32be(body, 44)?).map_err(|_| AvbParseError::IntegerOverflow)?;
    let digest_len = usize::try_from(u32be(body, 48)?).map_err(|_| AvbParseError::IntegerOverflow)?;
    let flags = u32be(body, 52)?;
    if flags & !1 != 0 || (descriptor.vbmeta_minor == 0 && (flags != 0 || digest_len == 0)) || body.get(56..116).ok_or(AvbParseError::InvalidDescriptor)?.iter().any(|b| *b != 0) {
        return Err(AvbParseError::InvalidDescriptor);
    }

    let variable_len = partition_len.checked_add(salt_len).and_then(|v| v.checked_add(digest_len))
        .ok_or(AvbParseError::IntegerOverflow)?;
    let padded_len = variable_len.checked_add(7).ok_or(AvbParseError::IntegerOverflow)? & !7;
    if FIXED_BODY_SIZE.checked_add(padded_len).ok_or(AvbParseError::IntegerOverflow)? != body.len() {
        return Err(AvbParseError::InvalidDescriptor);
    }
    let variable_start = descriptor.body.start.checked_add(FIXED_BODY_SIZE)
        .ok_or(AvbParseError::IntegerOverflow)?;
    let partition_end = variable_start.checked_add(partition_len).ok_or(AvbParseError::IntegerOverflow)?;
    let salt_end = partition_end.checked_add(salt_len).ok_or(AvbParseError::IntegerOverflow)?;
    let digest_end = salt_end.checked_add(digest_len).ok_or(AvbParseError::IntegerOverflow)?;
    let padded_end = descriptor.body.end;
    if data.get(digest_end..padded_end).ok_or(AvbParseError::OutOfBounds)?.iter().any(|b| *b != 0) {
        return Err(AvbParseError::InvalidDescriptor);
    }

    let partition_name = core::str::from_utf8(data.get(variable_start..partition_end)
        .ok_or(AvbParseError::OutOfBounds)?)
        .map_err(|_| AvbParseError::InvalidDescriptor)?.to_owned();
    if partition_name.is_empty() || partition_name.as_bytes().contains(&0) {
        return Err(AvbParseError::InvalidDescriptor);
    }
    let algorithm_end = algorithm_bytes.iter().position(|b| *b == 0).unwrap_or(algorithm_bytes.len());
    if algorithm_bytes[algorithm_end..].iter().any(|b| *b != 0)
        || !algorithm_bytes[..algorithm_end].iter().all(|b| b.is_ascii_alphanumeric() || *b == b'_')
    {
        return Err(AvbParseError::InvalidDescriptor);
    }
    let hash_algorithm = core::str::from_utf8(&algorithm_bytes[..algorithm_end])
        .map_err(|_| AvbParseError::InvalidDescriptor)?.to_owned();
    if hash_algorithm.is_empty() {
        return Err(AvbParseError::InvalidDescriptor);
    }

    Ok(AvbHashDescriptor {
        image_size,
        hash_algorithm,
        partition_name,
        salt: partition_end..salt_end,
        digest: salt_end..digest_end,
        flags,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VbMetaHeader {
    pub required_major: u32,
    pub required_minor: u32,
    pub authentication_block_size: u64,
    pub auxiliary_block_size: u64,
    pub algorithm_type: u32,
    pub hash: Range<usize>,
    pub signature: Range<usize>,
    pub public_key: Range<usize>,
    pub public_key_metadata: Range<usize>,
    pub descriptors: Range<usize>,
    pub rollback_index: u64,
    pub flags: u32,
    pub rollback_index_location: u32,
    pub release_string: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VbMetaImageIndex {
    pub header: VbMetaHeader,
    pub authentication_block: Range<usize>,
    pub auxiliary_block: Range<usize>,
    pub descriptors: Vec<AvbDescriptor>,
    pub verification: AvbVerificationState,
    pub total_indexed_bytes: u64,
}

fn u64be(data: &[u8], offset: usize) -> Result<u64, AvbParseError> {
    let end = offset.checked_add(8).ok_or(AvbParseError::IntegerOverflow)?;
    let bytes = data.get(offset..end).ok_or(AvbParseError::OutOfBounds)?;
    Ok(u64::from_be_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5], bytes[6], bytes[7],
    ]))
}

fn u32be(data: &[u8], offset: usize) -> Result<u32, AvbParseError> {
    let end = offset.checked_add(4).ok_or(AvbParseError::IntegerOverflow)?;
    let bytes = data.get(offset..end).ok_or(AvbParseError::OutOfBounds)?;
    Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn checked_block_range(
    block_start: usize,
    block_size: u64,
    offset: u64,
    size: u64,
    total: usize,
) -> Result<Range<usize>, AvbParseError> {
    let end_in_block = offset.checked_add(size).ok_or(AvbParseError::IntegerOverflow)?;
    if end_in_block > block_size {
        return Err(AvbParseError::OutOfBounds);
    }
    let start = u64::try_from(block_start).map_err(|_| AvbParseError::IntegerOverflow)?
        .checked_add(offset).ok_or(AvbParseError::IntegerOverflow)?;
    let end = start.checked_add(size).ok_or(AvbParseError::IntegerOverflow)?;
    if end > u64::try_from(total).map_err(|_| AvbParseError::IntegerOverflow)? {
        return Err(AvbParseError::OutOfBounds);
    }
    Ok(
        usize::try_from(start).map_err(|_| AvbParseError::IntegerOverflow)?
            ..usize::try_from(end).map_err(|_| AvbParseError::IntegerOverflow)?,
    )
}

fn ensure_non_overlapping(ranges: &[Range<usize>]) -> Result<(), AvbParseError> {
    let mut sorted: Vec<Range<usize>> = ranges.iter().filter(|r| !r.is_empty()).cloned().collect();
    sorted.sort_unstable_by_key(|r| r.start);
    for pair in sorted.windows(2) {
        if pair[0].end > pair[1].start {
            return Err(AvbParseError::InvalidBlockLayout);
        }
    }
    Ok(())
}

/// Parses a VBMeta image and indexes its authenticated/auxiliary areas and descriptor records.
/// This is structural parsing only; no digest, key trust, or signature verification is performed.
pub fn parse_vbmeta(
    data: &[u8],
    budget: AvbParseBudget,
) -> Result<VbMetaImageIndex, AvbParseError> {
    if u64::try_from(data.len()).map_err(|_| AvbParseError::IntegerOverflow)? > budget.max_input_bytes {
        return Err(AvbParseError::ResourceLimit);
    }
    if data.len() < VBMETA_HEADER_SIZE {
        return Err(AvbParseError::OutOfBounds);
    }
    if data.get(..4) != Some(VBMETA_MAGIC.as_slice()) {
        return Err(AvbParseError::InvalidMagic);
    }

    let required_major = u32be(data, 4)?;
    let required_minor = u32be(data, 8)?;
    if required_major != 1 || required_minor > 2 {
        return Err(AvbParseError::UnsupportedVersion);
    }

    let authentication_block_size = u64be(data, 12)?;
    let auxiliary_block_size = u64be(data, 20)?;
    let algorithm_type = u32be(data, 28)?;

    let hash_offset = u64be(data, 32)?;
    let hash_size = u64be(data, 40)?;
    let signature_offset = u64be(data, 48)?;
    let signature_size = u64be(data, 56)?;
    let public_key_offset = u64be(data, 64)?;
    let public_key_size = u64be(data, 72)?;
    let public_key_metadata_offset = u64be(data, 80)?;
    let public_key_metadata_size = u64be(data, 88)?;
    let descriptors_offset = u64be(data, 96)?;
    let descriptors_size = u64be(data, 104)?;
    let rollback_index = u64be(data, 112)?;
    let flags = u32be(data, 120)?;
    let rollback_index_location = u32be(data, 124)?;

    if data.get(176..256).ok_or(AvbParseError::OutOfBounds)?.iter().any(|b| *b != 0) {
        return Err(AvbParseError::InvalidHeader);
    }
    let release_bytes = data.get(128..176).ok_or(AvbParseError::OutOfBounds)?;
    let release_end = release_bytes.iter().position(|b| *b == 0).unwrap_or(release_bytes.len());
    if release_bytes[release_end..].iter().any(|b| *b != 0) {
        return Err(AvbParseError::InvalidHeader);
    }
    let release_string = core::str::from_utf8(&release_bytes[..release_end])
        .map_err(|_| AvbParseError::InvalidHeader)?
        .to_owned();
    if authentication_block_size % 64 != 0 || auxiliary_block_size % 64 != 0 {
        return Err(AvbParseError::InvalidBlockLayout);
    }

    let auth_start = VBMETA_HEADER_SIZE;
    let auth_end_u64 = u64::try_from(auth_start)
        .map_err(|_| AvbParseError::IntegerOverflow)?
        .checked_add(authentication_block_size)
        .ok_or(AvbParseError::IntegerOverflow)?;
    let aux_start_u64 = auth_end_u64;
    let total_end_u64 = aux_start_u64
        .checked_add(auxiliary_block_size)
        .ok_or(AvbParseError::IntegerOverflow)?;
    if total_end_u64 > u64::try_from(data.len()).map_err(|_| AvbParseError::IntegerOverflow)? {
        return Err(AvbParseError::OutOfBounds);
    }
    let auth_end = usize::try_from(auth_end_u64).map_err(|_| AvbParseError::IntegerOverflow)?;
    let aux_start = auth_end;
    let total_end = usize::try_from(total_end_u64).map_err(|_| AvbParseError::IntegerOverflow)?;

    let hash = checked_block_range(auth_start, authentication_block_size, hash_offset, hash_size, data.len())?;
    let signature = checked_block_range(auth_start, authentication_block_size, signature_offset, signature_size, data.len())?;
    let public_key = checked_block_range(aux_start, auxiliary_block_size, public_key_offset, public_key_size, data.len())?;
    let public_key_metadata = checked_block_range(aux_start, auxiliary_block_size, public_key_metadata_offset, public_key_metadata_size, data.len())?;
    let descriptors_range = checked_block_range(aux_start, auxiliary_block_size, descriptors_offset, descriptors_size, data.len())?;

    ensure_non_overlapping(&[hash.clone(), signature.clone()])?;
    ensure_non_overlapping(&[
        public_key.clone(),
        public_key_metadata.clone(),
        descriptors_range.clone(),
    ])?;

    let mut descriptors = Vec::new();
    let mut cursor = descriptors_range.start;
    while cursor < descriptors_range.end {
        if descriptors.len() >= budget.max_descriptors as usize {
            return Err(AvbParseError::ResourceLimit);
        }
        let header_end = cursor.checked_add(DESCRIPTOR_HEADER_SIZE).ok_or(AvbParseError::IntegerOverflow)?;
        if header_end > descriptors_range.end {
            return Err(AvbParseError::InvalidDescriptor);
        }
        let tag = u64be(data, cursor)?;
        let following = u64be(data, cursor + 8)?;
        let record_end = u64::try_from(header_end).map_err(|_| AvbParseError::IntegerOverflow)?
            .checked_add(following).ok_or(AvbParseError::IntegerOverflow)?;
        if record_end > u64::try_from(descriptors_range.end).map_err(|_| AvbParseError::IntegerOverflow)? {
            return Err(AvbParseError::InvalidDescriptor);
        }
        let record_end = usize::try_from(record_end).map_err(|_| AvbParseError::IntegerOverflow)?;
        descriptors.push(AvbDescriptor {
            tag,
            body: header_end..record_end,
            encoded: cursor..record_end,
            vbmeta_minor: required_minor,
        });
        cursor = record_end;
    }

    let header = VbMetaHeader {
        required_major,
        required_minor,
        authentication_block_size,
        auxiliary_block_size,
        algorithm_type,
        hash,
        signature,
        public_key,
        public_key_metadata,
        descriptors: descriptors_range,
        rollback_index,
        flags,
        rollback_index_location,
        release_string,
    };
    Ok(VbMetaImageIndex {
        header,
        authentication_block: auth_start..auth_end,
        auxiliary_block: aux_start..total_end,
        descriptors,
        verification: AvbVerificationState::NotPerformed,
        total_indexed_bytes: u64::try_from(total_end).map_err(|_| AvbParseError::IntegerOverflow)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vbmeta() -> Vec<u8> {
        let mut data = vec![0u8; 256 + 64 + 128];
        data[..4].copy_from_slice(b"AVB0");
        data[4..8].copy_from_slice(&1u32.to_be_bytes());
        data[8..12].copy_from_slice(&2u32.to_be_bytes());
        data[12..20].copy_from_slice(&64u64.to_be_bytes());
        data[20..28].copy_from_slice(&128u64.to_be_bytes());
        data[28..32].copy_from_slice(&1u32.to_be_bytes());
        data[32..40].copy_from_slice(&0u64.to_be_bytes());
        data[40..48].copy_from_slice(&32u64.to_be_bytes());
        data[48..56].copy_from_slice(&32u64.to_be_bytes());
        data[56..64].copy_from_slice(&32u64.to_be_bytes());
        data[64..72].copy_from_slice(&0u64.to_be_bytes());
        data[72..80].copy_from_slice(&32u64.to_be_bytes());
        data[80..88].copy_from_slice(&32u64.to_be_bytes());
        data[88..96].copy_from_slice(&16u64.to_be_bytes());
        data[96..104].copy_from_slice(&48u64.to_be_bytes());
        data[104..112].copy_from_slice(&16u64.to_be_bytes());
        data[112..120].copy_from_slice(&7u64.to_be_bytes());
        data[120..124].copy_from_slice(&0u32.to_be_bytes());
        data[124..128].copy_from_slice(&0u32.to_be_bytes());
        data[128..135].copy_from_slice(b"avbtool");
        // Authentication block: hash [256,288), signature [288,320).
        // Auxiliary block: public key [320,352), metadata [352,368), descriptor [368,384).
        data[368..376].copy_from_slice(&0u64.to_be_bytes());
        data[376..384].copy_from_slice(&0u64.to_be_bytes());
        data
    }

    fn budget() -> AvbParseBudget {
        AvbParseBudget { max_input_bytes: 1 << 20, max_descriptors: 32 }
    }

    #[test]
    fn parses_vbmeta_layout_and_descriptor_index() {
        let parsed = parse_vbmeta(&vbmeta(), budget()).unwrap();
        assert_eq!(parsed.header.required_major, 1);
        assert_eq!(parsed.header.required_minor, 2);
        assert_eq!(parsed.header.rollback_index, 7);
        assert_eq!(parsed.header.hash, 256..288);
        assert_eq!(parsed.header.signature, 288..320);
        assert_eq!(parsed.header.public_key, 320..352);
        assert_eq!(parsed.header.public_key_metadata, 352..368);
        assert_eq!(parsed.descriptors.len(), 1);
        assert_eq!(parsed.descriptors[0].tag, 0);
        assert_eq!(parsed.verification, AvbVerificationState::NotPerformed);
    }

    #[test]
    fn parses_hash_descriptor_fields_and_rejects_malformed_padding() {
        let mut data = vbmeta();
        // Replace the zero-sized descriptor with a tag-2 Hash descriptor.
        data.resize(256 + 64 + 256, 0);
        data[20..28].copy_from_slice(&256u64.to_be_bytes());
        data[96..104].copy_from_slice(&48u64.to_be_bytes());
        data[104..112].copy_from_slice(&148u64.to_be_bytes());
        let start = 368usize;
        data[start..start + 8].copy_from_slice(&2u64.to_be_bytes());
        data[start + 8..start + 16].copy_from_slice(&132u64.to_be_bytes());
        let body = start + 16;
        data[body..body + 8].copy_from_slice(&4096u64.to_be_bytes());
        data[body + 8..body + 14].copy_from_slice(b"sha256");
        data[body + 40..body + 44].copy_from_slice(&4u32.to_be_bytes());
        data[body + 44..body + 48].copy_from_slice(&2u32.to_be_bytes());
        data[body + 48..body + 52].copy_from_slice(&4u32.to_be_bytes());
        data[body + 116..body + 120].copy_from_slice(b"boot");
        data[body + 120..body + 122].copy_from_slice(&[0xaa, 0xbb]);
        data[body + 122..body + 126].copy_from_slice(&[1, 2, 3, 4]);
        let parsed = parse_vbmeta(&data, budget()).unwrap();
        let hash = parse_hash_descriptor(&data, &parsed.descriptors[0]).unwrap();
        assert_eq!(hash.image_size, 4096);
        assert_eq!(hash.hash_algorithm, "sha256");
        assert_eq!(hash.partition_name, "boot");
        assert_eq!(hash.salt, body + 120..body + 122);
        assert_eq!(hash.digest, body + 122..body + 126);

        data[body + 126] = 1;
        let parsed = parse_vbmeta(&data, budget()).unwrap();
        assert_eq!(parse_hash_descriptor(&data, &parsed.descriptors[0]), Err(AvbParseError::InvalidDescriptor));
    }

    #[test]
    fn parses_hashtree_descriptor_and_checks_padding() {
        let mut data = vbmeta();
        data.resize(256 + 64 + 256, 0);
        data[20..28].copy_from_slice(&256u64.to_be_bytes());
        data[96..104].copy_from_slice(&48u64.to_be_bytes());
        data[104..112].copy_from_slice(&196u64.to_be_bytes());
        let start = 368usize;
        data[start..start + 8].copy_from_slice(&1u64.to_be_bytes());
        data[start + 8..start + 16].copy_from_slice(&180u64.to_be_bytes());
        let body = start + 16;
        data[body..body + 4].copy_from_slice(&1u32.to_be_bytes());
        data[body + 4..body + 12].copy_from_slice(&4096u64.to_be_bytes());
        data[body + 12..body + 20].copy_from_slice(&4096u64.to_be_bytes());
        data[body + 20..body + 28].copy_from_slice(&1024u64.to_be_bytes());
        data[body + 28..body + 32].copy_from_slice(&4096u32.to_be_bytes());
        data[body + 32..body + 36].copy_from_slice(&4096u32.to_be_bytes());
        data[body + 56..body + 62].copy_from_slice(b"sha256");
        data[body + 88..body + 92].copy_from_slice(&4u32.to_be_bytes());
        data[body + 92..body + 96].copy_from_slice(&2u32.to_be_bytes());
        data[body + 96..body + 100].copy_from_slice(&4u32.to_be_bytes());
        data[body + 164..body + 168].copy_from_slice(b"root");
        data[body + 168..body + 170].copy_from_slice(&[0xaa, 0xbb]);
        data[body + 170..body + 174].copy_from_slice(&[1, 2, 3, 4]);
        let parsed = parse_vbmeta(&data, budget()).unwrap();
        let tree = parse_hashtree_descriptor(&data, &parsed.descriptors[0]).unwrap();
        assert_eq!(tree.image_size, 4096);
        assert_eq!(tree.tree_offset, 4096);
        assert_eq!(tree.hash_algorithm, "sha256");
        assert_eq!(tree.partition_name, "root");
        assert_eq!(tree.salt, body + 168..body + 170);
        assert_eq!(tree.root_digest, body + 170..body + 174);

        data[body + 174] = 1;
        let parsed = parse_vbmeta(&data, budget()).unwrap();
        assert_eq!(parse_hashtree_descriptor(&data, &parsed.descriptors[0]), Err(AvbParseError::InvalidDescriptor));
    }

    #[test]
    fn parses_legacy_chain_partition_descriptor_and_rejects_empty_key() {
        let mut data = vbmeta();
        data.resize(256 + 64 + 256, 0);
        data[20..28].copy_from_slice(&256u64.to_be_bytes());
        data[96..104].copy_from_slice(&48u64.to_be_bytes());
        data[104..112].copy_from_slice(&100u64.to_be_bytes());
        let start = 368usize;
        data[start..start + 8].copy_from_slice(&4u64.to_be_bytes());
        data[start + 8..start + 16].copy_from_slice(&84u64.to_be_bytes());
        let body = start + 16;
        data[body..body + 4].copy_from_slice(&3u32.to_be_bytes());
        data[body + 4..body + 8].copy_from_slice(&4u32.to_be_bytes());
        data[body + 8..body + 12].copy_from_slice(&4u32.to_be_bytes());
        data[body + 76..body + 80].copy_from_slice(b"vbme");
        data[body + 80..body + 84].copy_from_slice(&[9, 8, 7, 6]);
        let parsed = parse_vbmeta(&data, budget()).unwrap();
        let chain = parse_chain_partition_descriptor(&data, &parsed.descriptors[0]).unwrap();
        assert_eq!(chain.rollback_index_location, 3);
        assert_eq!(chain.partition_name, "vbme");
        assert_eq!(chain.public_key, body + 80..body + 84);

        data[body + 8..body + 12].copy_from_slice(&0u32.to_be_bytes());
        let parsed = parse_vbmeta(&data, budget()).unwrap();
        assert_eq!(parse_chain_partition_descriptor(&data, &parsed.descriptors[0]), Err(AvbParseError::InvalidDescriptor));
    }

    #[test]
    fn parses_property_descriptor_and_rejects_missing_nul() {
        let mut data = vbmeta();
        data.resize(256 + 64 + 256, 0);
        data[20..28].copy_from_slice(&256u64.to_be_bytes());
        data[96..104].copy_from_slice(&48u64.to_be_bytes());
        data[104..112].copy_from_slice(&48u64.to_be_bytes());
        let start = 368usize;
        data[start..start + 8].copy_from_slice(&0u64.to_be_bytes());
        data[start + 8..start + 16].copy_from_slice(&32u64.to_be_bytes());
        let body = start + 16;
        data[body..body + 8].copy_from_slice(&3u64.to_be_bytes());
        data[body + 8..body + 16].copy_from_slice(&2u64.to_be_bytes());
        data[body + 16..body + 19].copy_from_slice(b"foo");
        data[body + 19] = 0;
        data[body + 20..body + 22].copy_from_slice(b"42");
        data[body + 22] = 0;
        let parsed = parse_vbmeta(&data, budget()).unwrap();
        assert_eq!(parse_property_descriptor(&data, &parsed.descriptors[0]).unwrap(),
            AvbPropertyDescriptor { key: "foo".into(), value: "42".into() });
        data[body + 19] = b'x';
        let parsed = parse_vbmeta(&data, budget()).unwrap();
        assert_eq!(parse_property_descriptor(&data, &parsed.descriptors[0]), Err(AvbParseError::InvalidDescriptor));
    }

    #[test]
    fn parses_kernel_cmdline_and_rejects_conflicting_flags() {
        let mut data = vbmeta();
        data.resize(256 + 64 + 256, 0);
        data[20..28].copy_from_slice(&256u64.to_be_bytes());
        data[96..104].copy_from_slice(&48u64.to_be_bytes());
        data[104..112].copy_from_slice(&24u64.to_be_bytes());
        let start = 368usize;
        data[start..start + 8].copy_from_slice(&3u64.to_be_bytes());
        data[start + 8..start + 16].copy_from_slice(&8u64.to_be_bytes());
        let body = start + 16;
        data[body..body + 4].copy_from_slice(&1u32.to_be_bytes());
        data[body + 4..body + 8].copy_from_slice(&3u32.to_be_bytes());
        data[body + 8..body + 11].copy_from_slice(b"foo");
        let parsed = parse_vbmeta(&data, budget()).unwrap();
        let cmdline = parse_kernel_cmdline_descriptor(&data, &parsed.descriptors[0]).unwrap();
        assert_eq!(cmdline.flags, 1);
        assert_eq!(&data[cmdline.command_line], b"foo");
        data[body..body + 4].copy_from_slice(&3u32.to_be_bytes());
        let parsed = parse_vbmeta(&data, budget()).unwrap();
        assert_eq!(parse_kernel_cmdline_descriptor(&data, &parsed.descriptors[0]), Err(AvbParseError::InvalidDescriptor));
    }

    #[test]
    fn rejects_bad_magic_and_unsupported_version() {
        let mut data = vbmeta();
        data[0] = 0;
        assert_eq!(parse_vbmeta(&data, budget()), Err(AvbParseError::InvalidMagic));
        let mut data = vbmeta();
        data[4..8].copy_from_slice(&2u32.to_be_bytes());
        assert_eq!(parse_vbmeta(&data, budget()), Err(AvbParseError::UnsupportedVersion));
    }

    #[test]
    fn rejects_block_overflow_and_overlapping_ranges() {
        let mut data = vbmeta();
        data[12..20].copy_from_slice(&(u64::MAX - 63).to_be_bytes());
        assert_eq!(parse_vbmeta(&data, budget()), Err(AvbParseError::IntegerOverflow));
        let mut data = vbmeta();
        data[48..56].copy_from_slice(&16u64.to_be_bytes());
        assert_eq!(parse_vbmeta(&data, budget()), Err(AvbParseError::InvalidBlockLayout));
    }

    #[test]
    fn rejects_descriptor_truncation_and_budget_exhaustion() {
        let mut data = vbmeta();
        data[104..112].copy_from_slice(&15u64.to_be_bytes());
        assert_eq!(parse_vbmeta(&data, budget()), Err(AvbParseError::InvalidDescriptor));
        assert_eq!(
            parse_vbmeta(&vbmeta(), AvbParseBudget { max_input_bytes: 4096, max_descriptors: 0 }),
            Err(AvbParseError::ResourceLimit)
        );
    }

    #[test]
    fn rejects_unaligned_blocks_and_nonzero_release_padding() {
        let mut data = vbmeta();
        data[20..28].copy_from_slice(&127u64.to_be_bytes());
        assert_eq!(parse_vbmeta(&data, budget()), Err(AvbParseError::InvalidBlockLayout));
        let mut data = vbmeta();
        data[140] = 1;
        assert_eq!(parse_vbmeta(&data, budget()), Err(AvbParseError::InvalidHeader));
    }

    #[test]
    fn rejects_reserved_bytes_and_bad_release_string() {
        let mut data = vbmeta();
        data[200] = 1;
        assert_eq!(parse_vbmeta(&data, budget()), Err(AvbParseError::InvalidHeader));
        let mut data = vbmeta();
        data[128] = 0xff;
        assert_eq!(parse_vbmeta(&data, budget()), Err(AvbParseError::InvalidHeader));
    }
}
