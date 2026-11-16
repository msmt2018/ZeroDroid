# 49. Native VM core foundation and language boundaries

## Scope

This increment establishes the first executable VM lifecycle core in cpp/, separate from Android JNI glue in native/, and a safe Rust Guest machine-spec validator in rust/zerodroid-vm-spec-core/.

It is not a KVM implementation, AVF adapter, VMM, Guest boot loader or working VM backend.

## Source layout

- cpp/include/zerodroid/core/: portable C++ VM/backend contracts.
- cpp/src/: portable VM lifecycle implementation.
- cpp/tests/: host-side contract tests with a test-only backend.
- native/: C ABI, JNI and Android host capability adapters.
- rust/zerodroid-vm-spec-core/: safe validation of AArch64 Guest vCPU and Guest physical memory layout.
- rust/zerodroid-image-core/: bounded image metadata parsers.
- third_party/manifest/zerodroid.xml: upstream source acquisition manifest.
- third_party/aosp/: separate repo-managed upstream worktrees after source sync.

## Backend contract

A backend must explicitly report ARM64 Guest architecture, hardware virtualization, Guest memory mapping, vCPU execution, virtual interrupt controller, virtual timer, Guest device transport, maximum vCPU count and maximum Guest memory before VM preparation.

A missing capability or permission denial fails preflight. No CPU emulator or software fallback is permitted. Test backends exist only in host tests and are not registered in Android production builds.

The backend interface owns create/start/stop/destroy and state reconciliation. Failed start/stop operations are reconciled with the backend's observed state; the public lifecycle must not claim RUNNING merely because a request was issued.

## Current implementation

- Bounded VM configuration checks: nonzero VM ID/generation, AArch64, 1..64 vCPUs, 4 KiB-aligned memory and a 64 GiB hard ceiling.
- Serialized prepare/start/stop/destroy operations.
- Explicit CREATED/PREFLIGHT/PREPARED/STARTING/RUNNING/STOPPING/STOPPED/FAILED/DESTROYED states.
- Monotonic state revision and capability-gated backend creation.
- Host tests using a test-only backend.
- A portable Guest GPA layout validator with bounded region count, checked end arithmetic, sorted non-overlap validation, RAM total accounting, per-region permissions and single-region range resolution. Failed rebuilds preserve the previous valid layout.
- The Android Native CMake target now emits the shared `libzerodroid_vm.so`; the JNI facade remains a separate `libzerodroid_jni.so`.
- Rust validation of bounded memory regions, alignment, checked GPA arithmetic, non-overlap and RAM-size consistency.
- Android legacy boot image v0/v1/v2 indexing for kernel, ramdisk, second-stage, recovery DTBO and DTB; existing v3/v4 and init_boot v4 indexing remains available.
- ARM64 Linux Image header validation for magic, text offset, declared/effective image size, page-size flags and little-endian Android Guest ABI.
- Sparse FILL/DONT_CARE CRC32 processing now uses logarithmic GF(2) matrix combination, including multi-gigabyte zero/fill runs without allocating expanded image buffers.

The current C ABI/JNI does not yet expose this VM controller. The shared-library conversion packages the runtime foundation as a loadable `.so`, but does not make VM create/start operations available without a production backend. Production AVF and authorized-KVM backend implementations remain unimplemented. A /dev/kvm probe alone is not sufficient to create a usable Android VM.

## Upstream source acquisition

The 73-project Repo manifest is a source acquisition catalogue, not a claim that 73 projects are required in the APK. The canonical sync script checks out upstream trees under third_party/aosp/ and emits exact-revision lock files only after a complete successful sync.

C/C++ upstream source is not blindly copied into cpp/. That directory contains ZeroDroid-authored portable runtime code. Selected upstream modules are integrated from separate, revision-locked worktrees only after license/source review, Android Bionic/NDK adaptation, dependency pruning and build tests. This preserves upstream notices, history and local patch auditability.

## Build and validation

- Host CMake builds the portable VM core and its test-only backend.
- Native C ABI and C header smoke tests remain enabled.
- Cargo workspace tests the Rust machine-spec and existing parser crates.
- Android NDK compiles the portable C++ target through native CMake.

CI success proves compilation and host contract tests only. It does not prove AVF/KVM access, EL2 availability, Guest Kernel boot, virtual GIC/timer operation, VirtIO device closure or physical-device compatibility.

## Next implementation gates

1. Add stable C ABI VM operations and structured event/error snapshots around the C++ controller.
2. Implement Android AVF discovery/adapter using supported platform APIs and permission model.
3. Implement an authorized ARM64 KVM backend only where the system grants access; add VM/vCPU/Guest-memory/GIC/IRQ/Timer setup and teardown.
4. Integrate Rust machine-spec validation through a narrow, versioned C ABI after the wire layout is frozen.
5. Build ARM64 Guest memory/DTB/boot artifact assembly and boot ladder.
6. Add VirtIO-MMIO transport and block/console devices with matching Guest Kernel drivers.