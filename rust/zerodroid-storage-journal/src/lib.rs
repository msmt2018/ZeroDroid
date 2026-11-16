#![forbid(unsafe_code)]

use std::collections::BTreeMap;

pub const MAGIC: [u8; 4] = *b"ZDJR";
pub const FORMAT_VERSION: u16 = 1;
pub const HEADER_BYTES: usize = 40;
pub const HARD_MAX_LOG_BYTES: usize = 64 * 1024 * 1024;
pub const HARD_MAX_RECORD_PAYLOAD: usize = 1024 * 1024;
pub const HARD_MAX_RECORDS: usize = 65_536;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum RecordKind { Begin = 1, Mutation = 2, Commit = 3, Abort = 4 }

impl TryFrom<u16> for RecordKind {
    type Error = JournalError;
    fn try_from(v: u16) -> Result<Self, Self::Error> {
        match v { 1 => Ok(Self::Begin), 2 => Ok(Self::Mutation), 3 => Ok(Self::Commit), 4 => Ok(Self::Abort), _ => Err(JournalError::UnknownKind) }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalError {
    LogTooLarge, TooManyRecords, PayloadTooLarge, TruncatedRecord,
    BadMagic, UnsupportedVersion, UnknownKind, ChecksumMismatch,
    SequenceRegression, InvalidTransaction, IntegerOverflow,
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

pub fn encode(record: &JournalRecord) -> Result<Vec<u8>, JournalError> {
    if record.payload.len() > HARD_MAX_RECORD_PAYLOAD { return Err(JournalError::PayloadTooLarge); }
    let len = u32::try_from(record.payload.len()).map_err(|_| JournalError::IntegerOverflow)?;
    let mut out = Vec::with_capacity(HEADER_BYTES + record.payload.len());
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&(record.kind as u16).to_le_bytes());
    out.extend_from_slice(&record.sequence.to_le_bytes());
    out.extend_from_slice(&record.transaction_id.to_le_bytes());
    out.extend_from_slice(&len.to_le_bytes());
    let checksum = crc32_parts(&out, &record.payload);
    out.extend_from_slice(&checksum.to_le_bytes());
    out.extend_from_slice(&record.payload);
    if out.len() > HARD_MAX_LOG_BYTES { return Err(JournalError::LogTooLarge); }
    Ok(out)
}

/// Scans a bounded journal. An incomplete final record is treated as a torn tail;
/// a complete record with invalid metadata or checksum is corruption and fails closed.
pub fn recover(bytes: &[u8]) -> Result<Vec<CommittedTransaction>, JournalError> {
    if bytes.len() > HARD_MAX_LOG_BYTES { return Err(JournalError::LogTooLarge); }
    let mut cursor = 0usize;
    let mut last_sequence = None;
    let mut open: BTreeMap<u128, Vec<Vec<u8>>> = BTreeMap::new();
    let mut committed = Vec::new();
    let mut records = 0usize;
    while cursor < bytes.len() {
        let remaining = bytes.len() - cursor;
        if remaining < HEADER_BYTES { break; }
        let h = &bytes[cursor..cursor + HEADER_BYTES];
        if h[0..4] != MAGIC { return Err(JournalError::BadMagic); }
        let version = u16::from_le_bytes([h[4], h[5]]);
        if version != FORMAT_VERSION { return Err(JournalError::UnsupportedVersion); }
        let kind = RecordKind::try_from(u16::from_le_bytes([h[6], h[7]]))?;
        let sequence = u64::from_le_bytes(h[8..16].try_into().map_err(|_| JournalError::TruncatedRecord)?);
        let txid = u128::from_le_bytes(h[16..32].try_into().map_err(|_| JournalError::TruncatedRecord)?);
        let payload_len = u32::from_le_bytes(h[32..36].try_into().map_err(|_| JournalError::TruncatedRecord)?) as usize;
        let expected_crc = u32::from_le_bytes(h[36..40].try_into().map_err(|_| JournalError::TruncatedRecord)?);
        if payload_len > HARD_MAX_RECORD_PAYLOAD { return Err(JournalError::PayloadTooLarge); }
        let total = HEADER_BYTES.checked_add(payload_len).ok_or(JournalError::IntegerOverflow)?;
        if remaining < total { break; }
        let payload = &bytes[cursor + HEADER_BYTES..cursor + total];
        if crc32_parts(&h[..36], payload) != expected_crc { return Err(JournalError::ChecksumMismatch); }
        if last_sequence.is_some_and(|last| sequence <= last) { return Err(JournalError::SequenceRegression); }
        last_sequence = Some(sequence);
        records += 1;
        if records > HARD_MAX_RECORDS { return Err(JournalError::TooManyRecords); }
        match kind {
            RecordKind::Begin => {
                if !payload.is_empty() || open.contains_key(&txid) { return Err(JournalError::InvalidTransaction); }
                open.insert(txid, Vec::new());
            }
            RecordKind::Mutation => open.get_mut(&txid).ok_or(JournalError::InvalidTransaction)?.push(payload.to_vec()),
            RecordKind::Commit => {
                if !payload.is_empty() { return Err(JournalError::InvalidTransaction); }
                let mutations = open.remove(&txid).ok_or(JournalError::InvalidTransaction)?;
                committed.push(CommittedTransaction { transaction_id: txid, mutations, commit_sequence: sequence });
            }
            RecordKind::Abort => {
                if !payload.is_empty() || open.remove(&txid).is_none() { return Err(JournalError::InvalidTransaction); }
            }
        }
        cursor += total;
    }
    Ok(committed)
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
    fn rec(seq: u64, tx: u128, kind: RecordKind, payload: &[u8]) -> Vec<u8> {
        encode(&JournalRecord { sequence: seq, transaction_id: tx, kind, payload: payload.to_vec() }).unwrap()
    }
    #[test]
    fn recovers_only_committed_transactions() {
        let mut log = Vec::new();
        log.extend(rec(1, 7, RecordKind::Begin, &[]));
        log.extend(rec(2, 7, RecordKind::Mutation, b"page-a"));
        log.extend(rec(3, 7, RecordKind::Commit, &[]));
        log.extend(rec(4, 8, RecordKind::Begin, &[]));
        log.extend(rec(5, 8, RecordKind::Mutation, b"uncommitted"));
        let recovered = recover(&log).unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].mutations, vec![b"page-a".to_vec()]);
    }
    #[test]
    fn ignores_incomplete_tail_but_rejects_complete_corruption() {
        let mut log = rec(1, 1, RecordKind::Begin, &[]);
        log.extend_from_slice(&[1, 2, 3]);
        assert!(recover(&log).unwrap().is_empty());
        let mut bad = rec(1, 1, RecordKind::Begin, &[]);
        *bad.last_mut().unwrap() ^= 0x80;
        assert_eq!(recover(&bad), Err(JournalError::ChecksumMismatch));
    }
    #[test]
    fn rejects_sequence_regression_and_invalid_transaction_order() {
        let mut log = rec(2, 1, RecordKind::Begin, &[]);
        log.extend(rec(1, 1, RecordKind::Commit, &[]));
        assert_eq!(recover(&log), Err(JournalError::SequenceRegression));
        assert_eq!(recover(&rec(1, 4, RecordKind::Commit, &[])), Err(JournalError::InvalidTransaction));
    }
    #[test]
    fn rejects_payload_overflow_before_encoding() {
        let r = JournalRecord { sequence: 1, transaction_id: 1, kind: RecordKind::Mutation, payload: vec![0; HARD_MAX_RECORD_PAYLOAD + 1] };
        assert_eq!(encode(&r), Err(JournalError::PayloadTooLarge));
    }
}
