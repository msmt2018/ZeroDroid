package dev.zerodroid.core.runtime

import android.content.Context
import android.content.pm.PackageManager

/**
 * Public-SDK AVF discovery only.
 *
 * The AVF manager Java API is a restricted @SystemApi and requires
 * MANAGE_VIRTUAL_MACHINE. This adapter deliberately does not use reflection or
 * hidden Binder APIs. Therefore service availability and supported VM types
 * remain UNKNOWN until a privileged/system integration adapter can query them.
 */
class AndroidAvfCapabilityProbe(
    private val context: Context
) : AvfCapabilityProbe {
    override suspend fun probe(): AvfHostEvidence {
        val featurePresent = context.packageManager.hasSystemFeature(
            FEATURE_VIRTUALIZATION_FRAMEWORK
        )
        val permissionGranted = context.checkSelfPermission(
            PERMISSION_MANAGE_VIRTUAL_MACHINE
        ) == PackageManager.PERMISSION_GRANTED

        return AvfHostEvidence(
            apiAvailable = if (featurePresent) {
                CapabilityFact.AVAILABLE
            } else {
                CapabilityFact.UNAVAILABLE
            },
            // A package feature declaration does not prove the Binder service is alive.
            serviceAvailable = CapabilityFact.UNKNOWN,
            appAuthorized = if (permissionGranted) {
                CapabilityFact.AVAILABLE
            } else {
                CapabilityFact.ACCESS_DENIED
            },
            // Public SDK cannot safely query protected/non-protected VM capabilities.
            supportedVmTypes = emptySet()
        )
    }

    private companion object {
        const val FEATURE_VIRTUALIZATION_FRAMEWORK =
            "android.software.virtualization_framework"
        const val PERMISSION_MANAGE_VIRTUAL_MACHINE =
            "android.permission.MANAGE_VIRTUAL_MACHINE"
    }
}
