# 51. Rust FFI and Android Native Core Integration

## Build topology

- Safe parser and validation logic remains in Rust workspace crates.
- `rust/zerodroid-ffi` is the only unsafe boundary crate. It exposes fixed-width `repr(C)` records and pointer-based parser functions with explicit caller memory contracts.
- Cargo builds `libzerodroid_ffi.a` for `aarch64-linux-android`.
- `core/native-bridge` installs the Rust target and builds the static library before Android CMake configuration.
- `native/CMakeLists.txt` imports the static archive and links it into `libzerodroid_jni.so`; the portable C++ VM core remains in `libzerodroid_vm.so`.
- Kotlin passes a direct ByteBuffer slice to JNI. JNI forwards its address and capacity to Rust; image bytes are not copied to a temporary Java or C++ buffer.

## Current exported Rust parser functions

- `zdroid_rust_parse_arm64_kernel_image`: validates and indexes an uncompressed ARM64 Linux Image header.
- `zdroid_rust_parse_legacy_boot_image`: validates and indexes Android boot.img v0-v2 sections.

Both functions enforce caller-supplied input/payload budgets, reject null pointers and undersized output structures, and return stable status codes. C and Rust struct sizes are checked by the Native C ABI header smoke test.

## Ownership and safety

The caller must keep the direct buffer memory alive and unchanged for the synchronous JNI call. The Rust parser borrows the memory only for the duration of the call and returns offsets/lengths, never pointers into the source. The current Kotlin entry point accepts only direct ByteBuffers and parses the current remaining slice.

The FFI crate contains unsafe pointer conversion; image parsing itself remains in `zerodroid-image-core` with `#![forbid(unsafe_code)]`. No Rust parser performs file I/O, expands a sparse image, writes Host partitions, or boots a Guest.

## Build prerequisites

- Rust stable toolchain with rustup/cargo.
- Android NDK 30.0.16248370 and the `aarch64-linux-android28-clang` linker.
- Android ABI is intentionally restricted to `arm64-v8a`.

The Gradle Native Bridge build invokes `rustup target add aarch64-linux-android` and builds the FFI static archive before CMake. Host CMake tests do not require the Android Rust target.

## Verification boundary

Successful compilation proves the Rust/C ABI and Android linker integration compile. It does not prove that a production KVM/AVF backend exists, that a Guest kernel boots, or that a device grants virtualization permission.