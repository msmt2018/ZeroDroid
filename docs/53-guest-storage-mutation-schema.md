# 53. Guest Storage Mutation Schema

## Purpose

`zerodroid-storage-mutation` defines a versioned payload schema carried inside the Mutation records of `zerodroid-storage-journal`. It is deliberately separate from the journal framing and from the Sparse COW in-memory index. The schema converts opaque byte payloads into typed storage operations and rejects malformed or cross-VM transactions before any backend is invoked.

## Binary envelope

Version 1 uses a fixed 60-byte little-endian header:

| Offset | Size | Field |
|---:|---:|---|
| 0 | 4 | Magic `ZDSM` |
| 4 | 2 | Schema version |
| 6 | 2 | Mutation kind |
| 8 | 16 | VM ID |
| 24 | 8 | VM generation |
| 32 | 16 | Object ID |
| 48 | 8 | Page index |
| 56 | 4 | Payload length |
| 60 | variable | Kind-specific payload |

Unknown versions and kinds fail closed. Payload length must exactly match the remaining bytes; trailing bytes are rejected. The maximum payload is 1 MiB.

## Mutation kinds

1. **VmRegister**: quota in bytes; object ID and page index must be zero.
2. **ObjectCreate**: capacity, UTF-8 object name length, and object name.
3. **ObjectDelete**: object ID only.
4. **PagePut**: exactly one 4 KiB page.
5. **PageDelete**: object ID and page index, with no payload.

VM IDs, generations and object IDs must be nonzero. Object names are 1–96 ASCII bytes and limited to alphanumeric, underscore, hyphen and period. Object capacity is capped at 64 GiB. Page indices are bounded by the maximum 64 GiB object size at 4 KiB granularity. VM quota is capped at 256 GiB.

## Transaction validation

`decode_transaction` accepts a committed journal transaction and decodes every mutation. All mutations in one transaction must target exactly one VM ID and generation. Empty committed transactions are rejected. The returned transaction preserves journal transaction ID and commit sequence for a future replay coordinator.

This validation does not apply the operations. In particular, it does not guarantee object existence, object capacity relative to a previously created object, quota accounting, page allocation limits, or atomic publication. Those checks belong to the durable storage transaction layer, which must stage and validate the complete operation set before committing it.

## Integration status and limitations

The crate depends on the journal codec. It provides typed encode/decode, transaction-scope validation, and an in-memory replay planner. `ReplayState::apply_committed_transaction` decodes and applies one committed journal transaction; `ReplayState::replay_report` applies all committed transactions from a `RecoveryReport` to a private candidate state and publishes only if the complete report succeeds. This connects journal recovery to typed metadata reconstruction, but it is not yet wired into SparseCowStore page storage. SparseCowStore now exposes stable-ID object creation/lookup and a backend-batched object deletion primitive, which are prerequisites for that bridge; no coordinator currently translates a committed transaction into those calls. It does not implement snapshots, checkpointing, file I/O, fsync, locking, or Android private-storage persistence. The existing SparseCow backend contract still requires an all-or-nothing durable page batch from a future implementation. In particular, the current page-batch contract does not atomically include journal append, object-registry metadata, and checkpoint publication as one durable transaction.

Tests cover round trips for all operation kinds, invalid identity/capacity/name/page index, exact payload lengths, and mixed VM-generation transaction rejection. Unit tests do not prove storage durability or power-loss recovery.


## In-memory replay planner

The crate also exposes `ReplayState`. It accepts decoded committed transactions in strictly increasing commit-sequence order and validates their mutations against a bounded in-memory metadata index. It enforces VM generation ownership, object ID/name uniqueness, object/page capacity, per-VM object and tracked-page limits, VM quota, and bounded VM/object-ID counts. A generation change starts a fresh object namespace for that VM.

Each transaction is validated against a private copy of its target VM metadata. The copy is published only after every mutation succeeds, so a quota or metadata failure cannot leave a partially applied transaction in the replay index. Replay state exposes only bounded metadata counters; page bytes remain the responsibility of the durable page backend.

This planner is not yet connected to the SparseCowStore backend and does not itself read journal files; it consumes a decoded `RecoveryReport`. It restores metadata allocation indexes only, not page bytes, and does not persist the replay index or establish fsync/atomic-file guarantees. The copy-and-validate strategy favors a simple atomic contract at this stage; profiling and a staged-delta implementation are required before treating it as the final high-throughput recovery path.
