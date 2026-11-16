# 51. Guest Sparse COW Storage Core

## Scope

The zerodroid-sparse-cow-store crate is a bounded, platform-neutral sparse page/COW coordinator intended to sit behind VirtualDiskExtentBackend. It is not a filesystem path adapter and never resolves guest names to host paths or host physical partitions.

## Contracts

- VM registry and generation checks isolate every operation.
- Objects have validated metadata names, a bounded logical capacity and VM ownership.
- Data is allocated in 4 KiB pages; missing pages read as zero.
- Writes use read/modify/write page batches. The backend commit_pages contract is atomic: all page updates become visible or none do.
- VM quota is checked before commit; page fan-out and request size are hard bounded.
- Discard removes sparse pages, and flush is explicitly forwarded.
- Backend failure does not publish the in-memory allocation index.

## Important limitations

The crate currently keeps VM/object registry and allocation indexes in process memory. It does not yet provide durable registry serialization, restart recovery, a journal implementation, Android app-private file storage, encryption, concurrent request serialization, or a production backend. A platform backend must implement atomic durable page batches and enforce VM/generation identity itself. Do not restore a VM by reusing this volatile registry; persistent metadata/recovery is the next required layer.

The memory backend in tests proves core semantics only; it is not evidence of durable storage or Android-device operation. The core deliberately does not use mmap, raw block devices, host physical partitions, QEMU, or emulator storage paths.

## Bounds

- Page size: 4 KiB.
- Maximum single request: 16 MiB (inherited hard bound).
- Maximum object capacity: 64 GiB.
- Maximum per-VM quota: 256 GiB (configured quota ceiling; actual in-memory sparse allocation index is separately capped at 262,144 pages / 1 GiB). 
- Maximum objects per VM: 1,024, bounding the volatile registry.
- Batch pages: request-derived and bounded to at most 4098 pages.

## Stable identity and object lifecycle

The core now exposes `create_object_with_id` and `object_id` so a future recovery coordinator can restore the Object ID recorded in the mutation journal instead of allocating a different identity. IDs are nonzero, bounded to 65,536 per store lifetime, and remain reserved after object deletion and VM-generation changes. The ordinary `create_object` path uses the same insertion logic and advances its allocator past explicitly restored IDs.

`delete_object` removes all allocated pages through one backend batch before removing the volatile registry entry. If the backend batch fails, the object and allocation index remain visible and unchanged. This relies on the backend `commit_pages` all-or-nothing contract; it does not yet append a journal transaction or atomically combine page deletion with durable object metadata publication.

## Tests

Tests cover sparse-zero reads, partial-page read/modify/write, quota rejection before commit, quota-shrink rejection, per-VM object-count bounds, discard reclamation, stale generation rejection, backend failure atomicity, flush forwarding, and object-name/capacity validation, stable-ID restore and duplicate rejection, object deletion, and failed-delete rollback.
