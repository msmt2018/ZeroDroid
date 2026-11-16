# 52. Guest Storage Journal and Recovery Codec

## Scope

`zerodroid-storage-journal` defines a platform-neutral, bounded record format and recovery scanner. It is a codec/state reconstruction primitive, not yet a durable file journal or a complete storage transaction manager.

## Record format

All integers are little-endian. Each record has a fixed 40-byte header: magic `ZDJR`, format version, record kind, monotonically increasing sequence, 128-bit transaction ID, payload length, and CRC-32. CRC covers the header fields preceding the checksum and the payload.

Kinds are Begin, Mutation, Commit, and Abort. Recovery exposes mutations only for transactions with a valid Commit record. Open transactions at end-of-log are ignored. A torn final header or payload is reported as an ignored tail; the scanner exposes the exact valid-prefix byte offset. A complete record with bad magic, unsupported version, invalid ordering, duplicate transaction ID, or checksum mismatch fails closed.

Transaction IDs are unique for the lifetime of one journal stream and cannot be reused after Commit or Abort. Sequence values must be strictly increasing across all complete records.

## Hard bounds

- Journal input: 64 MiB.
- Payload per record: 1 MiB.
- Records per scan: 65,536.
- Retained mutation payloads during recovery: 32 MiB.
- No unbounded allocation based on unvalidated lengths.

The 32 MiB recovery budget applies to mutation bytes retained in open and committed transactions. Aborted transaction payloads release that budget. This budget is separate from the input-log bound because recovery materializes owned mutation buffers.

## Recovery report

The scanner provides a report containing committed transactions, valid-prefix bytes, ignored-tail bytes, last sequence, and scanned record count. The report does not authorize truncation by itself. A future storage backend must hold exclusive VM-journal ownership, verify the file identity and durability state, then apply its own crash-safe tail-repair policy. The codec never mutates the source bytes.

## Limitations and next integration

This crate does not perform filesystem I/O, fsync, atomic rename, locking, encryption, compaction, checkpointing, or replay of SparseCow page mutations. The Android private-storage backend must append records, enforce write ordering, sync the journal before publishing metadata, and implement startup truncation/repair policy. The existing SparseCow registry/index remains volatile until that backend and a versioned metadata snapshot are integrated.

Tests cover committed-only recovery, exact torn-tail offsets, checksum corruption, sequence regression, invalid transaction order, transaction-ID reuse, the IEEE CRC-32 known vector, and payload bounds. Unit tests do not prove power-loss durability on Android devices.
