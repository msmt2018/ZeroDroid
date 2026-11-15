package dev.zerodroid.core.runtime

import kotlinx.coroutines.CancellationException

/**
 * Platform-specific probes are injected by the Android/AVF/KVM integration layer.
 * Implementations must report only facts obtained from supported OS interfaces.
 */
interface HostArchitectureProbe {
    suspend fun probe(): HostArchitecture
}

interface AvfCapabilityProbe {
    suspend fun probe(): AvfHostEvidence
}

interface KvmCapabilityProbe {
    suspend fun probe(): KvmHostEvidence
}

data class CapabilityProbeFailure(
    val source: String,
    val exceptionType: String
)

data class HostCapabilityReport(
    val evidence: HostVirtualizationEvidence,
    val decision: HostCapabilityDecision,
    val failures: List<CapabilityProbeFailure>
)

/**
 * Executes independent platform probes and converts ordinary probe failures into
 * explicit ERROR evidence. Coroutine cancellation is always propagated.
 *
 * This class does not infer capabilities from Android API level, device model,
 * package presence, or CPU marketing features.
 */
class HostCapabilityDiscovery(
    private val architectureProbe: HostArchitectureProbe,
    private val avfProbe: AvfCapabilityProbe,
    private val kvmProbe: KvmCapabilityProbe
) {
    suspend fun discover(
        requiredAvfVmType: String,
        preference: List<VirtualizationBackend> = listOf(
            VirtualizationBackend.ANDROID_AVF,
            VirtualizationBackend.DIRECT_KVM
        )
    ): HostCapabilityReport {
        require(requiredAvfVmType.isNotBlank()) { "requiredAvfVmType must not be blank" }
        val failures = mutableListOf<CapabilityProbeFailure>()

        val architecture = try {
            architectureProbe.probe()
        } catch (error: Exception) {
            if (error is CancellationException) throw error
            failures += CapabilityProbeFailure("architecture", error::class.java.simpleName)
            HostArchitecture.UNKNOWN
        }
        val avf = try {
            avfProbe.probe()
        } catch (error: Exception) {
            if (error is CancellationException) throw error
            failures += CapabilityProbeFailure("avf", error::class.java.simpleName)
            AvfHostEvidence(
                apiAvailable = CapabilityFact.ERROR,
                serviceAvailable = CapabilityFact.ERROR,
                appAuthorized = CapabilityFact.ERROR
            )
        }
        val kvm = try {
            kvmProbe.probe()
        } catch (error: Exception) {
            if (error is CancellationException) throw error
            failures += CapabilityProbeFailure("kvm", error::class.java.simpleName)
            KvmHostEvidence(
                deviceApi = CapabilityFact.ERROR,
                vmCreationAuthorized = CapabilityFact.ERROR,
                el2Available = CapabilityFact.ERROR,
                stage2Available = CapabilityFact.ERROR,
                armGicAvailable = CapabilityFact.ERROR
            )
        }

        val evidence = HostVirtualizationEvidence(architecture, avf, kvm)
        return HostCapabilityReport(
            evidence = evidence,
            decision = HostCapabilityGate.evaluate(evidence, requiredAvfVmType, preference),
            failures = failures.toList()
        )
    }
}
