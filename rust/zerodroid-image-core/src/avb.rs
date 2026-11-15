use core::ops::Range;

const VBMeta_MAGIC: &[u8; 4] = b"AVB0";
const VBMeta_HEADER_SIZE: usize = 256;
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
    if data.len() < VBMeta_HEADER_SIZE {
        return Err(AvbParseError::OutOfBounds);
    }
    if data.get(..4) != Some(VBMeta_MAGIC.as_slice()) {
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

    let auth_start = VBMeta_HEADER_SIZE;
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
        data[128..136].copy_from_slice(b"avbtool");
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
        data[12..20].copy_from_slice(&u64::MAX.to_be_bytes());
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
