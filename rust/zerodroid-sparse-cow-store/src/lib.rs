#![forbid(unsafe_code)]
use std::collections::{BTreeMap, BTreeSet};
use zerodroid_block_core::{BlockError, VmId, HARD_MAX_TRANSFER_BYTES};

pub const PAGE_SIZE: usize = 4096;
pub const HARD_MAX_OBJECT_BYTES: u64 = 64 * 1024 * 1024 * 1024;
pub const HARD_MAX_VM_QUOTA_BYTES: u64 = 256 * 1024 * 1024 * 1024;
pub const HARD_MAX_BATCH_PAGES: usize = (HARD_MAX_TRANSFER_BYTES as usize / PAGE_SIZE) + 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjectId(pub u128);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    InvalidName, InvalidCapacity, QuotaExceeded, ObjectNotFound,
    StaleGeneration, TooManyPages, IntegerOverflow, BackendFailure,
}
impl From<StoreError> for BlockError {
    fn from(_: StoreError) -> Self { BlockError::BackendFailure }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageUpdate {
    pub object: ObjectId,
    pub page_index: u64,
    pub data: Option<Vec<u8>>,
}

/// A platform backend must make each commit all-or-nothing and durable.
/// ObjectId is opaque metadata, never a host path or physical partition address.
pub trait SparseCowBackend {
    fn read_page(&mut self, vm: VmId, generation: u64, object: ObjectId, page_index: u64, output: &mut [u8]) -> Result<bool, StoreError>;
    fn commit_pages(&mut self, vm: VmId, generation: u64, updates: &[PageUpdate]) -> Result<(), StoreError>;
    fn flush(&mut self, vm: VmId, generation: u64) -> Result<(), StoreError>;
}

#[derive(Debug, Clone)]
struct ObjectMeta { id: ObjectId, capacity: u64, allocated: BTreeSet<u64> }
#[derive(Debug, Clone, Copy)]
struct VmMeta { generation: u64, quota: u64 }

/// Bounded VM-scoped sparse COW page manager. Registry/index persistence and
/// recovery are intentionally delegated to a later durable metadata layer.
pub struct SparseCowStore<B: SparseCowBackend> {
    backend: B,
    vms: BTreeMap<u128, VmMeta>,
    objects: BTreeMap<(u128, String), ObjectMeta>,
    next_object_id: u128,
}

impl<B: SparseCowBackend> SparseCowStore<B> {
    pub fn new(backend: B) -> Self {
        Self { backend, vms: BTreeMap::new(), objects: BTreeMap::new(), next_object_id: 1 }
    }

    pub fn register_vm(&mut self, vm: VmId, generation: u64, quota_bytes: u64) -> Result<(), StoreError> {
        if quota_bytes == 0 || quota_bytes > HARD_MAX_VM_QUOTA_BYTES { return Err(StoreError::QuotaExceeded); }
        if self.vms.get(&vm.0).is_some_and(|current| current.generation != generation) {
            // Old-generation objects become unreachable immediately. Their opaque
            // backing pages are orphaned for backend garbage collection.
            self.objects.retain(|(owner, _), _| *owner != vm.0);
        }
        self.vms.insert(vm.0, VmMeta { generation, quota: quota_bytes });
        Ok(())
    }

    pub fn create_object(&mut self, vm: VmId, generation: u64, name: &str, capacity: u64) -> Result<ObjectId, StoreError> {
        self.check_vm(vm, generation)?;
        if !valid_name(name) { return Err(StoreError::InvalidName); }
        if capacity == 0 || capacity > HARD_MAX_OBJECT_BYTES { return Err(StoreError::InvalidCapacity); }
        let key = (vm.0, name.to_owned());
        if self.objects.contains_key(&key) { return Err(StoreError::InvalidName); }
        let id = ObjectId(self.next_object_id);
        self.next_object_id = self.next_object_id.checked_add(1).ok_or(StoreError::IntegerOverflow)?;
        self.objects.insert(key, ObjectMeta { id, capacity, allocated: BTreeSet::new() });
        Ok(id)
    }

    pub fn read(&mut self, vm: VmId, generation: u64, name: &str, offset: u64, output: &mut [u8]) -> Result<(), StoreError> {
        self.check_vm(vm, generation)?;
        validate_io(offset, output.len())?;
        let meta = self.object(vm, name)?.clone();
        let end = offset.checked_add(output.len() as u64).ok_or(StoreError::IntegerOverflow)?;
        if end > meta.capacity { return Err(StoreError::InvalidCapacity); }
        output.fill(0);
        let mut cursor = 0usize;
        while cursor < output.len() {
            let absolute = offset + cursor as u64;
            let page = absolute / PAGE_SIZE as u64;
            let in_page = (absolute % PAGE_SIZE as u64) as usize;
            let count = (PAGE_SIZE - in_page).min(output.len() - cursor);
            if meta.allocated.contains(&page) {
                let mut bytes = vec![0; PAGE_SIZE];
                if !self.backend.read_page(vm, generation, meta.id, page, &mut bytes)? {
                    return Err(StoreError::BackendFailure);
                }
                output[cursor..cursor + count].copy_from_slice(&bytes[in_page..in_page + count]);
            }
            cursor += count;
        }
        Ok(())
    }

