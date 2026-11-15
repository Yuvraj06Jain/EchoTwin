package dev.phonelink.app

import android.os.Build
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyPermanentlyInvalidatedException
import android.security.keystore.KeyProperties
import androidx.biometric.BiometricManager
import androidx.biometric.BiometricPrompt
import androidx.core.content.ContextCompat
import androidx.fragment.app.FragmentActivity
import java.security.KeyPairGenerator
import java.security.KeyStore
import java.security.MessageDigest
import java.security.PrivateKey
import java.security.Signature
import java.security.spec.ECGenParameterSpec

/**
 * The phone's key for approving `sudo` on the computer.
 *
 * An EC P-256 key in the Android Keystore that can only sign, and only right after a strong
 * biometric (fingerprint) check; it is destroyed when fingerprints are added or removed. The
 * private key never leaves the phone's secure hardware. The computer holds the public key in
 * a root-owned file and checks every signature itself.
 */
object Approver {
    private const val ALIAS = "echotwin_approve"

    private fun keyStore(): KeyStore = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }

    fun enrolled(): Boolean = runCatching { keyStore().containsAlias(ALIAS) }.getOrDefault(false)

    /** The public key (X.509 SubjectPublicKeyInfo, DER), if set up. */
    fun publicKey(): ByteArray? = runCatching { keyStore().getCertificate(ALIAS)?.publicKey?.encoded }.getOrNull()

    /** First 8 bytes of SHA-256(DER) as XXXX-XXXX-XXXX-XXXX, as the computer's setup shows it. */
    fun fingerprint(der: ByteArray): String =
        MessageDigest.getInstance("SHA-256").digest(der).take(8)
            .joinToString("") { "%02X".format(it) }
            .chunked(4).joinToString("-")

    /** A strong biometric is set up on this phone. */
    fun canUse(context: android.content.Context): Boolean =
        BiometricManager.from(context).canAuthenticate(BiometricManager.Authenticators.BIOMETRIC_STRONG) ==
            BiometricManager.BIOMETRIC_SUCCESS

    /** (Re)create the key. Returns its public key (DER). */
    fun enroll(): ByteArray {
        val ks = keyStore()
        if (ks.containsAlias(ALIAS)) ks.deleteEntry(ALIAS)
        val spec = KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_SIGN)
            .setAlgorithmParameterSpec(ECGenParameterSpec("secp256r1"))
            .setDigests(KeyProperties.DIGEST_SHA256)
            .setUserAuthenticationRequired(true)
            .setInvalidatedByBiometricEnrollment(true)
            .apply {
                if (Build.VERSION.SDK_INT >= 30) {
                    setUserAuthenticationParameters(0, KeyProperties.AUTH_BIOMETRIC_STRONG)
                } else {
                    @Suppress("DEPRECATION")
                    setUserAuthenticationValidityDurationSeconds(-1)
                }
            }
            .build()
        val kpg = KeyPairGenerator.getInstance(KeyProperties.KEY_ALGORITHM_EC, "AndroidKeyStore")
        kpg.initialize(spec)
        return kpg.generateKeyPair().public.encoded
    }

    fun remove() {
        runCatching { keyStore().deleteEntry(ALIAS) }
    }

    /** Ask for a fingerprint, then sign `message` (ECDSA P-256, SHA-256, DER). */
    fun sign(activity: FragmentActivity, computer: String, message: ByteArray, onResult: (Result<ByteArray>) -> Unit) {
        val signature = try {
            val key = keyStore().getKey(ALIAS, null) as? PrivateKey ?: error("Not set up on this phone")
            Signature.getInstance("SHA256withECDSA").apply { initSign(key) }
        } catch (e: KeyPermanentlyInvalidatedException) {
            remove()
            onResult(Result.failure(IllegalStateException("Fingerprints changed: set it up again in EchoTwin Settings")))
            return
        } catch (e: Exception) {
            onResult(Result.failure(e))
            return
        }
        val prompt = BiometricPrompt(
            activity,
            ContextCompat.getMainExecutor(activity),
            object : BiometricPrompt.AuthenticationCallback() {
                override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) {
                    val s = result.cryptoObject?.signature
                    onResult(
                        runCatching {
                            s ?: error("No signature")
                            s.update(message)
                            s.sign()
                        }
                    )
                }

                override fun onAuthenticationError(errorCode: Int, errString: CharSequence) {
                    onResult(Result.failure(IllegalStateException(errString.toString())))
                }
            },
        )
        val info = BiometricPrompt.PromptInfo.Builder()
            .setTitle("Approve admin access")
            .setSubtitle("sudo on $computer")
            .setNegativeButtonText("Cancel")
            .setAllowedAuthenticators(BiometricManager.Authenticators.BIOMETRIC_STRONG)
            .build()
        prompt.authenticate(info, BiometricPrompt.CryptoObject(signature))
    }
}
