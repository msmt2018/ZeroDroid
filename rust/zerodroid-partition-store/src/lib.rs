#![forbid(unsafe_code)]

use core::fmt;

pub const DEFAULT_MAX_IMAGE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
pub const DEFAULT_MAX_CHUNK_BYTES: usize = 1024 * 1024;
pub const MAX_PARTITION_NAME_BYTES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VmId(pub u128);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartitionInfo {
    pub capacity_bytes: u64,
    pub flashable: bool,
    pub erasable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlashPolicy {
    pub max_image_bytes: u64,
    pub max_chunk_bytes: usize,
}

impl Default for FlashPolicy {
    fn default() -> Self {
        Self {
            max_image_bytes: DEFAULT_MAX_IMAGE_BYTES,
            max_chunk_bytes: DEFAULT_MAX_CHUNK_BYTES,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreError {
    InvalidPartitionName,
    UnknownPartition,
    NotFlashable,
    NotErasable,
    InvalidSize,
    ImageTooLarge,
    ImageExceedsPartition,
    InvalidChunkSize,
    InvalidWriteOffset,
    WriteOverflow,
    IncompleteImage,
    StaleVmGeneration,
    StagingQuotaExceeded,
    StagingObjectMissing,
    DigestMismatch,
    BackendFailure,
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

fn validate_partition_name(name: &str) -> Result<(), StoreError> {
    if name.is_empty()
        || name.len() > MAX_PARTITION_NAME_BYTES
        || !name.bytes().all(|b| {
            b.is_ascii_lowercase()
                || b.is_ascii_digit()
                || b == b'_'
                || b == b'-'
                || b == b'.'
        })
        || name == "."
        || name == ".."
    {
        return Err(StoreError::InvalidPartitionName);
    }
    Ok(())
}

/// The storage backend owns namespace isolation, durable staging, per-VM quota,
/// locking and atomic metadata replacement. A handle must never be accepted
/// outside the VM and generation passed to the corresponding methods.
pub trait PartitionStore {
    type Handle: Copy + Eq;

    fn check_generation(&self, vm: VmId, generation: u64) -> Result<(), StoreError>;
    fn partition_info(&self, vm: VmId, partition: &str) -> Result<PartitionInfo, StoreError>;
    fn begin_staging(
        &mut self,
        vm: VmId,
        generation: u64,
        partition: &str,
        expected_size: u64,
    ) -> Result<Self::Handle, StoreError>;
    fn write_staging(
        &mut self,
        handle: Self::Handle,
        offset: u64,
        bytes: &[u8],
    ) -> Result<(), StoreError>;
    fn verify_staging(
        &mut self,
        handle: Self::Handle,
        expected_size: u64,
        expected_sha256: [u8; 32],
    ) -> Result<(), StoreError>;
    /// Must atomically publish the staged object as the active partition image.
    /// On failure the old partition image must remain available.
    fn commit_staging(
        &mut self,
        handle: Self::Handle,
        vm: VmId,
        generation: u64,
        partition: &str,
    ) -> Result<(), StoreError>;
    fn abort_staging(&mut self, handle: Self::Handle) -> Result<(), StoreError>;
    /// Must atomically replace the partition contents with the backend's
    /// defined erased state; never map this to a host physical block device.
    fn erase_partition_atomic(
        &mut self,
        vm: VmId,
        generation: u64,
        partition: &str,
    ) -> Result<(), StoreError>;
}

pub struct FlashTransaction<'a, S: PartitionStore> {
    store: &'a mut S,
    handle: Option<S::Handle>,
    vm: VmId,
    generation: u64,
    partition: String,
    expected_size: u64,
    received_size: u64,
    policy: FlashPolicy,
}

impl<'a, S: PartitionStore> FlashTransaction<'a, S> {
    pub fn begin(
        store: &'a mut S,
        vm: VmId,
        generation: u64,
        partition: &str,
        expected_size: u64,
        policy: FlashPolicy,
    ) -> Result<Self, StoreError> {
        validate_partition_name(partition)?;
        if expected_size == 0 {
            return Err(StoreError::InvalidSize);
        }
        if policy.max_image_bytes == 0 || expected_size > policy.max_image_bytes {
            return Err(StoreError::ImageTooLarge);
        }
        if policy.max_chunk_bytes == 0 || policy.max_chunk_bytes > DEFAULT_MAX_CHUNK_BYTES {
            return Err(StoreError::InvalidChunkSize);
        }
        store.check_generation(vm, generation)?;
        let info = store.partition_info(vm, partition)?;
        if !info.flashable {
            return Err(StoreError::NotFlashable);
        }
        if expected_size > info.capacity_bytes {
            return Err(StoreError::ImageExceedsPartition);
        }
        let handle = store.begin_staging(vm, generation, partition, expected_size)?;
        Ok(Self {
            store,
            handle: Some(handle),
            vm,
            generation,
            partition: partition.to_owned(),
            expected_size,
            received_size: 0,
            policy,
        })
    }

    /// Writes are strictly sequential and bounded. The backend must reject
    /// writes that target a different VM namespace or stale generation.
    pub fn write_chunk(&mut self, offset: u64, bytes: &[u8]) -> Result<u64, StoreError> {
        if bytes.is_empty() || bytes.len() > self.policy.max_chunk_bytes {
            return Err(StoreError::InvalidChunkSize);
        }
        if offset != self.received_size {
            return Err(StoreError::InvalidWriteOffset);
        }
        let length = u64::try_from(bytes.len()).map_err(|_| StoreError::WriteOverflow)?;
        let end = offset.checked_add(length).ok_or(StoreError::WriteOverflow)?;
        if end > self.expected_size {
            return Err(StoreError::WriteOverflow);
        }
        self.store.check_generation(self.vm, self.generation)?;
        let handle = self.handle.ok_or(StoreError::StagingObjectMissing)?;
        self.store.write_staging(handle, offset, bytes)?;
        self.received_size = end;
        Ok(self.expected_size - self.received_size)
    }

    /// Digest verification and atomic publication are backend responsibilities.
    /// This API never treats a merely parsed image as a trusted boot image.
    pub fn commit(mut self, expected_sha256: [u8; 32]) -> Result<(), StoreError> {
        if self.received_size != self.expected_size {
            return Err(StoreError::IncompleteImage);
        }
        self.store.check_generation(self.vm, self.generation)?;
        let handle = self.handle.ok_or(StoreError::StagingObjectMissing)?;
        self.store.verify_staging(handle, self.expected_size, expected_sha256)?;
        self.store.commit_staging(handle, self.vm, self.generation, &self.partition)?;
        self.handle = None;
        Ok(())
    }

    pub fn received_bytes(&self) -> u64 {
        self.received_size
    }

    pub fn expected_bytes(&self) -> u64 {
        self.expected_size
    }

    pub fn abort(mut self) -> Result<(), StoreError> {
        if let Some(handle) = self.handle.take() {
            self.store.abort_staging(handle)?;
        }
        Ok(())
    }
}

impl<S: PartitionStore> Drop for FlashTransaction<'_, S> {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            // Best effort only. Durable recovery must also sweep abandoned
            // staging records after process death/reboot.
            let _ = self.store.abort_staging(handle);
        }
    }
}

pub fn erase_partition<S: PartitionStore>(
    store: &mut S,
    vm: VmId,
    generation: u64,
    partition: &str,
) -> Result<(), StoreError> {
    validate_partition_name(partition)?;
    store.check_generation(vm, generation)?;
    let info = store.partition_info(vm, partition)?;
    if !info.erasable {
        return Err(StoreError::NotErasable);
    }
    store.erase_partition_atomic(vm, generation, partition)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct MemoryStore {
        generation: u64,
        partitions: BTreeMap<String, Vec<u8>>,
        staging: BTreeMap<u64, (String, Vec<u8>, u64)>,
        next_handle: u64,
        fail_commit: bool,
        last_vm: Option<VmId>,
    }

    impl MemoryStore {
        fn with_partition(name: &str, bytes: Vec<u8>) -> Self {
            let mut value = Self { generation: 7, ..Self::default() };
            value.partitions.insert(name.to_owned(), bytes);
            value
        }
    }

    impl PartitionStore for MemoryStore {
        type Handle = u64;

        fn check_generation(&self, _vm: VmId, generation: u64) -> Result<(), StoreError> {
            if generation == self.generation { Ok(()) } else { Err(StoreError::StaleVmGeneration) }
        }

        fn partition_info(&self, _vm: VmId, partition: &str) -> Result<PartitionInfo, StoreError> {
            let bytes = self.partitions.get(partition).ok_or(StoreError::UnknownPartition)?;
            Ok(PartitionInfo {
                capacity_bytes: 4096.max(bytes.len() as u64),
                flashable: partition != "system",
                erasable: partition != "system",
            })
        }

        fn begin_staging(&mut self, vm: VmId, generation: u64, partition: &str, expected_size: u64) -> Result<u64, StoreError> {
            self.check_generation(vm, generation)?;
            let id = self.next_handle.checked_add(1).ok_or(StoreError::BackendFailure)?;
            self.next_handle = id;
            self.staging.insert(id, (partition.to_owned(), Vec::new(), expected_size));
            self.last_vm = Some(vm);
            Ok(id)
        }

        fn write_staging(&mut self, handle: u64, offset: u64, bytes: &[u8]) -> Result<(), StoreError> {
            let (_, data, expected) = self.staging.get_mut(&handle).ok_or(StoreError::StagingObjectMissing)?;
            if offset != data.len() as u64 || data.len().checked_add(bytes.len()).ok_or(StoreError::WriteOverflow)? as u64 > *expected {
                return Err(StoreError::WriteOverflow);
            }
            data.extend_from_slice(bytes);
            Ok(())
        }

        fn verify_staging(&mut self, handle: u64, expected_size: u64, _expected_sha256: [u8; 32]) -> Result<(), StoreError> {
            let (_, data, expected) = self.staging.get(&handle).ok_or(StoreError::StagingObjectMissing)?;
            if *expected != expected_size || data.len() as u64 != expected_size {
                return Err(StoreError::IncompleteImage);
            }
            if expected_sha256 != [0; 32] {
                return Err(StoreError::DigestMismatch);
            }
            Ok(())
        }

        fn commit_staging(&mut self, handle: u64, vm: VmId, generation: u64, partition: &str) -> Result<(), StoreError> {
            self.check_generation(vm, generation)?;
            if self.fail_commit { return Err(StoreError::BackendFailure); }
            let (staged_partition, data, _) = self.staging.remove(&handle).ok_or(StoreError::StagingObjectMissing)?;
            if staged_partition != partition { return Err(StoreError::BackendFailure); }
            self.partitions.insert(partition.to_owned(), data);
            Ok(())
        }

        fn abort_staging(&mut self, handle: u64) -> Result<(), StoreError> {
            self.staging.remove(&handle);
            Ok(())
        }

        fn erase_partition_atomic(&mut self, vm: VmId, generation: u64, partition: &str) -> Result<(), StoreError> {
            self.check_generation(vm, generation)?;
            self.partitions.get_mut(partition).ok_or(StoreError::UnknownPartition)?.clear();
            Ok(())
        }
    }

    fn policy() -> FlashPolicy {
        FlashPolicy { max_image_bytes: 4096, max_chunk_bytes: 4 }
    }

    #[test]
    fn streams_and_atomically_commits_complete_image() {
        let vm = VmId(42);
        let mut store = MemoryStore::with_partition("boot_a", vec![9; 16]);
        let mut tx = FlashTransaction::begin(&mut store, vm, 7, "boot_a", 6, policy()).unwrap();
        assert_eq!(tx.write_chunk(0, &[1, 2, 3]), Ok(3));
        assert_eq!(tx.write_chunk(3, &[4, 5, 6]), Ok(0));
        tx.commit([0; 32]).unwrap();
        assert_eq!(store.partitions["boot_a"], vec![1, 2, 3, 4, 5, 6]);
        assert!(store.staging.is_empty());
    }

    #[test]
    fn rejects_bad_offsets_oversized_chunks_and_incomplete_commit() {
        let mut store = MemoryStore::with_partition("boot_a", vec![9; 8]);
        let mut tx = FlashTransaction::begin(&mut store, VmId(1), 7, "boot_a", 4, policy()).unwrap();
        assert_eq!(tx.write_chunk(1, &[1]), Err(StoreError::InvalidWriteOffset));
        assert_eq!(tx.write_chunk(0, &[1, 2, 3, 4, 5]), Err(StoreError::InvalidChunkSize));
        assert_eq!(tx.write_chunk(0, &[1, 2]), Ok(2));
        assert_eq!(tx.commit([0; 32]), Err(StoreError::IncompleteImage));
        assert!(store.staging.is_empty());
        assert_eq!(store.partitions["boot_a"], vec![9; 8]);
    }

    #[test]
    fn failed_commit_keeps_previous_partition_and_drop_aborts_stage() {
        let mut store = MemoryStore::with_partition("boot_a", vec![8; 8]);
        store.fail_commit = true;
        {
            let mut tx = FlashTransaction::begin(&mut store, VmId(2), 7, "boot_a", 2, policy()).unwrap();
            tx.write_chunk(0, &[1, 2]).unwrap();
            assert_eq!(tx.commit([0; 32]), Err(StoreError::BackendFailure));
        }
        assert_eq!(store.partitions["boot_a"], vec![8; 8]);
        assert!(store.staging.is_empty());
    }

    #[test]
    fn digest_mismatch_aborts_staging_and_preserves_old_partition() {
        let mut store = MemoryStore::with_partition("boot_a", vec![0xaa; 8]);
        let mut tx = FlashTransaction::begin(&mut store, VmId(9), 7, "boot_a", 2, policy()).unwrap();
        tx.write_chunk(0, &[1, 2]).unwrap();
        assert_eq!(tx.commit([1; 32]), Err(StoreError::DigestMismatch));
        assert_eq!(store.partitions["boot_a"], vec![0xaa; 8]);
        assert!(store.staging.is_empty());
    }

    #[test]
    fn generation_and_partition_policy_are_enforced() {
        let mut store = MemoryStore::with_partition("system", vec![1; 8]);
        assert_eq!(FlashTransaction::begin(&mut store, VmId(1), 6, "system", 4, policy()).err(), Some(StoreError::StaleVmGeneration));
        assert_eq!(FlashTransaction::begin(&mut store, VmId(1), 7, "system", 4, policy()).err(), Some(StoreError::NotFlashable));
    }

    #[test]
    fn erase_is_capability_gated_and_never_uses_host_paths() {
        let mut store = MemoryStore::with_partition("system", vec![1; 8]);
        assert_eq!(erase_partition(&mut store, VmId(3), 7, "system"), Err(StoreError::NotErasable));
        assert_eq!(erase_partition(&mut store, VmId(3), 7, "../boot"), Err(StoreError::InvalidPartitionName));
    }
}
