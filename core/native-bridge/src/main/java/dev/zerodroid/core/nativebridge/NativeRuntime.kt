package dev.zerodroid.core.nativebridge

import java.nio.ByteBuffer

/** JNI loading boundary; VM operations remain unavailable until a real backend is integrated. */
object NativeRuntime {
    init { System.loadLibrary("zerodroid_jni") }

    external fun abiVersion(): Int
    private external fun probeHostCapabilitiesRaw(): IntArray?
    private external fun parseArm64KernelImageRaw(buffer: ByteBuffer, maxInputBytes: Long, maxImageBytes: Long): LongArray?

    /**
     * Probes the process-visible KVM device interface.
     *
     * VM creation is separately tested with a temporary VM fd that is immediately
     * closed. This does not prove vCPU creation, EL2 ownership, GIC support, guest
     * boot support, or end-to-end virtualization. A denied probe is preserved
     * as a distinct state and is never treated as an invitation to use CPU emulation.
     */
    fun probeHostCapabilities(): NativeHostCapabilities {
        val values = checkNotNull(probeHostCapabilitiesRaw()) {
            "Native host capability probe failed"
        }
        require(values.size == 7) { "Unexpected native host capability payload" }
        val reportedAbi = values[0]
        val loadedAbi = abiVersion()
        require(reportedAbi == loadedAbi) {
            "Native capability payload ABI $reportedAbi does not match loaded library ABI $loadedAbi"
        }
        return NativeHostCapabilities(
            abiVersion = values[0],
            architecture = HostArchitecture.fromWireValue(values[1]),
            kvmState = KvmProbeState.fromWireValue(values[2]),
            kvmApiVersion = values[3],
            kvmErrno = values[4],
            vmCreateState = KvmProbeState.fromWireValue(values[5]),
            vmCreateErrno = values[6]
        )
    }
}

    /**
     * Parses an uncompressed ARM64 Linux Image header through the Rust image core.
     * The input must be a direct ByteBuffer; only its current remaining slice is inspected.
     * No image bytes are copied into a temporary Java or native buffer.
     */
    fun parseArm64KernelImage(
        buffer: ByteBuffer,
        maxInputBytes: Long = 256L * 1024L * 1024L,
        maxImageBytes: Long = 256L * 1024L * 1024L
    ): NativeArm64KernelImageInfo {
        require(buffer.isDirect) { "ARM64 kernel image parsing requires a direct ByteBuffer" }
        require(maxInputBytes > 0 && maxImageBytes > 0) { "Image budgets must be positive" }
        val input = buffer.slice()
        val values = checkNotNull(parseArm64KernelImageRaw(input, maxInputBytes, maxImageBytes)) {
            "Native Rust ARM64 kernel parser could not access the direct buffer"
        }
        require(values.size == 8) { "Unexpected Rust ARM64 kernel parser payload" }
        val status = values[0].toInt()
        require(status == 0) { "ARM64 kernel image rejected by Rust parser (status=$status)" }
        return NativeArm64KernelImageInfo(
            pageSizeCode = values[1].toInt(),
            textOffset = values[2],
            declaredImageSize = values[3],
            effectiveImageSize = values[4],
            flags = values[5],
            imageOffset = values[6],
            imageLength = values[7]
        )
    }
