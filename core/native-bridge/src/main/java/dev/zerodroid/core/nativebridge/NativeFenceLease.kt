package dev.zerodroid.core.nativebridge

/**
 * Owns a duplicated native sync-file fence descriptor.
 *
 * The caller retains ownership of the input fd. Import duplicates it with
 * close-on-exec; closing this lease never closes the caller's descriptor.
 */
class NativeFenceLease internal constructor(
    private val handle: Long
) : AutoCloseable {
    private var closed = false

    /** Returns true when signaled, false when the bounded wait times out. */
    @Synchronized
    fun await(timeoutMillis: Int = 0): Boolean {
        check(!closed) { "Native fence lease is already closed" }
        require(timeoutMillis in 0..60_000) { "Fence timeout must be within 0..60000 ms" }
        return when (NativeRuntime.waitFence(handle, timeoutMillis)) {
            NativeRuntime.FENCE_SIGNALED -> true
            NativeRuntime.FENCE_TIMEOUT -> false
            else -> error("Native fence wait failed")
        }
    }

    @Synchronized
    fun isSignaled(): Boolean = await(0)

    @Synchronized
    override fun close() {
        if (!closed) {
            closed = true
            NativeRuntime.releaseFence(handle)
        }
    }
}
