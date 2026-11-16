# ZeroDroid third-party source workspace

This workspace acquires upstream source repositories as separate Git checkouts. It does not copy unreviewed third-party code into the APK or treat a successful download as a completed port.

## Canonical source checkout

Install Git, Python 3 and the official Android `repo` launcher, then run from the repository root:

```bash
ZERODROID_SYNC_GROUPS=default,zerodroid-rom-reference,zerodroid-optional,zerodroid-test \
  bash third_party/scripts/sync_sources.sh
```

The executable manifest currently contains **73 projects**:
- **62 default projects**: Android platform/runtime and Guest framework sources, AVF/Microdroid, Android Common Kernel and Kleaf, crosvm VMM source, VirtIO/device sources, boot/recovery/OTA, image/filesystem tooling and graphics/audio references.
- **6 ROM reference projects**: TIK, RomTools, firmware extraction, Magisk, AnyKernel3 and standalone sparse-image tooling.
- **3 optional platform/firmware references**: upstream Linux, EDK II and AOSP Bluetooth.
- **2 optional validation source trees**: Android CTS and VTS.

The manifest explicitly excludes QEMU/TCG, Android Emulator, Goldfish, AEMU and Cuttlefish emulator/device-image source trees. crosvm is a VMM candidate, not a Hypervisor and not a CPU emulator. It must be audited and adapted for Android ARM64; it is not automatically an APK-ready runtime.

## Source layout and integration boundary

- `third_party/aosp/`: repo-managed upstream source worktrees, preserving upstream directory layout and Git history.
- `cpp/`: ZeroDroid-authored portable C++ VM core and its tests. Android/JNI-specific glue remains in `native/`.
- `native/`: Android NDK/JNI facade and Android host adapters.
- `rust/`: ZeroDroid-authored Rust safety-critical parsers, machine-spec validation and protocol cores.
- `third_party/manifest/zerodroid.xml`: authoritative upstream checkout list.
- `third_party/porting/source-portability.json`: per-project Android ARM64 audit matrix.
- `third_party/SOURCE_LOCK.xml` and `SOURCE_LOCK.json`: generated exact revisions after a successful complete checkout.

Upstream sources are not copied wholesale into `cpp/`. A project is integrated only after selecting its portable components, preserving its license/notices, recording local patches, reviewing its dependency graph and passing Android ARM64 build/runtime gates. The `cpp/` tree contains ZeroDroid's own integration/core code; selected upstream C/C++ libraries remain separately traceable under the source workspace.

## ROM and image pipeline in the default group

- ZIP/OTA archive and codec sources: AOSP libziparchive/libarchive, zlib, bzip2, xz, Brotli, zstd and lz4.
- Modern A/B OTA: AOSP update_engine and payload-dumper-go.
- Legacy block OTA: sdat2img, img2sdat and imgpatchtools.
- Dynamic partitions: AOSP lpdump, lpunpack and lpmake.
- Sparse images: AOSP libsparse and simg2img/img2simg.
- Filesystems: e2fsprogs, erofs-utils and f2fs-tools.
- Boot images: AOSP mkbootimg/unpack_bootimg and Android boot image format reference tooling.
- Device tree: libufdt and dtc/libfdt.

These tools are source inputs for selective porting, not a permission to run ROM-provided scripts or binaries.

## Resource controls

- `ZERODROID_SYNC_JOBS=4`: concurrent sync jobs.
- `ZERODROID_SYNC_DEPTH=0`: full Git history by default. Use depth 1 only for an intentional shallow source review.
- `ZERODROID_SYNC_GROUPS`: default selects all 73 manifest projects across the four documented groups.
- The initial checkout is very large and network/disk intensive. Review available storage before starting.

A successful sync generates exact-revision lock files. A source checkout is not Android build success, integration, device compatibility or a working VM. Keep all source licenses and upstream notices; do not mix local changes into upstream worktrees.
