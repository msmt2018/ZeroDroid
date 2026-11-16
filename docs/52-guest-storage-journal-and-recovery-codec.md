# 52. Guest Storage Journal and Recovery Codec

## Scope

`zerodroid-storage-journal` defines a platform-neutral, bounded record format and recovery scanner. It is a codec/state reconstruction primitive, not yet a durable file journal or a complete storage transaction manager.

## Record format

All integers are little-endian. Each record has a fixed 40-byte header: magic `ZDJR`, format version, record kind, monotonically increasing sequence, 128-bit transaction ID, payload length, and CRC-32. CRC covers the header fields preceding the checksum and the payload.

Kinds are Begin, Mutation, Commit, and Abort. Recovery exposes mutations only for transactions with a valid Commit record. Open transactions at end-of-log are ignored. A torn final header or payload is ignored as an incomplete tail; a complete record with bad magic, unsupported version, invalid ordering, or checksum mismatch fails closed.

## Hard bounds

- Journal input: 64 MiB.
- Payload per record: 1 MiB.
- Records per scan: 65,536.
- No unbounded allocation based on unvalidated lengths.

## Limitations and next integration

This crate does not perform filesystem I/O, fsync, atomic rename, locking, encryption, compaction, checkpointing, or replay of SparseCow page mutations. The Android private-storage backend must append records, enforce write ordering, sync the journal before publishing metadata, and implement startup truncation/repair policy. The existing SparseCow registry/index remains volatile until that backend and a versioned metadata snapshot are integrated.

Tests cover committed-only recovery, torn-tail handling, checksum corruption, sequence regression, invalid transaction order, and payload bounds. Unit tests do not prove power-loss durability on Android devices.
