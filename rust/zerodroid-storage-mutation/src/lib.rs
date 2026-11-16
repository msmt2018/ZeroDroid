#![forbid(unsafe_code)]

use zerodroid_storage_journal::CommittedTransaction;

pub const MAGIC: [u8; 4] = *b"ZDSM";
pub const SCHEMA_VERSION: u16 = 1;
pub const HEADER_BYTES: usize = 60;
pub const PAGE_SIZE: usize = 4096;
pub const HARD_MAX_NAME_BYTES: usize = 96;
pub const HARD_MAX_OBJECT_CAPACITY: u64 = 64 * 1024 * 1024 * 1024;
pub const HARD_MAX_PAGE_INDEX: u64 = HARD_MAX_OBJECT_CAPACITY / PAGE_SIZE as u64 - 1;
pub const HARD_MAX_PAYLOAD_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum MutationKind {
    VmRegister = 1,
    ObjectCreate = 2,
    ObjectDelete = 3,
    PagePut = 4,
    PageDelete = 5,
}

impl TryFrom<u16> for MutationKind {
    type Error = MutationError;
    fn try_from(value: u16) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::VmRegister),
            2 => Ok(Self::ObjectCreate),
            3 => Ok(Self::ObjectDelete),
            4 => Ok(Self::PagePut),
            5 => Ok(Self::PageDelete),
            _ => Err(MutationError::UnknownKind),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MutationError {
    Truncated,
    BadMagic,
    UnsupportedVersion,
    UnknownKind,
    InvalidIdentity,
    InvalidGeneration,
    InvalidObjectId,
    InvalidPageIndex,
    InvalidCapacity,
    InvalidName,
    InvalidPayloadLength,
    PayloadTooLarge,
    MixedTransactionScope,
    IntegerOverflow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageMutation {
    VmRegister { vm_id: u128, generation: u64, quota_bytes: u64 },
    ObjectCreate { vm_id: u128, generation: u64, object_id: u128, capacity_bytes: u64, name: String },
    ObjectDelete { vm_id: u128, generation: u64, object_id: u128 },
    PagePut { vm_id: u128, generation: u64, object_id: u128, page_index: u64, page: Vec<u8> },
    PageDelete { vm_id: u128, generation: u64, object_id: u128, page_index: u64 },
}

impl StorageMutation {
    pub fn vm_scope(&self) -> (u128, u64) {
        match self {
            Self::VmRegister { vm_id, generation, .. }
            | Self::ObjectCreate { vm_id, generation, .. }
            | Self::ObjectDelete { vm_id, generation, .. }
            | Self::PagePut { vm_id, generation, .. }
            | Self::PageDelete { vm_id, generation, .. } => (*vm_id, *generation),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedTransaction {
    pub transaction_id: u128,
    pub commit_sequence: u64,
    pub vm_id: u128,
    pub generation: u64,
    pub mutations: Vec<StorageMutation>,
}

pub fn encode(mutation: &StorageMutation) -> Result<Vec<u8>, MutationError> {
    let (vm_id, generation) = mutation.vm_scope();
    if vm_id == 0 { return Err(MutationError::InvalidIdentity); }
    if generation == 0 { return Err(MutationError::InvalidGeneration); }

    let (kind, object_id, page_index, payload) = match mutation {
        StorageMutation::VmRegister { quota_bytes, .. } => {
            if *quota_bytes == 0 || *quota_bytes > HARD_MAX_OBJECT_CAPACITY * 4 {
                return Err(MutationError::InvalidCapacity);
            }
            (MutationKind::VmRegister, 0, 0, quota_bytes.to_le_bytes().to_vec())
        }
        StorageMutation::ObjectCreate { object_id, capacity_bytes, name, .. } => {
            validate_object_id(*object_id)?;
            validate_name(name)?;
            if *capacity_bytes == 0 || *capacity_bytes > HARD_MAX_OBJECT_CAPACITY {
                return Err(MutationError::InvalidCapacity);
            }
            let mut payload = Vec::with_capacity(10 + name.len());
            payload.extend_from_slice(&capacity_bytes.to_le_bytes());
            payload.extend_from_slice(&(name.len() as u16).to_le_bytes());
            payload.extend_from_slice(name.as_bytes());
            (MutationKind::ObjectCreate, *object_id, 0, payload)
        }
        StorageMutation::ObjectDelete { object_id, .. } => {
            validate_object_id(*object_id)?;
            (MutationKind::ObjectDelete, *object_id, 0, Vec::new())
        }
        StorageMutation::PagePut { object_id, page_index, page, .. } => {
            validate_object_id(*object_id)?;
            validate_page_index(*page_index)?;
            if page.len() != PAGE_SIZE { return Err(MutationError::InvalidPayloadLength); }
            (MutationKind::PagePut, *object_id, *page_index, page.clone())
        }
        StorageMutation::PageDelete { object_id, page_index, .. } => {
            validate_object_id(*object_id)?;
            validate_page_index(*page_index)?;
            (MutationKind::PageDelete, *object_id, *page_index, Vec::new())
        }
    };
    if payload.len() > HARD_MAX_PAYLOAD_BYTES { return Err(MutationError::PayloadTooLarge); }

    let mut out = Vec::with_capacity(HEADER_BYTES + payload.len());
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&SCHEMA_VERSION.to_le_bytes());
    out.extend_from_slice(&(kind as u16).to_le_bytes());
    out.extend_from_slice(&vm_id.to_le_bytes());
    out.extend_from_slice(&generation.to_le_bytes());
    out.extend_from_slice(&object_id.to_le_bytes());
    out.extend_from_slice(&page_index.to_le_bytes());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&payload);
    Ok(out)
}

pub fn decode(bytes: &[u8]) -> Result<StorageMutation, MutationError> {
    if bytes.len() < HEADER_BYTES { return Err(MutationError::Truncated); }
    if bytes[0..4] != MAGIC { return Err(MutationError::BadMagic); }
    let version = read_u16(bytes, 4)?;
    if version != SCHEMA_VERSION { return Err(MutationError::UnsupportedVersion); }
    let kind = MutationKind::try_from(read_u16(bytes, 6)?)?;
    let vm_id = read_u128(bytes, 8)?;
    let generation = read_u64(bytes, 24)?;
    let object_id = read_u128(bytes, 32)?;
    let page_index = read_u64(bytes, 48)?;
    let payload_len = read_u32(bytes, 56)? as usize;
    if payload_len > HARD_MAX_PAYLOAD_BYTES { return Err(MutationError::PayloadTooLarge); }
    let expected = HEADER_BYTES.checked_add(payload_len).ok_or(MutationError::IntegerOverflow)?;
    if bytes.len() != expected { return Err(MutationError::InvalidPayloadLength); }
    if vm_id == 0 { return Err(MutationError::InvalidIdentity); }
    if generation == 0 { return Err(MutationError::InvalidGeneration); }
    let payload = &bytes[HEADER_BYTES..];

    match kind {
        MutationKind::VmRegister => {
            if object_id != 0 || page_index != 0 || payload.len() != 8 {
                return Err(MutationError::InvalidPayloadLength);
            }
            let quota = u64::from_le_bytes(payload.try_into().map_err(|_| MutationError::Truncated)?);
            if quota == 0 || quota > HARD_MAX_OBJECT_CAPACITY * 4 { return Err(MutationError::InvalidCapacity); }
            Ok(StorageMutation::VmRegister { vm_id, generation, quota_bytes: quota })
        }
        MutationKind::ObjectCreate => {
            validate_object_id(object_id)?;
            if page_index != 0 || payload.len() < 10 { return Err(MutationError::InvalidPayloadLength); }
            let capacity = u64::from_le_bytes(payload[0..8].try_into().map_err(|_| MutationError::Truncated)?);
            let name_len = u16::from_le_bytes(payload[8..10].try_into().map_err(|_| MutationError::Truncated)?) as usize;
            if payload.len() != 10 + name_len { return Err(MutationError::InvalidPayloadLength); }
            if capacity == 0 || capacity > HARD_MAX_OBJECT_CAPACITY { return Err(MutationError::InvalidCapacity); }
            let name = core::str::from_utf8(&payload[10..]).map_err(|_| MutationError::InvalidName)?.to_owned();
            validate_name(&name)?;
            Ok(StorageMutation::ObjectCreate { vm_id, generation, object_id, capacity_bytes: capacity, name })
        }
        MutationKind::ObjectDelete => {
            validate_object_id(object_id)?;
            if page_index != 0 || !payload.is_empty() { return Err(MutationError::InvalidPayloadLength); }
            Ok(StorageMutation::ObjectDelete { vm_id, generation, object_id })
        }
        MutationKind::PagePut => {
            validate_object_id(object_id)?;
            validate_page_index(page_index)?;
            if payload.len() != PAGE_SIZE { return Err(MutationError::InvalidPayloadLength); }
            Ok(StorageMutation::PagePut { vm_id, generation, object_id, page_index, page: payload.to_vec() })
        }
        MutationKind::PageDelete => {
            validate_object_id(object_id)?;
            validate_page_index(page_index)?;
            if !payload.is_empty() { return Err(MutationError::InvalidPayloadLength); }
            Ok(StorageMutation::PageDelete { vm_id, generation, object_id, page_index })
        }
    }
}

/// Decodes a committed journal transaction and rejects mixed VM/generation scope.
/// This is a validation boundary only; applying mutations requires a transactional
/// durable backend and is deliberately not performed by this crate.
pub fn decode_transaction(tx: &CommittedTransaction) -> Result<DecodedTransaction, MutationError> {
    let mut mutations = Vec::with_capacity(tx.mutations.len());
    let mut scope = None;
    for payload in &tx.mutations {
        let mutation = decode(payload)?;
        let next_scope = mutation.vm_scope();
        if scope.is_some_and(|current| current != next_scope) {
            return Err(MutationError::MixedTransactionScope);
        }
        scope = Some(next_scope);
        mutations.push(mutation);
    }
    let (vm_id, generation) = scope.ok_or(MutationError::InvalidPayloadLength)?;
    Ok(DecodedTransaction {
        transaction_id: tx.transaction_id,
        commit_sequence: tx.commit_sequence,
        vm_id,
        generation,
        mutations,
    })
}

fn validate_object_id(id: u128) -> Result<(), MutationError> {
    if id == 0 { Err(MutationError::InvalidObjectId) } else { Ok(()) }
}
fn validate_page_index(index: u64) -> Result<(), MutationError> {
    if index > HARD_MAX_PAGE_INDEX { Err(MutationError::InvalidPageIndex) } else { Ok(()) }
}
fn validate_name(name: &str) -> Result<(), MutationError> {
    if name.is_empty() || name.len() > HARD_MAX_NAME_BYTES
        || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.')
    { Err(MutationError::InvalidName) } else { Ok(()) }
}
fn read_u16(b: &[u8], at: usize) -> Result<u16, MutationError> {
    Ok(u16::from_le_bytes(b.get(at..at + 2).ok_or(MutationError::Truncated)?.try_into().map_err(|_| MutationError::Truncated)?))
}
fn read_u32(b: &[u8], at: usize) -> Result<u32, MutationError> {
    Ok(u32::from_le_bytes(b.get(at..at + 4).ok_or(MutationError::Truncated)?.try_into().map_err(|_| MutationError::Truncated)?))
}
fn read_u64(b: &[u8], at: usize) -> Result<u64, MutationError> {
    Ok(u64::from_le_bytes(b.get(at..at + 8).ok_or(MutationError::Truncated)?.try_into().map_err(|_| MutationError::Truncated)?))
}
fn read_u128(b: &[u8], at: usize) -> Result<u128, MutationError> {
    Ok(u128::from_le_bytes(b.get(at..at + 16).ok_or(MutationError::Truncated)?.try_into().map_err(|_| MutationError::Truncated)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_page_and_object_mutations() {
        let items = [
            StorageMutation::VmRegister { vm_id: 1, generation: 2, quota_bytes: 8192 },
            StorageMutation::ObjectCreate { vm_id: 1, generation: 2, object_id: 3, capacity_bytes: 8192, name: "super".into() },
            StorageMutation::PagePut { vm_id: 1, generation: 2, object_id: 3, page_index: 1, page: vec![0x5a; PAGE_SIZE] },
            StorageMutation::PageDelete { vm_id: 1, generation: 2, object_id: 3, page_index: 1 },
            StorageMutation::ObjectDelete { vm_id: 1, generation: 2, object_id: 3 },
        ];
        for item in items { assert_eq!(decode(&encode(&item).unwrap()).unwrap(), item); }
    }

    #[test]
    fn rejects_bad_identity_capacity_name_and_page_index() {
        assert_eq!(encode(&StorageMutation::VmRegister { vm_id: 0, generation: 1, quota_bytes: 1 }), Err(MutationError::InvalidIdentity));
        assert_eq!(encode(&StorageMutation::VmRegister { vm_id: 1, generation: 1, quota_bytes: 0 }), Err(MutationError::InvalidCapacity));
        assert_eq!(encode(&StorageMutation::ObjectCreate { vm_id: 1, generation: 1, object_id: 1, capacity_bytes: 4096, name: "../host".into() }), Err(MutationError::InvalidName));
        assert_eq!(encode(&StorageMutation::PageDelete { vm_id: 1, generation: 1, object_id: 1, page_index: HARD_MAX_PAGE_INDEX + 1 }), Err(MutationError::InvalidPageIndex));
    }

    #[test]
    fn rejects_trailing_bytes_and_wrong_page_size() {
        let item = StorageMutation::PageDelete { vm_id: 1, generation: 1, object_id: 2, page_index: 0 };
        let mut bytes = encode(&item).unwrap();
        bytes.push(0);
        assert_eq!(decode(&bytes), Err(MutationError::InvalidPayloadLength));
        let bad = StorageMutation::PagePut { vm_id: 1, generation: 1, object_id: 2, page_index: 0, page: vec![1; PAGE_SIZE - 1] };
        assert_eq!(encode(&bad), Err(MutationError::InvalidPayloadLength));
    }

    #[test]
    fn transaction_must_have_one_vm_generation_scope() {
        let a = encode(&StorageMutation::PageDelete { vm_id: 1, generation: 1, object_id: 2, page_index: 0 }).unwrap();
        let b = encode(&StorageMutation::PageDelete { vm_id: 1, generation: 2, object_id: 2, page_index: 0 }).unwrap();
        let tx = CommittedTransaction { transaction_id: 7, mutations: vec![a, b], commit_sequence: 9 };
        assert_eq!(decode_transaction(&tx), Err(MutationError::MixedTransactionScope));
    }

    #[test]
    fn decodes_committed_transaction_scope() {
        let a = encode(&StorageMutation::PageDelete { vm_id: 4, generation: 8, object_id: 9, page_index: 2 }).unwrap();
        let tx = CommittedTransaction { transaction_id: 7, mutations: vec![a], commit_sequence: 11 };
        let decoded = decode_transaction(&tx).unwrap();
        assert_eq!((decoded.vm_id, decoded.generation, decoded.commit_sequence), (4, 8, 11));
    }
}

mod replay;
pub use replay::{ReplayError, ReplayState, HARD_MAX_REPLAY_OBJECTS_PER_VM, HARD_MAX_REPLAY_OBJECT_IDS, HARD_MAX_REPLAY_TRACKED_PAGES_PER_VM, HARD_MAX_REPLAY_VMS, HARD_MAX_REPLAY_VM_QUOTA};
