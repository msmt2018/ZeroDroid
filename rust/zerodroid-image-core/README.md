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

VBMeta parsing validates the AVB0 header, supported required version, authentication/auxiliary block bounds, section overlap, descriptor TLV framing and descriptor/input budgets. It indexes digest/signature/key/metadata/descriptor ranges only; cryptographic verification and descriptor-specific semantics are intentionally not claimed.

Vendor boot parsing validates the page-size alignment, required ramdisk/DTB sections, v4 table geometry, per-entry fragment bounds and overlap, and total payload budget. It does not interpret DTB contents, select fragments for a particular board, validate bootconfig semantics, or perform AVB authentication.

GPT support parses the primary header at LBA 1, validates the backup header at the final LBA and requires reciprocal header fields plus byte-identical partition arrays. Protective MBR validation checks the 0x55AA signature and exactly one correctly sized 0xEE entry. Filesystem probing and image writes are not implemented.


Android Dynamic Partitions parsing validates the AOSP liblp geometry, metadata header and table SHA-256 checksums; reads primary metadata with backup-copy fallback; validates partition, extent, group and block-device tables; and rejects invalid extent ownership, physical range overlap, group-size overflow and metadata/data-region overlap. It indexes mappings without expanding or writing logical partition payloads. OTA payload application, snapshot/COW handling, filesystem parsing and AVB verification remain separate work.


Flattened Device Tree support parses FDT v16 and v17 with separate header handling, bounded reservation/node/property indexing, compatible/model extraction, and DTBO fragment target indexing. DTBO execution includes external/local phandle fixup planning, conflict-checked atomic writes to a private copy, target resolution, owned-tree/property merge, symbol export rewriting, bounded reservation-map union, and FDT v17 serialization with parse round-trip and canonical depth-first node ordering. `apply_dtbo_sequence` runs those stages in order for raw DTBO inputs and recalculates each overlay against the accumulated tree. Synthetic unit fixtures cover edge cases; CI additionally generates real DTB/DTBO files with `dtc -@` and exercises external/local fixups, phandle declaration/reference relocation and symbol export. Broader libfdt differential coverage and Guest Platform Profile device-resource validation remain outstanding. This does not perform AVB authentication or prove that a guest boots.

Overlay phandle relocation updates both node `phandle`/`linux,phandle` declarations and `__local_fixups__` references in one conflict-checked write set, keeping declarations and relocated references consistent.
Sparse FILL and DONT_CARE checksum updates use IEEE CRC-32 polynomial matrix combination: repeated byte patterns are folded in logarithmic time and constant memory instead of iterating once per expanded output byte. RAW chunks remain checksummed directly from their borrowed input ranges. This avoids turning a highly sparse multi-gigabyte image into billions of tiny checksum-loop iterations; the parser still validates the complete logical expanded stream and enforces the caller's expanded-size budget.
Legacy Android boot image v0/v1/v2 parsing indexes kernel, ramdisk, second-stage, recovery DTBO and DTB ranges with page alignment, declared recovery offset checks and input/payload budgets. Together with the existing v3/v4 and init_boot v4 parsers, this covers the major Android boot header generations at the framing/index level; it does not authenticate AVB or guarantee a device-specific boot.
