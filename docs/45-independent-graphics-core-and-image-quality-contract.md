# 45. Independent graphics core and image-quality contract

## 1. Source independence

ZeroDroid graphics is independently designed and implemented. AEmulator is research material only. No AEmulator source file, binary, generated artifact, implementation patch, renamed implementation or runtime dependency may enter this repository or production build.

The implementation must be based on documented Android/Linux graphics interfaces, public protocol specifications, independently authored code and independently authored tests. Every imported upstream component requires a separate provenance and license review; AEmulator is not an allowed source dependency.

## 2. Quality objective

The Guest display must preserve the sharpness and fidelity expected from a native Android device at the selected output geometry. "A frame was displayed" is not sufficient evidence of graphics completion.

The default quality path is native-resolution rendering with one-to-one pixel mapping. Resolution, density, physical output size and refresh rate are separate properties. Density changes UI dp sizing; it must not be used to disguise an undersized framebuffer.

No backend may silently:
- render below the negotiated Guest display resolution and upscale;
- resize a Guest buffer to an unrelated intermediate resolution;
- apply bilinear filtering to text or pixel-exact content without an explicit policy;
- change pixel format, alpha convention, color space or row stride without a declared conversion;
- drop high precision formats or use lossy texture compression without negotiated permission;
- present a buffer before its acquire fence signals or reuse it before release completion.

## 3. Independently implemented Rust contract core

Crate: `zerodroid-graphics-core`.

The crate is platform-neutral and forbids unsafe Rust. It currently defines:
- pixel formats: RGBA8888, RGBX8888, BGRA8888, RGB565, RGBA1010102 and RGBA FP16;
- color-space and alpha-mode metadata;
- display geometry, rotation and explicit scaling policy;
- one-to-one pixel mapping validation and integer-nearest scaling validation;
- checked stride, allocation range and arithmetic validation;
- bounded damage rectangles;
- generation-tagged frame descriptors and monotonic frame sequencing;
- a bounded frame metadata queue;
- bounded buffer registration and checked Available → Queued → InFlight → Available leasing.

The crate does not allocate GPU memory, own pixel storage, import DMA-BUF, wait on kernel fences, call EGL/Vulkan, access Android Surface APIs or perform rendering. Those responsibilities belong to independently implemented backends.

## 4. Scaling policy

- `OneToOne`: Guest dimensions must equal the rotated output dimensions. This is the preferred mode for native sharpness.
- `IntegerNearest`: only equal integer scale factors on both axes are accepted; the backend must use nearest-neighbor sampling.
- `Filtered`: filtering is an explicit opt-in. The caller must know that resampling may soften fine text and edges.

If the physical screen and chosen Guest aspect ratio differ, the display policy must explicitly choose letterboxing/cropping or a user-visible resolution change. It must not distort the image or silently stretch it.

## 5. Buffer and color invariants

Every buffer descriptor carries width, height, byte stride, pixel format, offset and allocation size. All size arithmetic is checked. The current core bounds dimensions to 16,384 pixels and a single allocation to 512 MiB; actual runtime budgets must be lower when device memory pressure requires it.

The backend must preserve the producer's format and color metadata or perform a named, testable conversion. RGBA/BGRA channel order, opaque/X alpha, premultiplied alpha, FP16 and wide-gamut handling must be tested independently.

A shared handle is not proof of zero-copy. Import success, modifier compatibility, cache coherency, ownership, synchronization and release behavior must be verified for each device/driver combination.

## 6. Frame synchronization and lifecycle

The frame descriptor carries a display generation, strictly increasing sequence, buffer identity, optional acquire-fence identity, damage and color metadata. A display reconfiguration increments generation and invalidates queued stale frames.

The platform backend must additionally implement:
- acquire-fence wait/import and release-fence export;
- GPU completion and display-consumer completion;
- buffer ownership transitions and cancellation;
- bounded in-flight count and backpressure;
- VSync scheduling and stale-frame policy;
- safe teardown after Surface destruction, GPU reset or Guest restart.

Fence identifiers in the portable core are opaque metadata only; they do not represent an operating-system fence implementation.

## 7. Quality and performance validation

Required tests:
1. Native one-to-one geometry at representative phone resolutions.
2. Rejection of implicit scaling in OneToOne mode.
3. Rotation and output-coordinate mapping.
4. Odd dimensions, padded stride and nonzero allocation offsets.
5. RGBA/BGRA/RGBX/RGB565/10-bit/FP16 channel and alpha correctness.
6. sRGB, Display P3, BT.2020 and linear-space conversion vectors.
7. One-pixel lines, checkerboards, subpixel edges and small text sharpness.
8. Damage-region bounds and partial-update correctness.
9. Acquire/release fence ordering, queue saturation and buffer reuse.
10. Resolution changes, stale-generation rejection and Surface teardown.
11. Sustained frame pacing, dropped frames, thermal throttling and memory pressure.

Track output-vs-Guest pixel dimensions, resampling count, format conversions, bytes copied, GPU/CPU time, frame latency, missed VSync, queue depth and buffer lifetime. Image quality must not be traded for an undocumented performance shortcut.

## 8. Implementation status boundary

This change provides validated contracts and unit-test source only. It is not a GLES/Vulkan backend, GPU passthrough, Gralloc implementation, Composer HAL, SurfaceFlinger integration, DMA-BUF import path or real-device image-quality proof. No GPU acceleration or zero-copy claim is made until those layers are implemented and tested on named Android devices and drivers.
