package dev.zerodroid.core.runtime

import android.content.Context
import dev.zerodroid.core.nativebridge.NativeRuntime

/** Wires the Android public-SDK and native KVM probes into the shared evidence gate. */
class AndroidHostCapabilityDiscovery(
    context: Context,
    nativeRuntime: NativeRuntimeFacade = NativeRuntimeFacade {
        NativeRuntime.probeHostCapabilities()
    }
) {
    private val discovery = HostCapabilityDiscovery(
        architectureProbe = AndroidHostArchitectureProbe(),
        avfProbe = AndroidAvfCapabilityProbe(context.applicationContext),
        kvmProbe = AndroidKvmCapabilityProbe(nativeRuntime)
    )

    suspend fun discover(
        requiredAvfVmType: String,
        preference: List<VirtualizationBackend> = listOf(
            VirtualizationBackend.ANDROID_AVF,
            VirtualizationBackend.DIRECT_KVM
        )
    ): HostCapabilityReport = discovery.discover(requiredAvfVmType, preference)
}
