package dev.zerodroid.core.nativebridge

import java.util.concurrent.atomic.AtomicBoolean

/** Current dimensions and Android pixel-format identifier reported by ANativeWindow. */
data class NativeSurfaceMetrics(
    val leaseHandle: Long,
    val widthPixels: Int,
    val heightPixels: Int,
    val pixelFormat: Int
) {
    init {
        require(leaseHandle > 0L)
        require(widthPixels > 0 && heightPixels > 0)
    }
}

/**
 * Owns one native ANativeWindow reference acquired from an Android Surface.
 *
 * This lease only manages Surface/native-window lifetime and queries its current
 * geometry. It does not allocate buffers, submit GPU work, present frames, or
 * establish Guest-to-Host graphics transport.
 */
class NativeSurfaceLease internal constructor(
    private val handle: Long,
    val initialMetrics: NativeSurfaceMetrics
) : AutoCloseable {
    private val closed = AtomicBoolean(false)

    fun queryMetrics(): NativeSurfaceMetrics {
        check(!closed.get()) { "Surface lease is already closed" }
        return NativeRuntime.querySurface(handle)
    }

    override fun close() {
        if (closed.compareAndSet(false, true)) {
            NativeRuntime.releaseSurface(handle)
        }
    }
}
