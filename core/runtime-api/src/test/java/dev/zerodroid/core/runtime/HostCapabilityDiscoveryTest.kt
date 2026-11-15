package dev.zerodroid.core.runtime
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
class HostCapabilityDiscoveryTest {
 @Test fun probeFailuresBecomeExplicitErrorsAndDoNotSelectBackend():Unit=runBlocking{
  val d=HostCapabilityDiscovery(object:HostArchitectureProbe{override suspend fun probe()=HostArchitecture.AARCH64},object:AvfCapabilityProbe{override suspend fun probe():AvfHostEvidence=error("AVF unavailable")},object:KvmCapabilityProbe{override suspend fun probe():KvmHostEvidence=error("KVM denied")})
  val r=d.discover("microdroid");assertFalse(r.decision.supported);assertEquals(CapabilityFact.ERROR,r.evidence.avf.apiAvailable);assertEquals(CapabilityFact.ERROR,r.evidence.kvm.deviceApi);assertEquals(setOf("avf","kvm"),r.failures.map{it.source}.toSet())
 }
 @Test fun successfulEvidenceIsPassedToGateWithoutInventingFacts():Unit=runBlocking{
  val d=HostCapabilityDiscovery(object:HostArchitectureProbe{override suspend fun probe()=HostArchitecture.AARCH64},object:AvfCapabilityProbe{override suspend fun probe()=AvfHostEvidence(apiAvailable=CapabilityFact.AVAILABLE,serviceAvailable=CapabilityFact.AVAILABLE,appAuthorized=CapabilityFact.AVAILABLE,supportedVmTypes=setOf("microdroid"))},object:KvmCapabilityProbe{override suspend fun probe()=KvmHostEvidence()})
  val r=d.discover("microdroid");assertEquals(VirtualizationBackend.ANDROID_AVF,r.decision.selectedBackend);assertTrue(r.failures.isEmpty());assertEquals(CapabilityFact.UNKNOWN,r.evidence.kvm.deviceApi)
 }
 @Test(expected=IllegalArgumentException::class) fun blankVmTypeIsRejectedBeforeProbing():Unit=runBlocking{
  val a=object:HostArchitectureProbe{override suspend fun probe():HostArchitecture=error("must not run")}
  val v=object:AvfCapabilityProbe{override suspend fun probe():AvfHostEvidence=error("must not run")}
  val k=object:KvmCapabilityProbe{override suspend fun probe():KvmHostEvidence=error("must not run")}
  HostCapabilityDiscovery(a,v,k).discover(" ");Unit
 }
 @Test fun cancellationIsNotSwallowedAsProbeFailure():Unit=runBlocking{
  val d=HostCapabilityDiscovery(object:HostArchitectureProbe{override suspend fun probe():HostArchitecture=throw CancellationException("cancel")},object:AvfCapabilityProbe{override suspend fun probe()=AvfHostEvidence()},object:KvmCapabilityProbe{override suspend fun probe()=KvmHostEvidence()})
  try{d.discover("microdroid");throw AssertionError("CancellationException should propagate")}catch(_:CancellationException){};Unit
 }
}
