package dev.zerodroid.app.virtualization

import android.content.Context
import android.content.pm.PackageManager
import dev.zerodroid.core.runtime.AvfCapabilityProbe
import dev.zerodroid.core.runtime.AvfHostEvidence
import dev.zerodroid.core.runtime.CapabilityFact

/**
 * Collects only AVF facts that an ordinary Android SDK application can verify.
 *
 * The AVF Java manager is a @SystemApi and is intentionally not accessed through
 * hidden-API reflection. A system feature declaration is not proof that this app
 * can reach VirtualizationService or create a VM.
 */
class AndroidAvfCapabilityProbe(
    private val context: Context
) : AvfCapabilityProbe {

    override suspend fun probe(): AvfHostEvidence {
        val featureAvailable = try {
            context.packageManager.hasSystemFeature(FEATURE_VIRTUALIZATION_FRAMEWORK)
        } catch (_: SecurityException) {
            return AvfHostEvidence(
                apiAvailable = CapabilityFact.ERROR,
                serviceAvailable = CapabilityFact.UNKNOWN,
                appAuthorized = permissionEvidence()
            )
        } catch (_: RuntimeException) {
            return AvfHostEvidence(
                apiAvailable = CapabilityFact.ERROR,
                serviceAvailable = CapabilityFact.UNKNOWN,
                appAuthorized = permissionEvidence()
            )
        }

        val api = if (featureAvailable) {
            CapabilityFact.AVAILABLE
        } else {
            CapabilityFact.UNAVAILABLE
        }

        return AvfHostEvidence(
            apiAvailable = api,
            // The public SDK does not expose the optional AVF manager/service contract.
            // Keep this UNKNOWN until a supported privileged adapter proves it.
            serviceAvailable = if (featureAvailable) {
                CapabilityFact.UNKNOWN
            } else {
                CapabilityFact.UNAVAILABLE
            },
            appAuthorized = permissionEvidence(),
            // Protected/non-protected VM support requires VirtualMachineManager.getCapabilities().
            // Do not infer it from the generic system feature.
            supportedVmTypes = emptySet()
        )
    }

    private fun permissionEvidence(): CapabilityFact = try {
        if (context.checkSelfPermission(MANAGE_VIRTUAL_MACHINE_PERMISSION) ==
            PackageManager.PERMISSION_GRANTED
        ) {
            CapabilityFact.AVAILABLE
        } else {
            CapabilityFact.ACCESS_DENIED
        }
    } catch (_: SecurityException) {
        CapabilityFact.ACCESS_DENIED
    } catch (_: RuntimeException) {
        CapabilityFact.ERROR
    }

    private companion object {
        const val FEATURE_VIRTUALIZATION_FRAMEWORK =
            "android.software.virtualization_framework"
        const val MANAGE_VIRTUAL_MACHINE_PERMISSION =
            "android.permission.MANAGE_VIRTUAL_MACHINE"
    }
}
