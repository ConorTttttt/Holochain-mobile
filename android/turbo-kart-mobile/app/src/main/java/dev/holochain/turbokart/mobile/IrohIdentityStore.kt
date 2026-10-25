package dev.holochain.turbokart.mobile

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import java.security.KeyStore
import java.security.SecureRandom
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * Independent 32-byte Iroh Ed25519 seed encrypted at rest with a non-exportable
 * Android Keystore AES-256-GCM master key. It must NOT reuse the Lair unlock
 * secret, nor overwrite PassphraseStore's existing player identity.
 *
 * Missing or incomplete ciphertext, or an inaccessible existing master key,
 * fails closed rather than silently minting another previously issued identity.
 *
 * No backup/recovery is implemented; allowBackup=false in AndroidManifest.
 */
internal class IrohIdentityStore(context: Context) {
    private val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    private fun androidKeyStore(): KeyStore =
        KeyStore.getInstance("AndroidKeyStore").apply { load(null) }

    private fun masterKey(ciphertextExists: Boolean): SecretKey {
        val keyStore = androidKeyStore()
        val existing = keyStore.getKey(ALIAS, null) as? SecretKey
        if (existing != null) {
            check(ciphertextExists) {
                "Existing Iroh Keystore identity but encrypted seed is missing. " +
                    "Refusing to replace the endpoint identity."
            }
            return existing
        }
        check(!ciphertextExists) {
            "Iroh endpoint Keystore key missing; existing endpoint identity cannot be recovered."
        }
        val generator = KeyGenerator.getInstance(
            KeyProperties.KEY_ALGORITHM_AES,
            "AndroidKeyStore"
        )
        generator.init(
            KeyGenParameterSpec.Builder(
                ALIAS,
                KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT
            )
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .build()
        )
        return generator.generateKey()
    }

    fun getOrCreate(): ByteArray {
        val encrypted = prefs.getString(CIPHER, null)
        val nonce = prefs.getString(IV, null)
        check((encrypted == null) == (nonce == null)) {
            "Incomplete encrypted Iroh identity. Refusing to reset the endpoint."
        }

        val key = masterKey(encrypted != null)
        if (encrypted != null && nonce != null) {
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(
                Cipher.DECRYPT_MODE,
                key,
                GCMParameterSpec(128, Base64.decode(nonce, Base64.NO_WRAP))
            )
            val seed = cipher.doFinal(Base64.decode(encrypted, Base64.NO_WRAP))
            check(seed.size == 32) { "Invalid persisted Iroh endpoint seed length." }
            return seed
        }

        val seed = ByteArray(32).also { SecureRandom().nextBytes(it) }
        try {
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(Cipher.ENCRYPT_MODE, key)
            val ciphertext = cipher.doFinal(seed)
            check(
                prefs.edit()
                    .putString(CIPHER, Base64.encodeToString(ciphertext, Base64.NO_WRAP))
                    .putString(IV, Base64.encodeToString(cipher.iv, Base64.NO_WRAP))
                    .commit()
            ) { "Could not persist encrypted Iroh seed." }
            return seed
        } catch (error: Exception) {
            seed.fill(0)
            throw error
        }
    }

    companion object {
        private const val PREFS = "iroh_identity_secret_v1"
        private const val ALIAS = "turbo_kart_iroh_endpoint_v1"
        private const val CIPHER = "ciphertext"
        private const val IV = "nonce"
    }
}
