# zerodroid-image-core

Dependency-free, unsafe-free Rust primitives for bounded Android image parsing.

Implemented:
- checked integer ranges and power-of-two alignment;
- explicit input, item-count, and expanded-output budgets;
- Android sparse-image v1 framing/index parsing with RAW/FILL/DONT_CARE CRC32 and chunk checkpoints;
- Android boot image v3/v4 header and payload-range indexing, including v4 boot signature size;
- Android vendor boot image v3/v4 section indexing, including v4 ramdisk-table entries and bootconfig;
- Primary GPT header and partition-array parsing with header/entry CRC32, disk-bound checks, and partition-overlap rejection;
- payload ranges reference the caller input without copying or expanding data.

Sparse CRC32 is calculated over the logical expanded byte stream, including zero-filled DONT_CARE regions, and checked at optional chunk checkpoints and the optional header checksum. Boot v3/v4 parsing validates header framing and bounded payload ranges only; it does not authenticate the boot signature. Neither parser writes images, verifies AVB/VBMeta, parses LP metadata, or boots a guest.


CRC32 validation now covers RAW, FILL and DONT_CARE expanded bytes, optional per-chunk CRC32 checkpoints, and the optional file-header checksum. This remains a bounded in-memory parser/indexer: it does not write expanded images, verify AVB/VBMeta, or validate LP metadata. The caller must choose conservative input and expanded-size budgets.

Vendor boot parsing validates the page-size alignment, required ramdisk/DTB sections, v4 table geometry, per-entry fragment bounds and overlap, and total payload budget. It does not interpret DTB contents, select fragments for a particular board, validate bootconfig semantics, or perform AVB authentication.
\nGPT support currently parses a primary header at LBA 1 and its partition array. Backup GPT consistency, protective MBR validation, partition filesystem probing, and writes are not implemented.\n