
package dev.zerodroid.core.runtime

/** A host fact. UNKNOWN must never be promoted to AVAILABLE. */
enum class CapabilityFact { UNKNOWN, AVAILABLE, UNAVAILABLE, ACCESS_DENIED, INCOMPATIBLE, ERROR }
enum class HostArchitecture { AARCH64, X86_64, OTHER, UNKNOWN }
enum class VirtualizationBackend { ANDROID_AVF, DIRECT_KVM }

data class AvfHostEvidence(
    val apiAvailable: CapabilityFact = CapabilityFact.UNKNOWN,
    val serviceAvailable: CapabilityFact = CapabilityFact.UNKNOWN,
    val appAuthorized: CapabilityFact = CapabilityFact.UNKNOWN,
    val supportedVmTypes: Set<String> = emptySet()
)

data class KvmHostEvidence(
    val deviceApi: CapabilityFact = CapabilityFact.UNKNOWN,
    val apiVersion: Int? = null,
    val vmCreationAuthorized: CapabilityFact = CapabilityFact.UNKNOWN,
    val el2Available: CapabilityFact = CapabilityFact.UNKNOWN,
    val stage2Available: CapabilityFact = CapabilityFact.UNKNOWN,
    val armGicAvailable: CapabilityFact = CapabilityFact.UNKNOWN
)

data class HostVirtualizationEvidence(
    val architecture: HostArchitecture = HostArchitecture.UNKNOWN,
    val avf: AvfHostEvidence = AvfHostEvidence(),
    val kvm: KvmHostEvidence = KvmHostEvidence()
)

data class BackendAssessment(
    val backend: VirtualizationBackend,
    val supported: Boolean,
    val blockers: List<String>
)

data class HostCapabilityDecision(
    val selectedBackend: VirtualizationBackend?,
    val assessments: List<BackendAssessment>,
    val supported: Boolean
)

/**
 * Selects only a fully evidenced hardware virtualization path.
 * There is intentionally no CPU-emulation or container fallback.
 */
object HostCapabilityGate {
    const val REQUIRED_KVM_API_VERSION = 12

    fun evaluate(
        evidence: HostVirtualizationEvidence,
        requiredAvfVmType: String,
        preference: List<VirtualizationBackend> = listOf(
            VirtualizationBackend.ANDROID_AVF,
            VirtualizationBackend.DIRECT_KVM
        )
    ): HostCapabilityDecision {
        require(requiredAvfVmType.isNotBlank()) { "requiredAvfVmType must not be blank" }

        val avfBlockers = buildList {
            if (evidence.avf.apiAvailable != CapabilityFact.AVAILABLE) add("AVF_API_${evidence.avf.apiAvailable.name}")
            if (evidence.avf.serviceAvailable != CapabilityFact.AVAILABLE) add("AVF_SERVICE_${evidence.avf.serviceAvailable.name}")
            if (evidence.avf.appAuthorized != CapabilityFact.AVAILABLE) add("AVF_AUTHORIZATION_${evidence.avf.appAuthorized.name}")
            if (requiredAvfVmType !in evidence.avf.supportedVmTypes) add("AVF_VM_TYPE_UNSUPPORTED")
        }
        val kvmBlockers = buildList {
            if (evidence.architecture != HostArchitecture.AARCH64) add("KVM_REQUIRES_AARCH64_HOST")
            if (evidence.kvm.deviceApi != CapabilityFact.AVAILABLE) add("KVM_DEVICE_API_${evidence.kvm.deviceApi.name}")
            if (evidence.kvm.apiVersion != REQUIRED_KVM_API_VERSION) add("KVM_API_VERSION_MISMATCH")
            if (evidence.kvm.vmCreationAuthorized != CapabilityFact.AVAILABLE) add("KVM_VM_CREATION_AUTHORIZATION_${evidence.kvm.vmCreationAuthorized.name}")
            if (evidence.kvm.el2Available != CapabilityFact.AVAILABLE) add("KVM_EL2_${evidence.kvm.el2Available.name}")
            if (evidence.kvm.stage2Available != CapabilityFact.AVAILABLE) add("KVM_STAGE2_${evidence.kvm.stage2Available.name}")
            if (evidence.kvm.armGicAvailable != CapabilityFact.AVAILABLE) add("KVM_ARM_GIC_${evidence.kvm.armGicAvailable.name}")
        }
        val assessments = listOf(
            BackendAssessment(VirtualizationBackend.ANDROID_AVF, avfBlockers.isEmpty(), avfBlockers),
            BackendAssessment(VirtualizationBackend.DIRECT_KVM, kvmBlockers.isEmpty(), kvmBlockers)
        )
        val byBackend = assessments.associateBy { it.backend }
        val selected = preference.firstNotNullOfOrNull { backend ->
            backend.takeIf { byBackend[it]?.supported == true }
        }
        return HostCapabilityDecision(selected, assessments, selected != null)
    }
}
