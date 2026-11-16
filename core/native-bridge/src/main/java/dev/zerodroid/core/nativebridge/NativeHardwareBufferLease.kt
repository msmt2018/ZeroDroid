package dev.zerodroid.core.nativebridge

/** Android HardwareBuffer usage bits accepted by the native allocation boundary. */
object HardwareBufferUsage {
    const val GPU_SAMPLED_IMAGE: Long = 0x100L
    const val GPU_COLOR_OUTPUT: Long = 0x200L
    const val CPU_READ_RARELY: Long = 2L
    const val CPU_READ_OFTEN: Long = 3L
    const val CPU_WRITE_RARELY: Long = 0x10L
    const val CPU_WRITE_OFTEN: Long = 0x30L
}

/** Immutable allocation metadata returned by AHardwareBuffer_describe(). */
data class NativeHardwareBufferMetrics(
    val leaseHandle: Long,
    val width: Int,
    val height: Int,
    val layers: Int,
    val format: Int,
    val stride: Int,
    val usage: Long
) {
    init {
        require(leaseHandle > 0L)
        require(width > 0 && height > 0 && layers == 1 && stride >= width)
    }
}

/**
 * Owns one AHardwareBuffer reference in the native process registry.
 *
 * This is allocation/lifetime management only. It does not expose the buffer to
 * EGL/Vulkan, lock CPU pixels, submit GPU work or provide Guest/Host sharing.
 */
class NativeHardwareBufferLease internal constructor(
    private val handle: Long,
    val initialMetrics: NativeHardwareBufferMetrics
) : AutoCloseable {
    private var closed = false

    @Synchronized
    fun queryMetrics(): NativeHardwareBufferMetrics {
        check(!closed) { "HardwareBuffer lease is already closed" }
        return NativeRuntime.describeHardwareBuffer(handle)
    }

    @Synchronized
    override fun close() {
        if (!closed) {
            closed = true
            NativeRuntime.releaseHardwareBuffer(handle)
        }
    }
}
