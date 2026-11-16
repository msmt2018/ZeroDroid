package dev.zerodroid.core.runtime

import dev.zerodroid.core.nativebridge.HostArchitecture
import dev.zerodroid.core.nativebridge.KvmProbeState
import dev.zerodroid.core.nativebridge.NativeHostCapabilities
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class AndroidKvmCapabilityProbeTest {
    @Test
    fun vmCreateEvidenceIsMappedSeparatelyFromDeviceApi() {
        val probe = AndroidKvmCapabilityProbe {
            NativeHostCapabilities(
                abiVersion = 2,
                architecture = HostArchitecture.AARCH64,
                kvmState = KvmProbeState.AVAILABLE,
                kvmApiVersion = 12,
                kvmErrno = 0,
                vmCreateState = KvmProbeState.ACCESS_DENIED,
                vmCreateErrno = 13
            )
        }

        kotlinx.coroutines.runBlocking {
            val evidence = probe.probe()
            assertEquals(CapabilityFact.AVAILABLE, evidence.deviceApi)
            assertEquals(12, evidence.apiVersion)
            assertEquals(CapabilityFact.ACCESS_DENIED, evidence.vmCreationAuthorized)
            assertEquals(CapabilityFact.UNKNOWN, evidence.el2Available)
            assertEquals(CapabilityFact.UNKNOWN, evidence.stage2Available)
            assertEquals(CapabilityFact.UNKNOWN, evidence.armGicAvailable)
        }
    }

    @Test
    fun unknownAndIncompatibleNativeEvidenceAreNeverPromoted() {
        assertEquals(CapabilityFact.UNKNOWN, KvmProbeState.UNKNOWN.toCapabilityFact())
        assertEquals(CapabilityFact.INCOMPATIBLE, KvmProbeState.INCOMPATIBLE.toCapabilityFact())
        assertEquals(CapabilityFact.ERROR, KvmProbeState.ERROR.toCapabilityFact())
        assertTrue(KvmProbeState.UNAVAILABLE.toCapabilityFact() != CapabilityFact.AVAILABLE)
    }

    @Test
    fun missingApiVersionIsNotReportedAsZeroVersion() {
        val probe = AndroidKvmCapabilityProbe {
            NativeHostCapabilities(
                abiVersion = 2,
                architecture = HostArchitecture.UNKNOWN,
                kvmState = KvmProbeState.UNAVAILABLE,
                kvmApiVersion = 0,
                kvmErrno = 2,
                vmCreateState = KvmProbeState.UNKNOWN,
                vmCreateErrno = 0
            )
        }
        kotlinx.coroutines.runBlocking {
            assertNull(probe.probe().apiVersion)
        }
    }
}
