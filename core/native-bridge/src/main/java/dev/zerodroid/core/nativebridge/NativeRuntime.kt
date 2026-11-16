package dev.zerodroid.core.nativebridge

/** JNI loading boundary; VM operations remain unavailable until a real backend is integrated. */
object NativeRuntime {
    init { System.loadLibrary("zerodroid_jni") }

    external fun abiVersion(): Int
    private external fun nativeAcquireSurface(surface: android.view.Surface): LongArray?
    private external fun nativeQuerySurface(handle: Long): LongArray?
    private external fun nativeReleaseSurface(handle: Long): Boolean

    /** Acquires one owned ANativeWindow reference from [surface]. */
    fun acquireSurface(surface: android.view.Surface): NativeSurfaceLease {
        val values = nativeAcquireSurface(surface)
            ?: error("Failed to acquire Android native Surface")
        require(values.size == 4) { "Unexpected native Surface metrics payload" }
        val metrics = NativeSurfaceMetrics(
            leaseHandle = values[0],
            widthPixels = values[1].toInt(),
            heightPixels = values[2].toInt(),
            pixelFormat = values[3].toInt()
        )
        return NativeSurfaceLease(metrics.leaseHandle, metrics)
    }

    internal fun querySurface(handle: Long): NativeSurfaceMetrics {
        val values = nativeQuerySurface(handle)
            ?: error("Failed to query native Surface")
        require(values.size == 4) { "Unexpected native Surface metrics payload" }
        return NativeSurfaceMetrics(values[0], values[1].toInt(), values[2].toInt(), values[3].toInt())
    }

    internal fun releaseSurface(handle: Long) {
        check(nativeReleaseSurface(handle)) { "Native Surface lease was already released or became stale" }
    }
    private external fun nativeAllocateHardwareBuffer(width: Int, height: Int, usage: Long): LongArray?
    private external fun nativeDescribeHardwareBuffer(handle: Long): LongArray?
    private external fun nativeReleaseHardwareBuffer(handle: Long): Boolean

    /** Allocates a bounded RGBA8 HardwareBuffer for GPU sampling and color output. */
    fun allocateHardwareBuffer(
        width: Int,
        height: Int,
        usage: Long = HardwareBufferUsage.GPU_SAMPLED_IMAGE or HardwareBufferUsage.GPU_COLOR_OUTPUT
    ): NativeHardwareBufferLease {
        val values = nativeAllocateHardwareBuffer(width, height, usage)
            ?: error("Failed to allocate Android HardwareBuffer")
        val metrics = values.toHardwareBufferMetrics()
        return NativeHardwareBufferLease(metrics.leaseHandle, metrics)
    }

    internal fun describeHardwareBuffer(handle: Long): NativeHardwareBufferMetrics =
        checkNotNull(nativeDescribeHardwareBuffer(handle)).toHardwareBufferMetrics()

    internal fun releaseHardwareBuffer(handle: Long) {
        check(nativeReleaseHardwareBuffer(handle)) { "HardwareBuffer lease was already released or became stale" }
    }

    private fun LongArray.toHardwareBufferMetrics(): NativeHardwareBufferMetrics {
        require(size == 7) { "Unexpected HardwareBuffer metrics payload" }
        return NativeHardwareBufferMetrics(
            leaseHandle = this[0], width = this[1].toInt(), height = this[2].toInt(),
            layers = this[3].toInt(), format = this[4].toInt(), stride = this[5].toInt(),
            usage = this[6]
        )
    }

    private external fun probeHostCapabilitiesRaw(): IntArray?

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
