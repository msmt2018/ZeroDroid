package dev.zerodroid.core.runtime

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class HostCapabilityGateTest {
    @Test
    fun unknownEvidenceNeverSelectsBackend() {
        val result = HostCapabilityGate.evaluate(HostVirtualizationEvidence(), "microdroid")
        assertFalse(result.supported)
        assertEquals(null, result.selectedBackend)
        assertTrue(result.assessments.all { !it.supported && it.blockers.isNotEmpty() })
    }

    @Test
    fun accessibleKvmDeviceAloneDoesNotAuthorizeVmCreation() {
        val evidence = HostVirtualizationEvidence(
            architecture = HostArchitecture.AARCH64,
            kvm = KvmHostEvidence(
                deviceApi = CapabilityFact.AVAILABLE,
                apiVersion = HostCapabilityGate.REQUIRED_KVM_API_VERSION
            )
        )
        val result = HostCapabilityGate.evaluate(evidence, "microdroid")
        assertFalse(result.supported)
        assertTrue(result.assessments.single { it.backend == VirtualizationBackend.DIRECT_KVM }
            .blockers.contains("KVM_VM_CREATION_AUTHORIZATION_UNKNOWN"))
    }

    @Test
    fun avfRequiresApiServiceAuthorizationAndRequestedVmType() {
        val evidence = HostVirtualizationEvidence(
            avf = AvfHostEvidence(
                apiAvailable = CapabilityFact.AVAILABLE,
                serviceAvailable = CapabilityFact.AVAILABLE,
                appAuthorized = CapabilityFact.AVAILABLE,
                supportedVmTypes = setOf("microdroid")
            )
        )
        val supported = HostCapabilityGate.evaluate(evidence, "microdroid")
        assertEquals(VirtualizationBackend.ANDROID_AVF, supported.selectedBackend)

        val unsupportedType = HostCapabilityGate.evaluate(evidence, "full-android")
        assertFalse(unsupportedType.supported)
        assertTrue(unsupportedType.assessments.first().blockers.contains("AVF_VM_TYPE_UNSUPPORTED"))
    }

    @Test
    fun directKvmRequiresAllArm64HypervisorPrerequisites() {
        val evidence = HostVirtualizationEvidence(
            architecture = HostArchitecture.AARCH64,
            kvm = KvmHostEvidence(
                deviceApi = CapabilityFact.AVAILABLE,
                apiVersion = HostCapabilityGate.REQUIRED_KVM_API_VERSION,
                vmCreationAuthorized = CapabilityFact.AVAILABLE,
                el2Available = CapabilityFact.AVAILABLE,
                stage2Available = CapabilityFact.AVAILABLE,
                armGicAvailable = CapabilityFact.AVAILABLE
            )
        )
        val result = HostCapabilityGate.evaluate(evidence, "microdroid")
        assertEquals(VirtualizationBackend.DIRECT_KVM, result.selectedBackend)
    }

    @Test
    fun explicitPreferenceIsHonoredWhenBothBackendsAreFullySupported() {
        val evidence = HostVirtualizationEvidence(
            architecture = HostArchitecture.AARCH64,
            avf = AvfHostEvidence(
                apiAvailable = CapabilityFact.AVAILABLE,
                serviceAvailable = CapabilityFact.AVAILABLE,
                appAuthorized = CapabilityFact.AVAILABLE,
                supportedVmTypes = setOf("microdroid")
            ),
            kvm = KvmHostEvidence(
                deviceApi = CapabilityFact.AVAILABLE,
                apiVersion = HostCapabilityGate.REQUIRED_KVM_API_VERSION,
                vmCreationAuthorized = CapabilityFact.AVAILABLE,
                el2Available = CapabilityFact.AVAILABLE,
                stage2Available = CapabilityFact.AVAILABLE,
                armGicAvailable = CapabilityFact.AVAILABLE
            )
        )
        val result = HostCapabilityGate.evaluate(
            evidence,
            "microdroid",
            listOf(VirtualizationBackend.DIRECT_KVM, VirtualizationBackend.ANDROID_AVF)
        )
        assertEquals(VirtualizationBackend.DIRECT_KVM, result.selectedBackend)
    }
}
