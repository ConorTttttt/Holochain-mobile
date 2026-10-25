package dev.holochain.turbokart.mobile

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import java.io.File
import java.security.KeyStore
import java.security.SecureRandom
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * A persistent random Lair unlock secret, encrypted with an Android Keystore
 * AES-GCM key. Neither secret nor master key is written to source or assets.
 *
 * Prototype limitations: no cross-device recovery or account migration yet;
 * app data must not be cleared if the player wishes to retain their identity.
 * Backups are disabled because Android Keystore keys cannot be copied between
 * devices. Missing key/ciphertext fails closed instead of minting an identity.
 */
internal class PassphraseStore(private val context: Context) {
    private val prefs = context.getSharedPreferences("lair_secret_v1", Context.MODE_PRIVATE)

    private fun masterKey(ciphertextExists: Boolean): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        val previous = store.getKey(ALIAS, null) as? SecretKey
        if (previous != null) return previous
        check(!ciphertextExists) {
            "Android Keystore key is missing. Preserve the existing data; no new identity created."
        }
        val generator = KeyGenerator.getInstance(
            KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore"
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
        val existing = prefs.getString(CIPHER, null)
        val iv = prefs.getString(IV, null)
        check((existing == null) == (iv == null)) {
            "Incomplete encrypted Lair secret; refusing to reset identity."
        }
        val key = masterKey(existing != null)
        if (existing != null && iv != null) {
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(
                Cipher.DECRYPT_MODE,
                key,
                GCMParameterSpec(128, Base64.decode(iv, Base64.NO_WRAP))
            )
            return cipher.doFinal(Base64.decode(existing, Base64.NO_WRAP))
        }

        val conductor = File(context.filesDir, "conductor")
        check(!conductor.exists() || conductor.list()?.isEmpty() == true) {
            "Existing conductor state but no unlock secret; refusing to reset identity."
        }
        val secret = ByteArray(48).also { SecureRandom().nextBytes(it) }
        try {
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(Cipher.ENCRYPT_MODE, key)
            val ciphertext = cipher.doFinal(secret)
            check(
                prefs.edit()
                    .putString(CIPHER, Base64.encodeToString(ciphertext, Base64.NO_WRAP))
                    .putString(IV, Base64.encodeToString(cipher.iv, Base64.NO_WRAP))
                    .commit()
            ) { "Could not persist encrypted Lair secret." }
            return secret
        } catch (error: Exception) {
            secret.fill(0)
            throw error
        }
    }

    companion object {
        private const val ALIAS = "turbo_kart_lair_unlock_v1"
        private const val CIPHER = "ciphertext"
        private const val IV = "nonce"
    }
}
