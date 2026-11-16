# 47. Android HardwareBuffer allocation and ownership

## Scope

This increment adds a bounded Android NDK `AHardwareBuffer` allocation/describe/release adapter. It is the first native allocation primitive intended to feed later EGL/Vulkan import work.

It does not export a Java `HardwareBuffer`, import into EGL/Vulkan, lock CPU pixels, create a BufferQueue, submit rendering commands, signal/wait native fences, scan out a frame, or connect a Guest GPU protocol.

## Allocation contract

- Initial supported pixel format is `AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM` (RGBA8 UNORM), one layer.
- Width and height are each in 1..8192.
- The requested usage must include both GPU sampled-image and GPU color-output flags. Only those GPU flags and the explicitly enumerated CPU read/write flags are accepted.
- The allocator's returned width, height, layer count, format and usage are checked before publication.
- Actual stride is read from `AHardwareBuffer_describe`; byte accounting uses `stride × height × 4`, with checked multiplication.
- Maximum estimated allocation is 64 MiB per buffer, 64 live buffers and 256 MiB total accounted buffer storage per process.
- Budget/count checks and handle publication are serialized. Failed publication releases the allocated native reference.
- Handles are positive, monotonic and never reused during process lifetime. Exhaustion fails closed.
- Describe takes a temporary acquired reference while holding the registry lock, then performs JNI array allocation outside the lock.
- Release removes the entry and adjusts accounting under lock, then drops the native reference outside the lock.

The byte budget is a conservative accounting bound based on the reported stride, not a guarantee of the driver's exact physical allocation size or GPU heap usage.

## Pixel quality

No scaling, filtering, format conversion or implicit resolution reduction is performed. The requested dimensions and actual allocator dimensions must match. The RGBA8 format is an initial baseline, not a claim of HDR/FP16/10-bit support. Future format negotiation must preserve exact color/alpha semantics and expose stride/usage explicitly.

## API level

The native bridge module currently targets minSdk 28, above the NDK HardwareBuffer API introduction level. The build uses the Android NDK and links the platform `android` library.

## Validation boundary

CI compiles the Android NDK adapter and runs host-side contract tests, but does not exercise actual device GPU allocation. Physical-device testing remains required across vendors for supported usage combinations, allocation failure, memory pressure, process teardown and concurrent acquire/describe/release.

No claim is made for EGL/Vulkan interoperability, native-fence synchronization, zero-copy, Guest GPU acceleration, or visual fidelity.
