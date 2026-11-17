package io.github.chengboonrong.agentpulse

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import io.github.chengboonrong.agentpulse.link.SecretStore
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * Each pairing's secrets (private key and relay token) sealed with AES-GCM under one
 * Android Keystore key that never leaves the device, the account name as associated
 * data so sealed values can't be swapped between pairings. Stored in private
 * SharedPreferences; the app, its widgets and WorkManager share the process.
 */
class KeystoreSecrets(context: Context) : SecretStore {
    private val prefs = context.getSharedPreferences("secrets", Context.MODE_PRIVATE)

    override fun read(account: String): ByteArray? {
        val sealed = Base64.decode(prefs.getString(account, null) ?: return null, Base64.NO_WRAP)
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, sealed, 0, IV))
        cipher.updateAAD(account.toByteArray())
        return cipher.doFinal(sealed, IV, sealed.size - IV)
    }

    override fun write(account: String, data: ByteArray) {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.ENCRYPT_MODE, key())
        cipher.updateAAD(account.toByteArray())
        val sealed = cipher.iv + cipher.doFinal(data)
        check(prefs.edit().putString(account, Base64.encodeToString(sealed, Base64.NO_WRAP)).commit()) {
            "couldn't save the pairing's keys"
        }
    }

    override fun delete(account: String) {
        prefs.edit().remove(account).commit()
    }

    private companion object {
        const val ALIAS = "agentpulse-secrets"
        const val TRANSFORMATION = "AES/GCM/NoPadding"
        const val IV = 12

        @Synchronized
        fun key(): SecretKey {
            val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
            (store.getKey(ALIAS, null) as SecretKey?)?.let { return it }
            val spec = KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .build()
            return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
                .apply { init(spec) }
                .generateKey()
        }
    }
}
