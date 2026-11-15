# zerodroid-image-core

Dependency-free, unsafe-free Rust primitives for bounded Android image parsing.

Implemented:
- checked integer ranges and power-of-two alignment;
- explicit input, item-count, and expanded-output budgets;
- Android sparse-image v1 framing/index parsing for RAW, FILL, DONT_CARE and CRC32 chunk framing;
- payload ranges reference the caller input without copying or expanding data.

The parser validates structure and size accounting only. It does not apply chunks, calculate CRC32, verify AVB/VBMeta, parse LP metadata, or boot a guest. Those remain separate stages.
