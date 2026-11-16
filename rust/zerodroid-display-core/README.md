# ZeroDroid Display Core

A platform-neutral, safe Rust contract layer for validating packed Guest display frame metadata before a future VMM/Host transport consumes it.

This crate does not allocate GPU buffers, open Android Surface objects, import DMA-BUFs, wait on sync fences, access DRM/EGL/Vulkan, copy pixels, or claim that a virtual GPU is implemented. Fence identifiers are opaque tokens; their ownership and wait/close operations belong to the eventual platform adapter.

The initial contract supports packed RGBA8888, RGBX8888, BGRA8888 and RGB565 frames. YUV/multi-plane formats, modifiers, actual fence FDs, GPU import and VirtIO-GPU transport require separately versioned contracts and validation.

Run tests with:

    cargo test -p zerodroid-display-core
