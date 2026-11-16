use std::collections::{BTreeMap, BTreeSet};

use crate::{
    MutationError, StorageMutation, HARD_MAX_NAME_BYTES, HARD_MAX_OBJECT_CAPACITY,
    HARD_MAX_PAGE_INDEX, PAGE_SIZE,
};

pub const HARD_MAX_REPLAY_VMS: usize = 64;
pub const HARD_MAX_REPLAY_OBJECTS_PER_VM: usize = 1024;
pub const HARD_MAX_REPLAY_TRACKED_PAGES_PER_VM: usize = 262_144;
pub const HARD_MAX_REPLAY_OBJECT_IDS: usize = 65_536;
pub const HARD_MAX_REPLAY_VM_QUOTA: u64 = HARD_MAX_OBJECT_CAPACITY * 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayError {
    Mutation(MutationError),
    InvalidTransaction,
    SequenceRegression,
    VmNotRegistered,
    VmLimitExceeded,
    StaleGeneration,
    ObjectNotFound,
    DuplicateObject,
    DuplicateObjectId,
    ObjectLimitExceeded,
    ObjectIdLimitExceeded,
    InvalidObjectCapacity,
    InvalidName,
    InvalidPageIndex,
    InvalidPageLength,
    QuotaExceeded,
    PageLimitExceeded,
    IntegerOverflow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ObjectState {
    name: String,
    capacity_bytes: u64,
    allocated_pages: BTreeSet<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VmState {
    generation: u64,
    quota_bytes: u64,
    objects: BTreeMap<u128, ObjectState>,
    names: BTreeMap<String, u128>,
    used_object_ids: BTreeSet<u128>,
    allocated_pages: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReplayState {
    vms: BTreeMap<u128, VmState>,
    last_commit_sequence: Option<u64>,
}

impl ReplayState {
    pub fn new() -> Self { Self::default() }

    pub fn last_commit_sequence(&self) -> Option<u64> {
        self.last_commit_sequence
    }

    pub fn vm_count(&self) -> usize { self.vms.len() }

    pub fn allocated_bytes(&self, vm_id: u128) -> Result<u64, ReplayError> {
        let vm = self.vms.get(&vm_id).ok_or(ReplayError::VmNotRegistered)?;
        (vm.allocated_pages as u64)
            .checked_mul(PAGE_SIZE as u64)
            .ok_or(ReplayError::IntegerOverflow)
    }

    pub fn object_count(&self, vm_id: u128) -> Result<usize, ReplayError> {
        Ok(self.vms.get(&vm_id).ok_or(ReplayError::VmNotRegistered)?.objects.len())
    }

    /// Applies one committed journal transaction after decoding its typed mutations.
    pub fn apply_committed_transaction(
        &mut self,
        transaction: &zerodroid_storage_journal::CommittedTransaction,
    ) -> Result<(), ReplayError> {
        let decoded = crate::decode_transaction(transaction).map_err(ReplayError::Mutation)?;
        self.apply_transaction(&decoded)
    }

    /// Rebuilds state from a complete recovery report atomically. If any committed
    /// transaction is invalid, the original state and commit sequence are retained.
    pub fn replay_report(
        &mut self,
        report: &zerodroid_storage_journal::RecoveryReport,
    ) -> Result<(), ReplayError> {
        let mut candidate = self.clone();
        for transaction in &report.committed_transactions {
            candidate.apply_committed_transaction(transaction)?;
        }
        *self = candidate;
        Ok(())
    }

    /// Applies one already-committed transaction atomically to the replay index.
    /// Validation happens against a private VM-state copy; no state is published
    /// when any mutation fails. This is an in-memory replay planner, not durable I/O.
    pub fn apply_transaction(
        &mut self,
        tx: &crate::DecodedTransaction,
    ) -> Result<(), ReplayError> {
        if tx.mutations.is_empty() || tx.vm_id == 0 || tx.generation == 0 {
            return Err(ReplayError::InvalidTransaction);
        }
        if self.last_commit_sequence.is_some_and(|last| tx.commit_sequence <= last) {
            return Err(ReplayError::SequenceRegression);
        }
        if tx.mutations.iter().any(|m| m.vm_scope() != (tx.vm_id, tx.generation)) {
            return Err(ReplayError::InvalidTransaction);
        }

        let mut candidate = self.vms.get(&tx.vm_id).cloned();
        for mutation in &tx.mutations {
            match mutation {
                StorageMutation::VmRegister { vm_id, generation, quota_bytes } => {
                    if *quota_bytes == 0 || *quota_bytes > HARD_MAX_REPLAY_VM_QUOTA {
                        return Err(ReplayError::QuotaExceeded);
                    }
                    match candidate.as_mut() {
                        Some(vm) if vm.generation == *generation => {
                            let used = (vm.allocated_pages as u64)
                                .checked_mul(PAGE_SIZE as u64)
                                .ok_or(ReplayError::IntegerOverflow)?;
                            if *quota_bytes < used { return Err(ReplayError::QuotaExceeded); }
                            vm.quota_bytes = *quota_bytes;
                        }
                        _ => {
                            if !self.vms.contains_key(vm_id) && self.vms.len() >= HARD_MAX_REPLAY_VMS {
                                return Err(ReplayError::VmLimitExceeded);
                            }
                            candidate = Some(VmState {
                                generation: *generation,
                                quota_bytes: *quota_bytes,
                                objects: BTreeMap::new(),
                                names: BTreeMap::new(),
                                used_object_ids: BTreeSet::new(),
                                allocated_pages: 0,
                            });
                        }
                    }
                }
                StorageMutation::ObjectCreate { object_id, capacity_bytes, name, .. } => {
                    let vm = candidate.as_mut().ok_or(ReplayError::VmNotRegistered)?;
                    if vm.generation != tx.generation { return Err(ReplayError::StaleGeneration); }
                    validate_name(name)?;
                    if *object_id == 0 || vm.used_object_ids.contains(object_id) {
                        return Err(ReplayError::DuplicateObjectId);
                    }
                    if vm.names.contains_key(name) { return Err(ReplayError::DuplicateObject); }
                    if *capacity_bytes == 0 || *capacity_bytes > HARD_MAX_OBJECT_CAPACITY {
                        return Err(ReplayError::InvalidObjectCapacity);
                    }
                    if vm.objects.len() >= HARD_MAX_REPLAY_OBJECTS_PER_VM {
                        return Err(ReplayError::ObjectLimitExceeded);
                    }
                    // Count IDs from every other VM plus the candidate VM's
                    // current generation. A generation reset may make the
                    // candidate set smaller than the old VM set, so subtract
                    // with saturation rather than unsigned arithmetic.
                    let total_used_ids = self.vms.values()
                        .map(|state| state.used_object_ids.len())
                        .sum::<usize>();
                    let replaced_vm_ids = self.vms.get(&tx.vm_id)
                        .map_or(0, |state| state.used_object_ids.len());
                    let other_vm_ids = total_used_ids.saturating_sub(replaced_vm_ids);
                    if other_vm_ids.saturating_add(vm.used_object_ids.len())
                        >= HARD_MAX_REPLAY_OBJECT_IDS
                    {
                        return Err(ReplayError::ObjectIdLimitExceeded);
                    }
                    vm.used_object_ids.insert(*object_id);
                    vm.names.insert(name.clone(), *object_id);
                    vm.objects.insert(*object_id, ObjectState {
                        name: name.clone(),
                        capacity_bytes: *capacity_bytes,
                        allocated_pages: BTreeSet::new(),
                    });
                }
                StorageMutation::ObjectDelete { object_id, .. } => {
                    let vm = candidate.as_mut().ok_or(ReplayError::VmNotRegistered)?;
                    if vm.generation != tx.generation { return Err(ReplayError::StaleGeneration); }
                    let object = vm.objects.remove(object_id).ok_or(ReplayError::ObjectNotFound)?;
                    vm.names.remove(&object.name);
                    vm.allocated_pages = vm.allocated_pages.checked_sub(object.allocated_pages.len())
                        .ok_or(ReplayError::IntegerOverflow)?;
                }
                StorageMutation::PagePut { object_id, page_index, page, .. } => {
                    let vm = candidate.as_mut().ok_or(ReplayError::VmNotRegistered)?;
                    if vm.generation != tx.generation { return Err(ReplayError::StaleGeneration); }
                    if page.len() != PAGE_SIZE { return Err(ReplayError::InvalidPageLength); }
                    if *page_index > HARD_MAX_PAGE_INDEX { return Err(ReplayError::InvalidPageIndex); }
                    let object = vm.objects.get_mut(object_id).ok_or(ReplayError::ObjectNotFound)?;
                    let page_count = object.capacity_bytes.checked_add(PAGE_SIZE as u64 - 1)
                        .ok_or(ReplayError::IntegerOverflow)? / PAGE_SIZE as u64;
                    if *page_index >= page_count { return Err(ReplayError::InvalidPageIndex); }
                    if !object.allocated_pages.contains(page_index) {
                        if vm.allocated_pages >= HARD_MAX_REPLAY_TRACKED_PAGES_PER_VM {
                            return Err(ReplayError::PageLimitExceeded);
                        }
                        let next_bytes = (vm.allocated_pages as u64)
                            .checked_add(1).and_then(|n| n.checked_mul(PAGE_SIZE as u64))
                            .ok_or(ReplayError::IntegerOverflow)?;
                        if next_bytes > vm.quota_bytes { return Err(ReplayError::QuotaExceeded); }
                        object.allocated_pages.insert(*page_index);
                        vm.allocated_pages += 1;
                    }
                }
                StorageMutation::PageDelete { object_id, page_index, .. } => {
                    let vm = candidate.as_mut().ok_or(ReplayError::VmNotRegistered)?;
                    if vm.generation != tx.generation { return Err(ReplayError::StaleGeneration); }
                    let object = vm.objects.get_mut(object_id).ok_or(ReplayError::ObjectNotFound)?;
                    if !object.allocated_pages.remove(page_index) {
                        return Err(ReplayError::InvalidPageIndex);
                    }
                    vm.allocated_pages = vm.allocated_pages.checked_sub(1).ok_or(ReplayError::IntegerOverflow)?;
                }
            }
        }
        let next = candidate.ok_or(ReplayError::VmNotRegistered)?;
        self.vms.insert(tx.vm_id, next);
        self.last_commit_sequence = Some(tx.commit_sequence);
        Ok(())
    }
}

fn validate_name(name: &str) -> Result<(), ReplayError> {
    if name.is_empty() || name.len() > HARD_MAX_NAME_BYTES
        || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.')
    { Err(ReplayError::InvalidName) } else { Ok(()) }
}

#[cfg(test)]
mod replay_tests {
    use super::*;
    use crate::{decode_transaction, encode, StorageMutation};
    use zerodroid_storage_journal::CommittedTransaction;

    fn tx(sequence: u64, mutations: Vec<StorageMutation>) -> crate::DecodedTransaction {
        let payloads = mutations.iter().map(|m| encode(m).unwrap()).collect();
        decode_transaction(&CommittedTransaction {
            transaction_id: sequence as u128,
            mutations: payloads,
            commit_sequence: sequence,
        }).unwrap()
    }

    #[test]
    fn journal_recovery_report_rebuilds_typed_storage_state() {
        use zerodroid_storage_journal::{encode as encode_record, recover_report, JournalRecord, RecordKind};
        let mutations = vec![
            encode(&StorageMutation::VmRegister { vm_id: 41, generation: 3, quota_bytes: 8192 }).unwrap(),
            encode(&StorageMutation::ObjectCreate { vm_id: 41, generation: 3, object_id: 9, capacity_bytes: 8192, name: "super".into() }).unwrap(),
        ];
        let mut log = encode_record(&JournalRecord { sequence: 1, transaction_id: 77, kind: RecordKind::Begin, payload: vec![] }).unwrap();
        for (index, payload) in mutations.into_iter().enumerate() {
            log.extend(encode_record(&JournalRecord {
                sequence: index as u64 + 2, transaction_id: 77,
                kind: RecordKind::Mutation, payload,
            }).unwrap());
        }
        log.extend(encode_record(&JournalRecord { sequence: 4, transaction_id: 77, kind: RecordKind::Commit, payload: vec![] }).unwrap());

        let report = recover_report(&log).unwrap();
        let mut state = ReplayState::new();
        state.replay_report(&report).unwrap();
        assert_eq!(state.vm_count(), 1);
        assert_eq!(state.object_count(41).unwrap(), 1);
        assert_eq!(state.last_commit_sequence(), Some(4));
    }

    #[test]
    fn replay_report_is_atomic_when_a_later_transaction_is_invalid() {
        use zerodroid_storage_journal::{encode as encode_record, recover_report, JournalRecord, RecordKind};
        let register = encode(&StorageMutation::VmRegister { vm_id: 41, generation: 3, quota_bytes: 4096 }).unwrap();
        let invalid_page = encode(&StorageMutation::PagePut {
            vm_id: 41, generation: 3, object_id: 99, page_index: 0, page: vec![1; PAGE_SIZE],
        }).unwrap();
        let mut log = Vec::new();
        for (sequence, transaction_id, kind, payload) in [
            (1, 10, RecordKind::Begin, vec![]),
            (2, 10, RecordKind::Mutation, register),
            (3, 10, RecordKind::Commit, vec![]),
            (4, 11, RecordKind::Begin, vec![]),
            (5, 11, RecordKind::Mutation, invalid_page),
            (6, 11, RecordKind::Commit, vec![]),
        ] {
            log.extend(encode_record(&JournalRecord { sequence, transaction_id, kind, payload }).unwrap());
        }
        let report = recover_report(&log).unwrap();
        let mut state = ReplayState::new();
        assert_eq!(state.replay_report(&report), Err(ReplayError::ObjectNotFound));
        assert_eq!(state.vm_count(), 0);
        assert_eq!(state.last_commit_sequence(), None);
    }

    #[test]
    fn replays_register_create_put_delete_and_release_quota() {
        let mut state = ReplayState::new();
        state.apply_transaction(&tx(1, vec![
            StorageMutation::VmRegister { vm_id: 1, generation: 2, quota_bytes: 8192 },
            StorageMutation::ObjectCreate { vm_id: 1, generation: 2, object_id: 3, capacity_bytes: 8192, name: "super".into() },
        ])).unwrap();
        state.apply_transaction(&tx(2, vec![
            StorageMutation::PagePut { vm_id: 1, generation: 2, object_id: 3, page_index: 0, page: vec![1; PAGE_SIZE] },
            StorageMutation::PagePut { vm_id: 1, generation: 2, object_id: 3, page_index: 1, page: vec![2; PAGE_SIZE] },
        ])).unwrap();
        assert_eq!(state.allocated_bytes(1).unwrap(), 8192);
        state.apply_transaction(&tx(3, vec![
            StorageMutation::PageDelete { vm_id: 1, generation: 2, object_id: 3, page_index: 0 },
        ])).unwrap();
        assert_eq!(state.allocated_bytes(1).unwrap(), 4096);
    }

    #[test]
    fn failed_transaction_does_not_publish_partial_changes() {
        let mut state = ReplayState::new();
        state.apply_transaction(&tx(1, vec![
            StorageMutation::VmRegister { vm_id: 1, generation: 1, quota_bytes: 4096 },
            StorageMutation::ObjectCreate { vm_id: 1, generation: 1, object_id: 2, capacity_bytes: 8192, name: "system".into() },
        ])).unwrap();
        let bad = tx(2, vec![
            StorageMutation::PagePut { vm_id: 1, generation: 1, object_id: 2, page_index: 0, page: vec![1; PAGE_SIZE] },
            StorageMutation::PagePut { vm_id: 1, generation: 1, object_id: 2, page_index: 1, page: vec![2; PAGE_SIZE] },
        ]);
        assert_eq!(state.apply_transaction(&bad), Err(ReplayError::QuotaExceeded));
        assert_eq!(state.allocated_bytes(1).unwrap(), 0);
        assert_eq!(state.last_commit_sequence(), Some(1));
    }

    #[test]
    fn generation_reset_does_not_underflow_global_object_id_accounting() {
        let mut state = ReplayState::new();
        state.apply_transaction(&tx(1, vec![
            StorageMutation::VmRegister { vm_id: 1, generation: 1, quota_bytes: 4096 },
            StorageMutation::ObjectCreate { vm_id: 1, generation: 1, object_id: 7, capacity_bytes: 4096, name: "old".into() },
        ])).unwrap();
        state.apply_transaction(&tx(2, vec![
            StorageMutation::VmRegister { vm_id: 1, generation: 2, quota_bytes: 4096 },
        ])).unwrap();
        state.apply_transaction(&tx(3, vec![
            StorageMutation::ObjectCreate { vm_id: 1, generation: 2, object_id: 8, capacity_bytes: 4096, name: "new".into() },
        ])).unwrap();
        assert_eq!(state.object_count(1).unwrap(), 1);
    }

    #[test]
    fn generation_change_resets_previous_generation_objects() {
        let mut state = ReplayState::new();
        state.apply_transaction(&tx(1, vec![
            StorageMutation::VmRegister { vm_id: 1, generation: 1, quota_bytes: 4096 },
            StorageMutation::ObjectCreate { vm_id: 1, generation: 1, object_id: 7, capacity_bytes: 4096, name: "old".into() },
        ])).unwrap();
        state.apply_transaction(&tx(2, vec![
            StorageMutation::VmRegister { vm_id: 1, generation: 2, quota_bytes: 4096 },
            StorageMutation::ObjectCreate { vm_id: 1, generation: 2, object_id: 7, capacity_bytes: 4096, name: "new".into() },
        ])).unwrap();
        assert_eq!(state.object_count(1).unwrap(), 1);
        assert_eq!(state.allocated_bytes(1).unwrap(), 0);
    }
}
