# zerodroid-image-core

Dependency-free, unsafe-free Rust primitives for bounded Android image parsing.

Implemented:
- checked integer ranges and power-of-two alignment;
- explicit input, item-count, and expanded-output budgets;
- Android sparse-image v1 framing/index parsing with RAW/FILL/DONT_CARE CRC32 and chunk checkpoints;
- Android boot image v3/v4 header and payload-range indexing, including v4 boot signature size;
- Android vendor boot image v3/v4 section indexing, including v4 ramdisk-table entries and bootconfig;
- Primary and backup GPT header/partition-array parsing with reciprocal-header checks, CRC32 and byte-for-byte array consistency;
- Android liblp geometry and metadata v10.0-v10.2 parsing with SHA-256 checksums, primary/backup slot fallback, partition/group/extent/block-device indexing and bounded mapping validation;
- Protective MBR signature and single 0xEE partition validation; duplicate partition GUID and overlapping partition rejection;
- payload ranges reference the caller input without copying or expanding data.

Sparse CRC32 is calculated over the logical expanded byte stream, including zero-filled DONT_CARE regions, and checked at optional chunk checkpoints and the optional header checksum. Boot v3/v4 parsing validates header framing and bounded payload ranges only; it does not authenticate the boot signature. Neither parser writes images, verifies AVB/VBMeta, parses LP metadata, or boots a guest.


CRC32 validation now covers RAW, FILL and DONT_CARE expanded bytes, optional per-chunk CRC32 checkpoints, and the optional file-header checksum. This remains a bounded in-memory parser/indexer: it does not write expanded images, verify AVB/VBMeta, or validate LP metadata. The caller must choose conservative input and expanded-size budgets.

Vendor boot parsing validates the page-size alignment, required ramdisk/DTB sections, v4 table geometry, per-entry fragment bounds and overlap, and total payload budget. It does not interpret DTB contents, select fragments for a particular board, validate bootconfig semantics, or perform AVB authentication.

GPT support parses the primary header at LBA 1, validates the backup header at the final LBA and requires reciprocal header fields plus byte-identical partition arrays. Protective MBR validation checks the 0x55AA signature and exactly one correctly sized 0xEE entry. Filesystem probing and image writes are not implemented.


Android Dynamic Partitions parsing validates the AOSP liblp geometry, metadata header and table SHA-256 checksums; reads primary metadata with backup-copy fallback; validates partition, extent, group and block-device tables; and rejects invalid extent ownership, physical range overlap, group-size overflow and metadata/data-region overlap. It indexes mappings without expanding or writing logical partition payloads. OTA payload application, snapshot/COW handling, filesystem parsing and AVB verification remain separate work.