    pub fn write(&mut self, vm: VmId, generation: u64, name: &str, offset: u64, input: &[u8]) -> Result<(), StoreError> {
        self.check_vm(vm, generation)?;
        validate_io(offset, input.len())?;
        let meta = self.object(vm, name)?.clone();
        let end = offset.checked_add(input.len() as u64).ok_or(StoreError::IntegerOverflow)?;
        if end > meta.capacity { return Err(StoreError::InvalidCapacity); }
        let first = offset / PAGE_SIZE as u64;
        let last = (end - 1) / PAGE_SIZE as u64;
        let page_count = usize::try_from(last - first + 1).map_err(|_| StoreError::IntegerOverflow)?;
        if page_count > HARD_MAX_BATCH_PAGES { return Err(StoreError::TooManyPages); }
        let new_pages = (first..=last).filter(|p| !meta.allocated.contains(p)).count() as u64;
        let vm_meta = *self.vms.get(&vm.0).ok_or(StoreError::StaleGeneration)?;
        let additional = new_pages.checked_mul(PAGE_SIZE as u64).ok_or(StoreError::IntegerOverflow)?;
        if self.allocated_bytes(vm).checked_add(additional).ok_or(StoreError::IntegerOverflow)? > vm_meta.quota {
            return Err(StoreError::QuotaExceeded);
        }

        let mut updates = Vec::with_capacity(page_count);
        let mut consumed = 0usize;
        for page in first..=last {
            let mut bytes = vec![0; PAGE_SIZE];
            if meta.allocated.contains(&page) {
                if !self.backend.read_page(vm, generation, meta.id, page, &mut bytes)? {
                    return Err(StoreError::BackendFailure);
                }
            }
            let page_start = page * PAGE_SIZE as u64;
            let write_start = offset.max(page_start);
            let write_end = end.min(page_start + PAGE_SIZE as u64);
            let count = (write_end - write_start) as usize;
            let in_page = (write_start - page_start) as usize;
            bytes[in_page..in_page + count].copy_from_slice(&input[consumed..consumed + count]);
            consumed += count;
            updates.push(PageUpdate { object: meta.id, page_index: page, data: Some(bytes) });
        }
        self.backend.commit_pages(vm, generation, &updates)?;
        let stored = self.objects.get_mut(&(vm.0, name.to_owned())).ok_or(StoreError::ObjectNotFound)?;
        for update in updates { stored.allocated.insert(update.page_index); }
        Ok(())
    }

    pub fn discard(&mut self, vm: VmId, generation: u64, name: &str, offset: u64, length: u64) -> Result<(), StoreError> {
        self.check_vm(vm, generation)?;
        if length == 0 || offset % PAGE_SIZE as u64 != 0 || length % PAGE_SIZE as u64 != 0 {
            return Err(StoreError::InvalidCapacity);
        }
        if length > HARD_MAX_TRANSFER_BYTES as u64 { return Err(StoreError::TooManyPages); }
        let meta = self.object(vm, name)?.clone();
        let end = offset.checked_add(length).ok_or(StoreError::IntegerOverflow)?;
        if end > meta.capacity { return Err(StoreError::InvalidCapacity); }
        let updates: Vec<_> = (offset / PAGE_SIZE as u64..end / PAGE_SIZE as u64)
            .filter(|p| meta.allocated.contains(p))
            .map(|page_index| PageUpdate { object: meta.id, page_index, data: None }).collect();
        if updates.is_empty() { return Ok(()); }
        self.backend.commit_pages(vm, generation, &updates)?;
        let stored = self.objects.get_mut(&(vm.0, name.to_owned())).ok_or(StoreError::ObjectNotFound)?;
        for update in updates { stored.allocated.remove(&update.page_index); }
        Ok(())
    }

    pub fn flush(&mut self, vm: VmId, generation: u64) -> Result<(), StoreError> {
        self.check_vm(vm, generation)?;
        self.backend.flush(vm, generation)
    }

