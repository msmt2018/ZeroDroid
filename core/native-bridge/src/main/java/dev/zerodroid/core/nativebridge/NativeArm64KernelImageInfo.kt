package dev.zerodroid.core.nativebridge

/** Validated ARM64 Linux Image header fields returned by the Rust image core. */
data class NativeArm64KernelImageInfo(
    val pageSizeCode: Int,
    val textOffset: Long,
    val declaredImageSize: Long,
    val effectiveImageSize: Long,
    val flags: Long,
    val imageOffset: Long,
    val imageLength: Long
) {
    init {
        require(pageSizeCode in 0..3)
        require(textOffset >= 0)
        require(declaredImageSize >= 0)
        require(effectiveImageSize >= 64)
        require(imageOffset >= 0)
        require(imageLength == effectiveImageSize)
    }

    val pageSizeBytes: Int?
        get() = when (pageSizeCode) {
            1 -> 4096
            2 -> 16384
            3 -> 65536
            else -> null
        }
}
