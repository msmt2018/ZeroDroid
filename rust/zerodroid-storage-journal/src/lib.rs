#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};

pub const MAGIC: [u8; 4] = *b"ZDJR";
pub const FORMAT_VERSION: u16 = 1;
pub const HEADER_BYTES: usize = 40;
pub const HARD_MAX_LOG_BYTES: usize = 64 * 1024 * 1024;
pub const HARD_MAX_RECORD_PAYLOAD: usize = 1024 * 1024;
pub const HARD_MAX_RECORDS: usize = 65_536;
pub const HARD_MAX_RECOVERED_MUTATION_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum RecordKind { Begin = 1, Mutation = 2, Commit = 3, Abort = 4 }

impl TryFrom<u16> for RecordKind {
    type Error = JournalError;
    fn try_from(value: u16) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Begin), 2 => Ok(Self::Mutation),
            3 => Ok(Self::Commit), 4 => Ok(Self::Abort),
            _ => Err(JournalError::UnknownKind),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalError {
    LogTooLarge, TooManyRecords, PayloadTooLarge, RecoveryBudgetExceeded,
    TruncatedRecord, BadMagic, UnsupportedVersion, UnknownKind, ChecksumMismatch,
    SequenceRegression, DuplicateTransactionId, InvalidTransaction, IntegerOverflow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalRecord {
    pub sequence: u64,
    pub transaction_id: u128,
    pub kind: RecordKind,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommittedTransaction {
    pub transaction_id: u128,
    pub mutations: Vec<Vec<u8>>,
    pub commit_sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryReport {
    pub committed_transactions: Vec<CommittedTransaction>,
    pub valid_bytes: usize,
    pub ignored_tail_bytes: usize,
    pub last_sequence: Option<u64>,
    pub scanned_records: usize,
}

pub fn encode(record: &JournalRecord) -> Result<Vec<u8>, JournalError> {
    if record.payload.len() > HARD_MAX_RECORD_PAYLOAD { return Err(JournalError::PayloadTooLarge); }
    let payload_len = u32::try_from(record.payload.len()).map_err(|_| JournalError::IntegerOverflow)?;
    let mut output = Vec::with_capacity(HEADER_BYTES + record.payload.len());
    output.extend_from_slice(&MAGIC);
    output.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    output.extend_from_slice(&(record.kind as u16).to_le_bytes());
    output.extend_from_slice(&record.sequence.to_le_bytes());
    output.extend_from_slice(&record.transaction_id.to_le_bytes());
    output.extend_from_slice(&payload_len.to_le_bytes());
    let checksum = crc32_parts(&output, &record.payload);
    output.extend_from_slice(&checksum.to_le_bytes());
    output.extend_from_slice(&record.payload);
    if output.len() > HARD_MAX_LOG_BYTES { return Err(JournalError::LogTooLarge); }
    Ok(output)
}

/// Compatibility API. Use recover_report when scan diagnostics are required.
pub fn recover(bytes: &[u8]) -> Result<Vec<CommittedTransaction>, JournalError> {
    Ok(recover_report(bytes)?.committed_transactions)
}

/// Scans a bounded journal. An incomplete final header or payload is reported as
/// an ignored torn tail. Complete malformed records fail closed. Transaction IDs
/// cannot be reused within one journal, including after commit or abort.
pub fn recover_report(bytes: &[u8]) -> Result<RecoveryReport, JournalError> {
    if bytes.len() > HARD_MAX_LOG_BYTES { return Err(JournalError::LogTooLarge); }
    let mut cursor = 0usize;
    let mut last_sequence = None;
    let mut open: BTreeMap<u128, Vec<Vec<u8>>> = BTreeMap::new();
    let mut seen_transactions = BTreeSet::new();
    let mut committed_transactions = Vec::new();
    let mut retained_mutation_bytes = 0usize;
    let mut scanned_records = 0usize;

    while cursor < bytes.len() {
        let remaining = bytes.len() - cursor;
        if remaining < HEADER_BYTES { break; }
        let header = &bytes[cursor..cursor + HEADER_BYTES];
        if header[0..4] != MAGIC { return Err(JournalError::BadMagic); }
        let version = u16::from_le_bytes([header[4], header[5]]);
        if version != FORMAT_VERSION { return Err(JournalError::UnsupportedVersion); }
        let kind = RecordKind::try_from(u16::from_le_bytes([header[6], header[7]]))?;
        let sequence = u64::from_le_bytes(header[8..16].try_into().map_err(|_| JournalError::TruncatedRecord)?);
        let transaction_id = u128::from_le_bytes(header[16..32].try_into().map_err(|_| JournalError::TruncatedRecord)?);
        let payload_len = u32::from_le_bytes(header[32..36].try_into().map_err(|_| JournalError::TruncatedRecord)?) as usize;
        let expected_crc = u32::from_le_bytes(header[36..40].try_into().map_err(|_| JournalError::TruncatedRecord)?);
        if payload_len > HARD_MAX_RECORD_PAYLOAD { return Err(JournalError::PayloadTooLarge); }
        let total_len = HEADER_BYTES.checked_add(payload_len).ok_or(JournalError::IntegerOverflow)?;
        if remaining < total_len { break; }
        let payload = &bytes[cursor + HEADER_BYTES..cursor + total_len];
        if crc32_parts(&header[..36], payload) != expected_crc { return Err(JournalError::ChecksumMismatch); }
        if last_sequence.is_some_and(|last| sequence <= last) { return Err(JournalError::SequenceRegression); }
        if scanned_records >= HARD_MAX_RECORDS { return Err(JournalError::TooManyRecords); }
        scanned_records += 1;
        last_sequence = Some(sequence);

        match kind {
            RecordKind::Begin => {
                if !payload.is_empty() { return Err(JournalError::InvalidTransaction); }
                if !seen_transactions.insert(transaction_id) { return Err(JournalError::DuplicateTransactionId); }
                open.insert(transaction_id, Vec::new());
            }
            RecordKind::Mutation => {
                let next_bytes = retained_mutation_bytes.checked_add(payload.len()).ok_or(JournalError::IntegerOverflow)?;
                if next_bytes > HARD_MAX_RECOVERED_MUTATION_BYTES { return Err(JournalError::RecoveryBudgetExceeded); }
                open.get_mut(&transaction_id).ok_or(JournalError::InvalidTransaction)?.push(payload.to_vec());
                retained_mutation_bytes = next_bytes;
            }
            RecordKind::Commit => {
                if !payload.is_empty() { return Err(JournalError::InvalidTransaction); }
                let mutations = open.remove(&transaction_id).ok_or(JournalError::InvalidTransaction)?;
                committed_transactions.push(CommittedTransaction { transaction_id, mutations, commit_sequence: sequence });
            }
            RecordKind::Abort => {
                if !payload.is_empty() { return Err(JournalError::InvalidTransaction); }
                let mutations = open.remove(&transaction_id).ok_or(JournalError::InvalidTransaction)?;
                let released = mutations.iter().try_fold(0usize, |total, mutation| {
                    total.checked_add(mutation.len()).ok_or(JournalError::IntegerOverflow)
                })?;
                retained_mutation_bytes = retained_mutation_bytes.checked_sub(released).ok_or(JournalError::IntegerOverflow)?;
            }
        }
        cursor += total_len;
    }

    Ok(RecoveryReport {
        committed_transactions,
        valid_bytes: cursor,
        ignored_tail_bytes: bytes.len() - cursor,
        last_sequence,
        scanned_records,
    })
}

fn crc32_parts(prefix: &[u8], payload: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in prefix.iter().chain(payload.iter()) {
        crc ^= *byte as u32;
        for _ in 0..8 { crc = (crc >> 1) ^ (0xedb8_8320 & (0u32.wrapping_sub(crc & 1))); }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;
    fn record(sequence: u64, tx: u128, kind: RecordKind, payload: &[u8]) -> Vec<u8> {
        encode(&JournalRecord { sequence, transaction_id: tx, kind, payload: payload.to_vec() }).unwrap()
    }

    #[test]
    fn committed_only_recovery_reports_valid_prefix() {
        let mut log = Vec::new();
        log.extend(record(1, 7, RecordKind::Begin, &[]));
        log.extend(record(2, 7, RecordKind::Mutation, b"page-a"));
        log.extend(record(3, 7, RecordKind::Commit, &[]));
        log.extend(record(4, 8, RecordKind::Begin, &[]));
        log.extend(record(5, 8, RecordKind::Mutation, b"uncommitted"));
        let report = recover_report(&log).unwrap();
        assert_eq!(report.committed_transactions.len(), 1);
        assert_eq!(report.committed_transactions[0].mutations, vec![b"page-a".to_vec()]);
        assert_eq!(report.valid_bytes, log.len());
        assert_eq!(report.ignored_tail_bytes, 0);
        assert_eq!(report.last_sequence, Some(5));
        assert_eq!(report.scanned_records, 5);
    }

    #[test]
    fn torn_header_and_payload_report_exact_valid_offset() {
        let mut log = record(1, 1, RecordKind::Begin, &[]);
        let valid = log.len();
        log.extend_from_slice(&[1, 2, 3]);
        let report = recover_report(&log).unwrap();
        assert_eq!(report.valid_bytes, valid);
        assert_eq!(report.ignored_tail_bytes, 3);

        let mut partial = record(1, 2, RecordKind::Mutation, b"payload");
        partial.truncate(partial.len() - 2);
        let report = recover_report(&partial).unwrap();
        assert_eq!(report.valid_bytes, 0);
        assert_eq!(report.ignored_tail_bytes, partial.len());
    }

    #[test]
    fn rejects_complete_checksum_corruption() {
        let mut bad = record(1, 1, RecordKind::Begin, &[]);
        *bad.last_mut().unwrap() ^= 0x80;
        assert_eq!(recover(&bad), Err(JournalError::ChecksumMismatch));
    }

    #[test]
    fn rejects_sequence_regression_and_invalid_order() {
        let mut log = record(2, 1, RecordKind::Begin, &[]);
        log.extend(record(1, 1, RecordKind::Commit, &[]));
        assert_eq!(recover(&log), Err(JournalError::SequenceRegression));
        assert_eq!(recover(&record(1, 4, RecordKind::Commit, &[])), Err(JournalError::InvalidTransaction));
    }

    #[test]
    fn rejects_transaction_id_reuse_after_commit_or_abort() {
        let mut committed = record(1, 9, RecordKind::Begin, &[]);
        committed.extend(record(2, 9, RecordKind::Commit, &[]));
        committed.extend(record(3, 9, RecordKind::Begin, &[]));
        assert_eq!(recover(&committed), Err(JournalError::DuplicateTransactionId));

        let mut aborted = record(1, 10, RecordKind::Begin, &[]);
        aborted.extend(record(2, 10, RecordKind::Abort, &[]));
        aborted.extend(record(3, 10, RecordKind::Begin, &[]));
        assert_eq!(recover(&aborted), Err(JournalError::DuplicateTransactionId));
    }

    #[test]
    fn crc32_matches_ieee_known_vector() {
        assert_eq!(crc32_parts(b"", b"123456789"), 0xcbf4_3926);
    }

    #[test]
    fn rejects_payload_overflow_before_encoding() {
        let item = JournalRecord {
            sequence: 1, transaction_id: 1, kind: RecordKind::Mutation,
            payload: vec![0; HARD_MAX_RECORD_PAYLOAD + 1],
        };
        assert_eq!(encode(&item), Err(JournalError::PayloadTooLarge));
    }
}