    pub fn allocated_bytes(&self, vm: VmId) -> u64 {
        self.objects.iter().filter(|((id, _), _)| *id == vm.0)
            .map(|(_, meta)| meta.allocated.len() as u64 * PAGE_SIZE as u64).sum()
    }
    pub fn into_backend(self) -> B { self.backend }
    fn check_vm(&self, vm: VmId, generation: u64) -> Result<(), StoreError> {
        match self.vms.get(&vm.0) {
            Some(meta) if meta.generation == generation => Ok(()),
            _ => Err(StoreError::StaleGeneration),
        }
    }
    fn object(&self, vm: VmId, name: &str) -> Result<&ObjectMeta, StoreError> {
        self.objects.get(&(vm.0, name.to_owned())).ok_or(StoreError::ObjectNotFound)
    }
}

fn validate_io(offset: u64, length: usize) -> Result<(), StoreError> {
    if length == 0 { return Err(StoreError::InvalidCapacity); }
    if length > HARD_MAX_TRANSFER_BYTES as usize { return Err(StoreError::TooManyPages); }
    offset.checked_add(length as u64).ok_or(StoreError::IntegerOverflow)?;
    Ok(())
}
fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 96 &&
        name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.')
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct MemoryBackend { pages: BTreeMap<(u128, u128, u64), Vec<u8>>, flushes: u32, fail_commit: bool }
    impl SparseCowBackend for MemoryBackend {
        fn read_page(&mut self, vm: VmId, generation: u64, object: ObjectId, page: u64, out: &mut [u8]) -> Result<bool, StoreError> {
            if generation != 7 { return Err(StoreError::StaleGeneration); }
            if let Some(bytes) = self.pages.get(&(vm.0, object.0, page)) { out.copy_from_slice(bytes); Ok(true) }
            else { out.fill(0); Ok(false) }
        }
        fn commit_pages(&mut self, vm: VmId, generation: u64, updates: &[PageUpdate]) -> Result<(), StoreError> {
            if generation != 7 { return Err(StoreError::StaleGeneration); }
            if self.fail_commit { return Err(StoreError::BackendFailure); }
            let mut next = self.pages.clone();
            for u in updates {
                match &u.data {
                    Some(data) if data.len() == PAGE_SIZE => { next.insert((vm.0, u.object.0, u.page_index), data.clone()); }
                    Some(_) => return Err(StoreError::BackendFailure),
                    None => { next.remove(&(vm.0, u.object.0, u.page_index)); }
                }
            }
            self.pages = next; Ok(())
        }
        fn flush(&mut self, _: VmId, generation: u64) -> Result<(), StoreError> {
            if generation != 7 { return Err(StoreError::StaleGeneration); }
            self.flushes += 1; Ok(())
        }
    }
    fn setup(quota: u64) -> SparseCowStore<MemoryBackend> {
        let mut s = SparseCowStore::new(MemoryBackend::default());
        s.register_vm(VmId(1), 7, quota).unwrap();
        s.create_object(VmId(1), 7, "super", PAGE_SIZE as u64 * 8).unwrap(); s
    }
    #[test]
    fn sparse_zero_reads_and_partial_write_round_trip() {
        let mut s = setup(PAGE_SIZE as u64 * 4);
        let mut out = [0xff; 16]; s.read(VmId(1), 7, "super", 10, &mut out).unwrap();
        assert_eq!(out, [0; 16]);
        s.write(VmId(1), 7, "super", 10, &[0x5a; 16]).unwrap();
        let mut got = [0; 16]; s.read(VmId(1), 7, "super", 10, &mut got).unwrap();
        assert_eq!(got, [0x5a; 16]); assert_eq!(s.allocated_bytes(VmId(1)), PAGE_SIZE as u64);
    }
    #[test]
    fn quota_is_checked_before_atomic_commit() {
        let mut s = setup(PAGE_SIZE as u64);
        s.write(VmId(1), 7, "super", 0, &[1; PAGE_SIZE]).unwrap();
        assert_eq!(s.write(VmId(1), 7, "super", PAGE_SIZE as u64, &[2; PAGE_SIZE]), Err(StoreError::QuotaExceeded));
        assert_eq!(s.allocated_bytes(VmId(1)), PAGE_SIZE as u64);
    }
    #[test]
    fn discard_releases_pages_and_restores_zero_reads() {
        let mut s = setup(PAGE_SIZE as u64 * 2);
        s.write(VmId(1), 7, "super", 0, &[3; PAGE_SIZE * 2]).unwrap();
        s.discard(VmId(1), 7, "super", 0, PAGE_SIZE as u64).unwrap();
        let mut out = [0xff; 8]; s.read(VmId(1), 7, "super", 0, &mut out).unwrap();
        assert_eq!(out, [0; 8]); assert_eq!(s.allocated_bytes(VmId(1)), PAGE_SIZE as u64);
    }
    #[test]
    fn stale_generation_and_failed_commit_do_not_publish_index() {
        let mut s = setup(PAGE_SIZE as u64 * 2);
        assert_eq!(s.read(VmId(1), 6, "super", 0, &mut [0; 1]), Err(StoreError::StaleGeneration));
        s.backend.fail_commit = true;
        assert_eq!(s.write(VmId(1), 7, "super", 0, &[4; 32]), Err(StoreError::BackendFailure));
        assert_eq!(s.allocated_bytes(VmId(1)), 0);
    }
    #[test]
    fn flush_is_forwarded() {
        let mut s = setup(PAGE_SIZE as u64); s.flush(VmId(1), 7).unwrap();
        assert_eq!(s.backend.flushes, 1);
    }
    #[test]
    fn rejects_unsafe_names_and_oversized_objects() {
        let mut s = setup(PAGE_SIZE as u64);
        assert_eq!(s.create_object(VmId(1), 7, "../physical", 4096), Err(StoreError::InvalidName));
        assert_eq!(s.create_object(VmId(1), 7, "too-large", HARD_MAX_OBJECT_BYTES + 1), Err(StoreError::InvalidCapacity));
    }
}
