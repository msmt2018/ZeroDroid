package dev.zerodroid.core.nativebridge

/** JNI loading boundary; VM operations remain unavailable until a real backend is integrated. */
object NativeRuntime {
    init { System.loadLibrary("zerodroid_jni") }

    external fun abiVersion(): Int
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
