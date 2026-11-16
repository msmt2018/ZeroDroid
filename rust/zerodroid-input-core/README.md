# ZeroDroid Input Core

This crate is the platform-neutral input normalization layer for Android-hosted ARM64 guests.

## Responsibilities

- Convert Android-adapter pointer samples into Linux input event semantics used by VirtIO Input.
- Map host display-local coordinates into the Guest's declared ABS axis ranges.
- Maintain stable Guest multitouch slots independently of Android pointer IDs.
- Emit complete event frames terminated by exactly one SYN_REPORT.
- Handle pointer up and cancellation without leaving BTN_TOUCH stuck.
- Encode the 8-byte little-endian virtio_input_event payload.
- Queue complete frames atomically with a fixed frame capacity and explicit full/closed errors.

## Deliberate boundaries

- No uinput device is created on the Android host.
- No /dev/input or /dev/uinput node is opened.
- No JNI, Android SDK, hidden API, root, or privileged permission is assumed.
- This crate does not yet implement the VirtIO transport, descriptor/feature negotiation, event queue, interrupt delivery, or Guest DTB node.
- The Android adapter and VMM transport must be implemented separately and connected through bounded queues with backpressure.

Linux uinput is a host-kernel facility for creating a host input device. It is not a substitute for a Guest-facing VirtIO Input device. A normal APK also cannot assume permission to create arbitrary host input devices.

## API

- InputConfig validates display dimensions and Guest axis limits.
- MultiTouchNormalizer owns a fixed maximum of ten contacts and maps Android pointer IDs to stable Guest slots.
- pointer_down, pointer_move, pointer_up and cancel_all produce atomic event frames.
- key_event accepts Linux key codes. Android KeyEvent-to-Linux mapping belongs in the Android-specific adapter.

## Validation

Unit tests cover coordinate scaling, stable slots, multi-contact movement, per-pointer release, cancellation, malformed frames, finite-value validation, pointer limits, exact little-endian event encoding and bounded-queue backpressure/lifecycle.

## Upstream reference

The public rust-uinput-sys project may be consulted for Linux uinput ioctl definitions and ABI conventions after checking its current license and API. It is not copied or linked by this crate. The VirtIO input device protocol, rather than host uinput, is the intended Guest device interface.
