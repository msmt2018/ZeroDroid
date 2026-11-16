# 48. Android Native Fence FD lifecycle and bounded waits

## Purpose

This increment adds an owned native descriptor lease for Android native fence sync-file descriptors. It is a synchronization ownership primitive for later EGL/Vulkan interop, not a GPU submission engine.

## Import and ownership

- The input descriptor must be a valid Android/Linux `sync_file` fence FD produced by a trusted EGL/Vulkan native-fence export path or another explicitly supported platform producer.
- The adapter duplicates a non-negative input descriptor using `F_DUPFD_CLOEXEC`; the caller retains ownership of its original descriptor.
- Input `-1` is accepted as the native-fence convention for an already-signaled fence. Values below -1 are rejected.
- The registry owns the duplicate and closes it exactly once when the lease is released or process teardown occurs.
- Handles are monotonic, positive and never reused. At most 128 live fence leases are allowed.
- Wait operations duplicate the registry descriptor while holding the registry mutex, then perform `poll()` without holding the mutex. A concurrent close therefore cannot invalidate an in-progress wait.
- Wait timeout is restricted to 0..60,000 ms. EINTR retries use a monotonic deadline so repeated signals cannot extend the requested timeout without bound.
- `POLLIN` means signaled; timeout returns false; invalid/error poll states fail closed.

## Trust boundary

This layer deliberately does not claim to authenticate arbitrary file descriptors as sync_file objects. The import API is internal to the native bridge and must only receive descriptors from trusted native EGL/Vulkan/platform producers. A future hardening step should add kernel sync-file identity validation where the supported Android NDK/kernel headers expose a stable ioctl contract.

The adapter does not merge fences, signal fences, create GPU work, export a new fence, or prove that a particular buffer's rendering is complete. Callers must only attach a fence to a buffer when the producer's API defines that fence as the completion dependency for that buffer.

## Synchronization semantics

- Fence FDs are one-shot completion objects. Waiting does not consume or close the lease.
- A successful wait reports only that the imported fence signaled; it does not establish image layout, cache visibility, ownership transfer, or presentation completion beyond the producer's fence contract.
- Vulkan queue-family ownership and image-layout transitions remain the Vulkan backend's responsibility.
- EGL native-fence import/export and AHardwareBuffer acquire/release integration are not implemented in this increment.

## Validation boundary

Android CI compiles the NDK source and Kotlin bridge. Host-side native tests do not compile this Android-only source. Physical-device validation is still required for actual sync_file producers, signal/error behavior, descriptor duplication, timeout accuracy, concurrent wait/close, and vendor EGL/Vulkan behavior.

No claim is made for Guest GPU protocol completion, zero-copy, GPU passthrough or end-to-end frame presentation.
