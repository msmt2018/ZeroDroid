package dev.zerodroid.core.runtime

import android.os.Build
import dev.zerodroid.core.nativebridge.HostArchitecture as NativeHostArchitecture
import dev.zerodroid.core.nativebridge.KvmProbeState
import dev.zerodroid.core.nativebridge.NativeRuntime

/** Reads process-visible KVM evidence from the native C ABI without inferring EL2/GIC support. */
class AndroidHostArchitectureProbe : HostArchitectureProbe {
    override suspend fun probe(): HostArchitecture {
        return when {
            Build.SUPPORTED_ABIS.any { it == "arm64-v8a" } -> HostArchitecture.AARCH64
            Build.SUPPORTED_ABIS.any { it == "x86_64" } -> HostArchitecture.X86_64
            Build.SUPPORTED_ABIS.isNotEmpty() -> HostArchitecture.OTHER
            else -> HostArchitecture.UNKNOWN
        }
    }
}

class AndroidKvmCapabilityProbe(
    private val nativeRuntime: NativeRuntimeFacade = NativeRuntimeFacade { NativeRuntime.probeHostCapabilities() }
) : KvmCapabilityProbe {
    override suspend fun probe(): KvmHostEvidence {
        val native = nativeRuntime.probe()
        return KvmHostEvidence(
            deviceApi = native.kvmState.toCapabilityFact(),
            apiVersion = native.kvmApiVersion.takeIf { it > 0 },
            vmCreationAuthorized = native.vmCreateState.toCapabilityFact(),
            // A successful KVM_CREATE_VM is not evidence for these separate properties.
            el2Available = CapabilityFact.UNKNOWN,
            stage2Available = CapabilityFact.UNKNOWN,
            armGicAvailable = CapabilityFact.UNKNOWN
        )
    }
}

fun interface NativeRuntimeFacade {
    fun probe(): dev.zerodroid.core.nativebridge.NativeHostCapabilities
}

internal fun KvmProbeState.toCapabilityFact(): CapabilityFact = when (this) {
    KvmProbeState.UNKNOWN -> CapabilityFact.UNKNOWN
    KvmProbeState.AVAILABLE -> CapabilityFact.AVAILABLE
    KvmProbeState.UNAVAILABLE -> CapabilityFact.UNAVAILABLE
    KvmProbeState.ACCESS_DENIED -> CapabilityFact.ACCESS_DENIED
    KvmProbeState.INCOMPATIBLE -> CapabilityFact.INCOMPATIBLE
    KvmProbeState.ERROR -> CapabilityFact.ERROR
}
