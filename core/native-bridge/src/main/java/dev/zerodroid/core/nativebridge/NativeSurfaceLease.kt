package dev.zerodroid.core.nativebridge

import android.view.Surface
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
    initialMetrics: NativeSurfaceMetrics
) : AutoCloseable {
    private val closed = AtomicBoolean(false)

    val initialMetrics: NativeSurfaceMetrics = initialMetrics

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

/** Acquires and owns the native window reference associated with [surface]. */
fun NativeRuntime.acquireSurface(surface: Surface): NativeSurfaceLease {
    val metrics = nativeAcquireSurface(surface)
        ?: error("Failed to acquire Android native Surface")
    require(metrics.size == 4) { "Unexpected native Surface metrics payload" }
    val snapshot = NativeSurfaceMetrics(
        leaseHandle = metrics[0],
        widthPixels = metrics[1].toInt(),
        heightPixels = metrics[2].toInt(),
        pixelFormat = metrics[3].toInt()
    )
    return NativeSurfaceLease(snapshot.leaseHandle, snapshot)
}

private fun NativeRuntime.querySurface(handle: Long): NativeSurfaceMetrics {
    val values = nativeQuerySurface(handle)
        ?: error("Failed to query native Surface")
    require(values.size == 4) { "Unexpected native Surface metrics payload" }
    return NativeSurfaceMetrics(values[0], values[1].toInt(), values[2].toInt(), values[3].toInt())
}

private fun NativeRuntime.releaseSurface(handle: Long) {
    check(nativeReleaseSurface(handle)) { "Native Surface lease was already released or became stale" }
}

internal external fun NativeRuntime.nativeAcquireSurface(surface: Surface): LongArray?
internal external fun NativeRuntime.nativeQuerySurface(handle: Long): LongArray?
internal external fun NativeRuntime.nativeReleaseSurface(handle: Long): Boolean
