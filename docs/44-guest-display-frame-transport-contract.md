# 44. Guest Display Frame Transport Contract

## 1. Scope

This stage defines a safe, platform-neutral metadata contract between a future Guest display producer and a Host/VMM transport. It is deliberately not a GPU implementation and does not depend on Android Emulator, Goldfish, QEMU, AEMU or Cuttlefish.

The first supported payloads are single-plane packed RGBA8888, RGBX8888, BGRA8888 and RGB565 frames. Multi-plane YUV, DRM modifiers, protected buffers, DMA-BUF import, native fence FDs and VirtIO-GPU resource commands require separate contracts and capability negotiation.

## 2. Frame descriptor

Every frame carries:
- nonzero buffer identity;
- VM/Surface generation, invalidating stale callbacks after stop/restart or Surface replacement;
- monotonically increasing sequence within a generation;
- width/height and explicit packed pixel format;
- byte offset, row stride and allocation size;
- opaque acquire-fence token (not assumed to be an FD);
- monotonic timestamp in nanoseconds;
- bounded damage rectangles.

The descriptor describes metadata only. The receiver must independently validate the underlying buffer handle, allocation provenance, usage flags, ownership and access rights before importing or reading pixels.

## 3. Validation rules

- Dimensions are nonzero and bounded by negotiated maximum width/height.
- stride_bytes >= width * bytes_per_pixel.
- All multiplication and addition uses checked arithmetic.
- stride_bytes * height and the complete allocation remain under the configured hard byte limit.
- offset_bytes + payload_bytes <= allocation_size_bytes.
- Damage rectangles are nonempty and remain within frame bounds; the list is capped.
- Unknown pixel formats, modifiers and multi-plane layouts are rejected until negotiated.
- Frame generation must match the active VM/Surface generation.
- Sequence numbers must increase strictly within one generation.

The Rust core performs metadata/range validation only. It does not prove that a buffer handle points to valid memory.

## 4. Bounded queue and lifecycle

The queue has both frame-count and byte budgets. A frame is either accepted in full or rejected; no partial frame enqueue is allowed. Full queues apply backpressure and do not silently drop a frame. A consumer must drain the old generation before switching to a new generation. Closing the queue clears queued metadata and rejects future frames.

The future Android adapter must ensure queue wakeups do not run on the UI thread and must never retain a raw VMM descriptor pointer across VM destruction. A transport implementation may define safe frame coalescing only after proving that it cannot lose presentation-critical acquire/release or lifecycle semantics.

## 5. Fence and buffer ownership boundary

Fence IDs are opaque values. The Rust core does not wait, duplicate, close or interpret them. The Android/VMM adapter must define:
- which side creates and owns each native fence;
- whether ownership transfers or is borrowed;
- timeout and cancellation behavior;
- release-fence delivery;
- behavior when the Surface or VM generation is destroyed;
- protected-content restrictions.

DMA-BUF, AHardwareBuffer, gralloc handles and native fence FDs must not be serialized as arbitrary integers and trusted by the Guest. Cross-process or cross-VM access requires explicit kernel/backend authorization and a documented sharing mechanism.

## 6. Integration sequence

1. Add Android Surface lifecycle adapter and generation management.
2. Add Host buffer import capability discovery for AHardwareBuffer/Gralloc and supported formats.
3. Add transport-specific descriptor/queue implementation behind the VMM device API.
4. Add VirtIO-GPU resource and scanout configuration only when the selected VMM backend supports it.
5. Add Guest Kernel DRM, Gralloc Mapper/Allocator and Composer integration matching the Guest Android release.
6. Add end-to-end tests for buffer lifetime, fences, resize, rotation, Surface destruction, VM restart, GPU reset, backpressure and memory pressure.

A SurfaceView or TextureView is only a presentation container; it does not itself implement a Guest GPU or framebuffer transport.

## 7. Current implementation status

Implemented in rust/zerodroid-display-core:
- safe packed-frame metadata validation;
- checked stride, allocation range and size-budget arithmetic;
- bounded damage rectangle validation;
- frame-count and byte-bounded metadata queue;
- generation and sequence validation;
- queue drain/reset/close lifecycle;
- unit tests for invalid ranges, overflow, stale generations, backpressure and teardown.

Not implemented:
- Android Surface adapter;
- AHardwareBuffer/Gralloc import;
- DMA-BUF and sync-fence ownership/FD transfer;
- VMM display or VirtIO-GPU device;
- Guest DRM/Gralloc/Composer/EGL/Vulkan stack;
- actual pixel transport or GPU acceleration;
- device-level rendering validation.

A passing unit test establishes only the platform-neutral metadata contract. It does not establish a usable display or accelerated GPU path.
