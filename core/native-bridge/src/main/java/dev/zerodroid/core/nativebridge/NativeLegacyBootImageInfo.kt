package dev.zerodroid.core.nativebridge

/** Validated section offsets for Android boot.img header versions 0, 1 and 2. */
data class NativeLegacyBootImageInfo(
    val headerVersion: Int,
    val headerSize: Int,
    val pageSize: Int,
    val kernelOffset: Long,
    val kernelSize: Long,
    val ramdiskOffset: Long,
    val ramdiskSize: Long,
    val secondOffset: Long,
    val secondSize: Long,
    val recoveryDtboOffset: Long?,
    val recoveryDtboSize: Long,
    val dtbOffset: Long?,
    val dtbSize: Long,
    val dtbAddress: Long,
    val totalIndexedBytes: Long
) {
    init {
        require(headerVersion in 0..2)
        require(headerSize > 0)
        require(pageSize >= 2048 && pageSize <= 65536 && pageSize and (pageSize - 1) == 0)
        require(kernelOffset >= 0 && kernelSize > 0)
        require(ramdiskOffset >= 0 && ramdiskSize >= 0)
        require(secondOffset >= 0 && secondSize >= 0)
        require(recoveryDtboSize >= 0 && dtbSize >= 0)
        require(totalIndexedBytes >= kernelOffset + kernelSize)
    }
}
