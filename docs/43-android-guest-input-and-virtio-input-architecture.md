# 43. Android Host Input to ARM64 Guest Input Architecture

## 1. Goal

Provide a correct, bounded input path from Android UI input to an independently running ARM64 Guest. The first supported device class is a virtual touchscreen plus virtual keyboard transported through a VMM-owned VirtIO Input device.

This is a Guest device implementation. It must not inject synthetic events into the Android host's global input stack.

## 2. Why uinput is not the Guest transport

Linux uinput creates a virtual input device inside the host Linux kernel. It requires a writable uinput device node and kernel support, and the resulting device is visible to the host input subsystem. It does not automatically create a device inside a separate Guest kernel.

Therefore:
- Do not assume an ordinary Android APK can open /dev/uinput.
- Do not create host-global virtual touch/key devices as a workaround for missing VM input transport.
- Do not treat uinput access as proof of KVM/AVF privileges.
- A separately authorized system integration may use uinput for a clearly scoped host-side feature, but that is not this Guest input path.

The rust-uinput-sys repository is an external reference for ioctl and Linux input ABI research only. ZeroDroid does not vendor its implementation. Before any dependency is added, review its exact upstream revision, license, maintenance status, Android NDK compatibility and ioctl ABI behavior.

## 3. Data path

1. Android UI adapter receives MotionEvent/KeyEvent through public Android APIs.
2. Adapter translates display/surface coordinates into a defined logical coordinate space and maps Android key codes to Linux input key codes.
3. zerodroid-input-core validates samples, clamps coordinates to the Guest axis range, tracks Android pointer IDs independently from Guest multitouch slots and emits event frames.
4. A bounded frame-atomic input queue accepts complete frames with explicit backpressure and lifecycle cancellation.
5. VirtIO Input transport serializes the 8-byte little-endian virtio_input_event records into Guest-provided descriptor chains.
6. VMM signals the configured virtual interrupt only after queue writes are committed.
7. Guest Linux input driver exposes the device to the Guest input subsystem.

The current implementation completes step 3 and provides the event wire payload encoder. Steps 1, 4, 5, 6 and 7 are not yet implemented or device-validated.

## 4. Event semantics

- Every frame ends with exactly one EV_SYN/SYN_REPORT.
- Host pointer IDs are opaque identifiers and must never be used directly as ABS_MT_SLOT values.
- Guest slots are allocated from a fixed bounded range and remain stable until pointer-up/cancel.
- Each active pointer is updated from its own Android pointer sample; one pointer's coordinates must never be replayed for all contacts.
- Pointer-up releases only that contact. BTN_TOUCH is released only when the last active contact is gone.
- ACTION_CANCEL releases every active contact in one frame.
- Invalid or incomplete multi-pointer frames are rejected before state mutation.
- Coordinates and pressure must be finite. Coordinate conversion is clamped to the declared Guest axis range.
- Event timestamps are not added to virtio_input_event; transport framing follows the VirtIO Input ABI.

## 5. Device configuration contract

The future versioned Virtual Hardware Manifest must define:
- device identity/name and input device class;
- ABS_MT_SLOT range and maximum contact count;
- ABS_MT_POSITION_X/Y minimum and maximum;
- ABS_MT_PRESSURE range;
- supported BTN/KEY capability bitmaps;
- VirtIO transport type, queue sizes, feature bits, MMIO address and IRQ;
- reset, suspend, resume and VM-destroy behavior.

The Guest DTB node, VMM device instance and advertised input capabilities must be generated from the same manifest. No hard-coded MMIO address or IRQ is permitted.

## 6. Lifecycle, queueing and security

The Android adapter must not hold a raw VMM queue pointer across VM destruction. A session-scoped handle and generation must reject stale events after surface replacement, VM stop or restart. Queue capacity is bounded; overflow policy must be explicit (coalesce MOVE frames where safe, never silently drop DOWN/UP/CANCEL transitions). On teardown, synthesize releases for active contacts before disabling the device when the backend remains writable.

Input is untrusted. Validate pointer counts, IDs, coordinates, pressure, key codes, frame sizes and descriptor lengths. Do not allow host-provided input to write arbitrary Guest memory; descriptor validation and DMA/shared-memory ownership remain the VMM's responsibility.

## 7. Relationship to Twoyi reference material

Twoyi's input and renderer design may inform architectural questions only. No Twoyi source, binary, copied implementation, generated binding, symbol table or package identity is imported into ZeroDroid. ZeroDroid owns its event normalization, Android adapter, VirtIO transport and rendering interfaces.

## 8. Implementation status

Implemented:
- safe, platform-neutral Rust input core;
- fixed-capacity ten-contact slot tracker;
- coordinate/pressure validation and scaling;
- multi-contact frame consistency checks;
- per-contact release and cancellation;
- Linux key-event frame helper;
- exact little-endian VirtIO input event encoding;
- bounded frame-atomic queue with capacity, malformed-frame rejection, backpressure and close semantics;
- unit tests for the above.

Not implemented:
- Android MotionEvent/KeyEvent adapter and keycode mapping;
- asynchronous adapter-to-VMM scheduling and queue wakeup integration (the current bounded queue is synchronous and transport-neutral);
- VirtIO Input device config/feature negotiation and descriptor queue;
- virtual interrupt delivery and reset;
- Guest DTB/manifest wiring;
- Android Surface lifecycle integration;
- real Guest Linux input-driver validation.

A successful Rust unit test is not evidence that a VM input device is attached or usable on a phone.
