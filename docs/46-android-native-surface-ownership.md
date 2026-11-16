# 46. Android Native Surface ownership boundary

## Scope

This increment introduces the first Android-specific graphics resource adapter: an independently implemented JNI bridge that acquires, queries and releases an `ANativeWindow` reference obtained from an Android `Surface`.

It deliberately stops at resource ownership. It is not a Guest display backend, Gralloc allocator, EGL/Vulkan renderer, buffer queue implementation, native-fence bridge, scanout path or GPU passthrough implementation.

## Ownership contract

- `ANativeWindow_fromSurface` supplies the native-window reference owned by the returned lease.
- The native registry owns exactly one acquired reference per live handle.
- Handles are monotonic, non-zero and never intentionally reused; exhaustion fails closed.
- Registry allocation failure is reported and the acquired native reference is released by RAII.
- Query and release are serialized against registry mutation.
- Release removes the handle under the registry lock and drops the native reference after unlocking.
- Kotlin's `NativeSurfaceLease` is idempotently closeable. Query after close is rejected.
- A retained native reference does not mean the Java Surface remains a valid presentation target; Surface lifecycle events still need to close the lease and invalidate the associated display generation.

## Geometry and image quality

The adapter reports the current native-window width, height and Android pixel-format identifier. It does not call `ANativeWindow_setBuffersGeometry`, choose a lower resolution, rescale, filter, convert color formats or silently change buffer dimensions.

The reported dimensions are host Surface geometry, not proof that the Guest framebuffer matches them. The display negotiation layer must compare them with the Guest's declared geometry and apply the explicit scaling policy from the portable graphics core.

## Threading

JNI calls may be made from worker threads. The native registry protects its handle map with a mutex. No rendering or blocking fence wait occurs while holding that mutex. Callers must not perform acquisition/query/release on the Android main thread as part of a latency-sensitive frame loop.

## Validation boundary

The Android CI build compiles this adapter against the configured Android NDK. Host-side native contract tests do not compile Android-only APIs. Device validation is still required for Surface destruction/recreation, rotation, resize, multi-window transitions, rapid acquire/release and process teardown.

No claim is made for EGL, Vulkan, HardwareBuffer, sync_file/native fences, Guest Gralloc/Composer, GPU acceleration, zero-copy or visual fidelity. Those are separate increments.
