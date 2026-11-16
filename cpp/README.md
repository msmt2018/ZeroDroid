# ZeroDroid portable C++ core

This directory contains ZeroDroid-authored portable C++ VM runtime code. Android JNI and Android-specific host adapters remain under native/.

## Build

The native CMake project includes this directory as zerodroid_vm_core. It is built for host contract tests and Android NDK targets.

## Rules

- No Activity, Compose, JNI or Android framework types in portable core headers.
- No STL types or C++ exceptions cross the public C ABI.
- A backend must explicitly advertise hardware virtualization, Guest memory, vCPU, virtual interrupt controller, timer and device transport capabilities.
- Missing KVM/AVF permissions result in an unavailable backend, never CPU emulation.
- Test-only backends belong only in tests/; they are not production runtime fallbacks.
- Keep upstream projects under the revision-locked source workspace. Do not copy third-party source into this directory without a reviewed integration patch and retained license notices.