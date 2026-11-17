package io.github.chengboonrong.agentpulse.link

import com.google.crypto.tink.InsecureSecretKeyAccess
import com.google.crypto.tink.hybrid.HpkeParameters
import com.google.crypto.tink.hybrid.HpkePrivateKey
import com.google.crypto.tink.hybrid.HpkePublicKey
import com.google.crypto.tink.hybrid.internal.HpkeContext
import com.google.crypto.tink.hybrid.internal.HpkeEncrypt
import com.google.crypto.tink.hybrid.internal.HpkeKemKeyFactory
import com.google.crypto.tink.hybrid.internal.HpkePrimitiveFactory
import com.google.crypto.tink.subtle.X25519
import com.google.crypto.tink.util.Bytes
import com.google.crypto.tink.util.SecretBytes
import java.security.MessageDigest
import kotlinx.serialization.EncodeDefault
import kotlinx.serialization.ExperimentalSerializationApi
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json

class LinkException(message: String) : Exception(message)

/** `DOWN` is Mac to phone, `UP` is phone to Mac. */
enum class Direction(val wire: String) { DOWN("down"), UP("up") }

/** A Curve25519 key pair as raw 32-byte keys, as they travel in links and JSON. */
class KeyPair(val privateKey: ByteArray) {
    val publicKey: ByteArray = X25519.publicFromPrivate(privateKey)

    companion object {
        fun generate() = KeyPair(X25519.generatePrivateKey())
    }
}

/**
 * HPKE single-shot sealing for relay messages, as link/'s `RelayMessage`
 * (docs/mobile.md, "Cryptography"): DHKEM X25519 HKDF-SHA256, HKDF-SHA256,
 * ChaCha20-Poly1305, no associated data, base64url (unpadded) of `enc || ciphertext`.
 */
object RelayMessage {
    private val params = HpkeParameters.builder()
        .setKemId(HpkeParameters.KemId.DHKEM_X25519_HKDF_SHA256)
        .setKdfId(HpkeParameters.KdfId.HKDF_SHA256)
        .setAeadId(HpkeParameters.AeadId.CHACHA20_POLY1305)
        .setVariant(HpkeParameters.Variant.NO_PREFIX)
        .build()
    private val kem = HpkePrimitiveFactory.createKem(params.kemId)
    private val kdf = HpkePrimitiveFactory.createKdf(params.kdfId)
    private val aead = HpkePrimitiveFactory.createAead(params.aeadId)
    private val noAssociatedData = ByteArray(0)

    fun info(direction: Direction, pairingId: ByteArray): ByteArray {
        if (pairingId.size != 16) throw LinkException("invalid pairing id")
        return "agentpulse/1 ${direction.wire} ${Base64Url.encode(pairingId)}".toByteArray()
    }

    /** Auth mode: sealed to `to`, authenticated by `from`'s private key. */
    fun seal(plaintext: ByteArray, to: ByteArray, from: KeyPair, direction: Direction, pairingId: ByteArray): String {
        val context = HpkeContext.createAuthSenderContext(
            publicKey(to), kem, kdf, aead, info(direction, pairingId), kemPrivateKey(from))
        return Base64Url.encode(context.encapsulatedKey + context.seal(plaintext, noAssociatedData))
    }

    /** Opens an auth-mode message from `from`. */
    fun open(message: String, with: KeyPair, from: ByteArray, direction: Direction, pairingId: ByteArray): ByteArray {
        val data = Base64Url.decode(message)
        // 32-byte enc plus at least the 16-byte tag.
        if (data == null || data.size < 48) throw LinkException("invalid message")
        val context = HpkeContext.createAuthRecipientContext(
            data.copyOfRange(0, 32), kemPrivateKey(with), kem, kdf, aead, info(direction, pairingId), publicKey(from))
        return context.open(data.copyOfRange(32, data.size), noAssociatedData)
    }

    /** The phone's hello: the only base-mode message, direction `up`. */
    fun sealHello(hello: Hello, macKey: ByteArray, pairingId: ByteArray): String {
        val plaintext = Json.encodeToString(Hello.serializer(), hello).toByteArray()
        return Base64Url.encode(HpkeEncrypt.create(publicKey(macKey)).encrypt(plaintext, info(Direction.UP, pairingId)))
    }

    internal fun publicKey(key: ByteArray): HpkePublicKey = HpkePublicKey.create(params, Bytes.copyFrom(key), null)

    internal fun privateKey(key: KeyPair): HpkePrivateKey =
        HpkePrivateKey.create(publicKey(key.publicKey), SecretBytes.copyFrom(key.privateKey, InsecureSecretKeyAccess.get()))

    private fun kemPrivateKey(key: KeyPair) = HpkeKemKeyFactory.createPrivate(privateKey(key))
}

/** The hello plaintext, link/'s `Hello`: byte fields as base64url, always `up` seq 0. */
@OptIn(ExperimentalSerializationApi::class)
@Serializable
class Hello private constructor(
    @EncodeDefault val seq: Long = 0,
    val secret: String,
    val phoneKey: String,
    val phoneName: String,
    val sentAtMs: Long,
) {
    companion object {
        fun of(secret: ByteArray, phoneKey: ByteArray, phoneName: String, sentAtMs: Long): Hello {
            if (secret.size != 32 || phoneKey.size != 32) throw LinkException("invalid message")
            val name = sanitizedName(phoneName) ?: throw LinkException("invalid name")
            return Hello(0, Base64Url.encode(secret), Base64Url.encode(phoneKey), name, sentAtMs)
        }
    }
}

/**
 * The six-digit code both screens show: the first 4 bytes, big-endian, of
 * SHA-256("agentpulse/1 code " + base64url(id) || Mac key || phone key), mod 1,000,000.
 */
fun verificationCode(pairingId: ByteArray, macKey: ByteArray, phoneKey: ByteArray): String {
    val sha = MessageDigest.getInstance("SHA-256")
    sha.update("agentpulse/1 code ${Base64Url.encode(pairingId)}".toByteArray())
    sha.update(macKey)
    sha.update(phoneKey)
    val h = sha.digest()
    val n = (h[0].toLong() and 0xFF shl 24) or (h[1].toLong() and 0xFF shl 16) or
        (h[2].toLong() and 0xFF shl 8) or (h[3].toLong() and 0xFF)
    return "%06d".format(n % 1_000_000)
}

/** base64url without padding (RFC 4648 §5); decoding is strict, as in link/. */
object Base64Url {
    private val alphabet = Regex("[A-Za-z0-9_-]*")

    fun encode(data: ByteArray): String = java.util.Base64.getUrlEncoder().withoutPadding().encodeToString(data)

    fun decode(s: String): ByteArray? {
        if (!alphabet.matches(s) || s.length % 4 == 1) return null
        val d = try {
            java.util.Base64.getUrlDecoder().decode(s)
        } catch (_: IllegalArgumentException) {
            return null
        }
        return if (encode(d) == s) d else null
    }
}
