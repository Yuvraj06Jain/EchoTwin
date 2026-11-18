package dev.phonelink.app

import android.app.Activity
import android.app.KeyguardManager
import android.hardware.biometrics.BiometricManager
import android.hardware.biometrics.BiometricPrompt
import android.os.Build
import android.os.CancellationSignal

/**
 * "Unlock <computer>?": Android's own fingerprint / screen-lock prompt before the phone asks
 * the computer to unlock. Uses the platform prompt (no extra library); needs the
 * USE_BIOMETRIC permission, which Android grants without asking.
 */
object UnlockPrompt {
    fun confirm(activity: Activity, computer: String, onOk: () -> Unit, onProblem: (String) -> Unit) {
        val keyguard = activity.getSystemService(KeyguardManager::class.java)
        if (keyguard?.isDeviceSecure != true) {
            onProblem("Set a screen lock on this phone first: unlocking the computer needs it")
            return
        }
        val builder = BiometricPrompt.Builder(activity)
            .setTitle("Unlock $computer?")
            .setSubtitle("Confirm it's you")
        if (Build.VERSION.SDK_INT >= 30) {
            builder.setAllowedAuthenticators(
                BiometricManager.Authenticators.BIOMETRIC_STRONG or BiometricManager.Authenticators.DEVICE_CREDENTIAL
            )
        } else {
            @Suppress("DEPRECATION")
            builder.setDeviceCredentialAllowed(true)
        }
        builder.build().authenticate(
            CancellationSignal(),
            activity.mainExecutor,
            object : BiometricPrompt.AuthenticationCallback() {
                override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) = onOk()

                override fun onAuthenticationError(code: Int, message: CharSequence) {
                    // Cancelled by you: nothing to say.
                    if (code != BiometricPrompt.BIOMETRIC_ERROR_USER_CANCELED && code != BiometricPrompt.BIOMETRIC_ERROR_CANCELED) {
                        onProblem(message.toString())
                    }
                }
            },
        )
    }
}
