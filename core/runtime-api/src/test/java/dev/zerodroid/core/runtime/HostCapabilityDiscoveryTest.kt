package dev.zerodroid.core.runtime

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class HostCapabilityDiscoveryTest {
    @Test
    fun probeFailuresBecomeExplicitErrorsAndDoNotSelectBackend() = runBlocking {
        val discovery = HostCapabilityDiscovery(
            architectureProbe = object : HostArchitectureProbe {
                override suspend fun probe() = HostArchitecture.AARCH64
            },
            avfProbe = object : AvfCapabilityProbe {
                override suspend fun probe(): AvfHostEvidence = error("AVF unavailable")
            },
            kvmProbe = object : KvmCapabilityProbe {
                override suspend fun probe(): KvmHostEvidence = error("KVM denied")
            }
        )

        val report = discovery.discover("microdroid")
        assertFalse(report.decision.supported)
        assertEquals(CapabilityFact.ERROR, report.evidence.avf.apiAvailable)
        assertEquals(CapabilityFact.ERROR, report.evidence.kvm.deviceApi)
        assertEquals(setOf("avf", "kvm"), report.failures.map { it.source }.toSet())
    }

    @Test
    fun successfulEvidenceIsPassedToGateWithoutInventingFacts() = runBlocking {
        val discovery = HostCapabilityDiscovery(
            architectureProbe = object : HostArchitectureProbe {
                override suspend fun probe() = HostArchitecture.AARCH64
            },
            avfProbe = object : AvfCapabilityProbe {
                override suspend fun probe() = AvfHostEvidence(
                    apiAvailable = CapabilityFact.AVAILABLE,
                    serviceAvailable = CapabilityFact.AVAILABLE,
                    appAuthorized = CapabilityFact.AVAILABLE,
                    supportedVmTypes = setOf("microdroid")
                )
            },
            kvmProbe = object : KvmCapabilityProbe {
                override suspend fun probe() = KvmHostEvidence()
            }
        )

        val report = discovery.discover("microdroid")
        assertEquals(VirtualizationBackend.ANDROID_AVF, report.decision.selectedBackend)
        assertTrue(report.failures.isEmpty())
        assertEquals(CapabilityFact.UNKNOWN, report.evidence.kvm.deviceApi)
    }

    @Test(expected = IllegalArgumentException::class)
    fun blankVmTypeIsRejectedBeforeProbing() = runBlocking {
        val unusedArchitecture = object : HostArchitectureProbe {
            override suspend fun probe(): HostArchitecture = error("must not run")
        }
        val unusedAvf = object : AvfCapabilityProbe {
            override suspend fun probe(): AvfHostEvidence = error("must not run")
        }
        val unusedKvm = object : KvmCapabilityProbe {
            override suspend fun probe(): KvmHostEvidence = error("must not run")
        }
        HostCapabilityDiscovery(unusedArchitecture, unusedAvf, unusedKvm).discover(" ")
    }

    @Test
    fun cancellationIsNotSwallowedAsProbeFailure() = runBlocking {
        val discovery = HostCapabilityDiscovery(
            architectureProbe = object : HostArchitectureProbe {
                override suspend fun probe(): HostArchitecture = throw CancellationException("cancel")
            },
            avfProbe = object : AvfCapabilityProbe {
                override suspend fun probe() = AvfHostEvidence()
            },
            kvmProbe = object : KvmCapabilityProbe {
                override suspend fun probe() = KvmHostEvidence()
            }
        )
        try {
            discovery.discover("microdroid")
            throw AssertionError("CancellationException should propagate")
        } catch (_: CancellationException) {
            // Expected: structured concurrency must remain cancellable.
        }
    }
}
