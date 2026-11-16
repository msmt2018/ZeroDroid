# 50. Guest Virtual Disk I/O Adapter

## 1. Purpose

Connect the validated Dynamic Partition layout to the existing Guest BlockBackend contract. This crate routes partition-relative byte I/O through liblp linear and zero extents. It does not open host paths or access host physical partitions.

## 2. Adapter contract

`MappedPartitionBackend<B>` implements `zerodroid_block_core::BlockBackend` and accepts one validated `DynamicPartition`, one VM identity and one generation. It validates VM/generation on every operation, bounds individual requests to the block core hard transfer limit, and bounds extent fan-out to at most 256 segments.

The backing implementation provides `VirtualDiskExtentBackend`:
- linear read/write/discard routes by validated block-device index/name and byte offset;
- zero extent reads use a sparse zero/COW overlay (unallocated bytes read as zero);
- zero extent writes are retained in the sparse overlay, never silently discarded;
- flush is forwarded to the backend so it can order and persist all affected backing objects.

## 3. Address and trust boundaries

All offsets are relative to a VM-owned virtual backing object. A block-device name is metadata, not a host path. Implementations must resolve it through a VM-scoped backing-object registry, enforce per-VM quotas, reject stale generation handles, and never map Guest addresses to host physical partitions.

The adapter trusts only a `DynamicPartition` produced by the validated dynamic partition core. It does not parse untrusted liblp bytes itself.

## 4. Failure semantics

A mapping or backend failure aborts the current operation with an error. A multi-segment write can have earlier segments written before a later segment fails; therefore production backends must provide transactional/COW semantics or maintain a recoverable journal if atomic Guest request writes are required. Flush ordering and durability are backend responsibilities.

## 5. Current limits

- This is an I/O routing adapter, not a production filesystem/block-storage implementation.
- No Android file descriptor backend, mmap, direct I/O, io_uring, AIO or host block-device access is implemented.
- No COW snapshot engine, sparse-file allocator, crash recovery or per-VM quota manager is implemented.
- No VirtIO queue, Guest DMA translation, interrupt injection or ARM64 VMM integration is implemented.
- The existing liblp parser still consumes a complete super image byte slice; bounded random-access parsing remains required for large images.

## 6. Tests

Unit tests cover cross-extent reads/writes, zero extent overlay persistence, and VM/generation rejection using an in-memory backend. They do not constitute Android-device or production-backend validation.
