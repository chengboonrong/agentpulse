package io.github.chengboonrong.agentpulse.link

import com.google.crypto.tink.aead.internal.InsecureNonceChaCha20Poly1305
import com.google.crypto.tink.subtle.Hkdf
import com.google.crypto.tink.subtle.X25519
import java.io.Closeable
import java.io.DataInputStream
import java.io.OutputStream
import java.net.InetSocketAddress
import java.net.Socket
import java.nio.ByteBuffer
import java.security.MessageDigest
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec

// The direct connection (docs/mobile.md, "Direct connection"): link/'s Handshake.swift
// and LinkClient, over a blocking java.net.Socket.

/** The largest frame body (ciphertext and tag) after the 4-byte length. */
internal const val MAX_FRAME = 65_536
internal val MAGIC = "APL1".toByteArray()
/** Message 1's body: `"APL1" || ep (32) || tag (16)`. */
internal const val HELLO_LENGTH = 4 + 32 + 16
internal const val REPLY_LENGTH = 32

internal fun prefixed(body: ByteArray): ByteArray = ByteBuffer.allocate(4).putInt(body.size).array() + body

internal fun dh(privateKey: ByteArray, publicKey: ByteArray): ByteArray = try {
    X25519.computeSharedSecret(privateKey, publicKey)
} catch (_: Exception) {
    throw LinkException("handshake failed")
}

private fun hkdf(ikm: ByteArray, salt: ByteArray?, info: String): ByteArray =
    Hkdf.computeHkdf("HMACSHA256", ikm, salt, info.toByteArray(), 32)

/**
 * The first 16 bytes of HMAC-SHA256(k, ep), with
 * k = HKDF-SHA256(DH(sp, sm), info: "agentpulse/1 link id " + base64url(id)), no salt.
 */
internal fun idTag(ss: ByteArray, id: ByteArray, ep: ByteArray): ByteArray {
    val mac = Mac.getInstance("HmacSHA256")
    mac.init(SecretKeySpec(hkdf(ss, null, "agentpulse/1 link id ${Base64Url.encode(id)}"), "HmacSHA256"))
    return mac.doFinal(ep).copyOf(16)
}

/** `ikm` is `ee || es || se || ss`; the Mac sends with `down`, the phone with `up`. */
internal fun session(
    id: ByteArray, sm: ByteArray, sp: ByteArray, ep: ByteArray, em: ByteArray, ikm: ByteArray, isMac: Boolean,
): LinkSession {
    val h = MessageDigest.getInstance("SHA-256")
    for (part in listOf("agentpulse/1 link".toByteArray(), id, sm, sp, ep, em)) h.update(part)
    val salt = h.digest()
    val down = hkdf(ikm, salt, "down")
    val up = hkdf(ikm, salt, "up")
    return if (isMac) LinkSession(down, up) else LinkSession(up, down)
}

/** The phone's side of the handshake, for one connection: [finish] works once. */
class LinkClientHandshake internal constructor(
    private val id: ByteArray, private val phoneKey: KeyPair, private val macKey: ByteArray,
    private val ephemeral: KeyPair,
) {
    constructor(pairingId: ByteArray, phoneKey: KeyPair, macKey: ByteArray) :
        this(pairingId, phoneKey, macKey, KeyPair.generate())

    init {
        if (id.size != 16) throw LinkException("invalid pairing id")
    }

    /** Message 1, length prefix included: `"APL1" || ep || tag`. The id itself is never sent. */
    val message: ByteArray = prefixed(MAGIC + ephemeral.publicKey + idTag(dh(phoneKey.privateKey, macKey), id, ephemeral.publicKey))
    private var used = false

    /** Message 2 (length prefix included), the Mac's ephemeral key, to the session. */
    fun finish(reply: ByteArray): LinkSession {
        if (used || reply.size != 4 + REPLY_LENGTH || ByteBuffer.wrap(reply).int != REPLY_LENGTH) {
            throw LinkException("handshake failed")
        }
        used = true
        val em = reply.copyOfRange(4, reply.size)
        val e = ephemeral.privateKey
        val s = phoneKey.privateKey
        val ikm = dh(e, em) + dh(e, macKey) + dh(s, em) + dh(s, macKey)
        return session(id, macKey, phoneKey.publicKey, ephemeral.publicKey, em, ikm, isMac = false)
    }
}

/**
 * Seals and opens frames with strict per-direction counters; any error makes it unusable.
 * Frame: 4-byte big-endian length, then ChaCha20-Poly1305 under that direction's key, nonce
 * 4 zero bytes || the 8-byte big-endian counter, the length prefix as associated data.
 */
class LinkSession internal constructor(sendKey: ByteArray, receiveKey: ByteArray) {
    private val sendBox = InsecureNonceChaCha20Poly1305(sendKey)
    private val receiveBox = InsecureNonceChaCha20Poly1305(receiveKey)
    private var sent = 0L
    private var received = 0L
    private var failed = false

    /** The frame, length prefix included. */
    @Synchronized
    fun seal(plaintext: ByteArray): ByteArray = guarded {
        if (plaintext.size + 16 > MAX_FRAME) throw LinkException("frame too large")
        val header = ByteBuffer.allocate(4).putInt(plaintext.size + 16).array()
        val frame = header + sendBox.encrypt(nonce(sent), plaintext, header)
        sent++
        frame
    }

    /** Opens one frame, length prefix included. */
    @Synchronized
    fun open(frame: ByteArray): ByteArray = guarded {
        if (frame.size < 4 + 16 || ByteBuffer.wrap(frame).int != frame.size - 4) throw LinkException("frame failed")
        if (frame.size - 4 > MAX_FRAME) throw LinkException("frame too large")
        val header = frame.copyOf(4)
        val plaintext = try {
            receiveBox.decrypt(nonce(received), frame.copyOfRange(4, frame.size), header)
        } catch (_: Exception) {
            throw LinkException("frame failed")
        }
        received++
        plaintext
    }

    private fun nonce(counter: Long): ByteArray {
        // The counter never wraps: a session this long is closed instead.
        if (counter == Long.MAX_VALUE) throw LinkException("session failed")
        return ByteBuffer.allocate(12).putInt(0).putLong(counter).array()
    }

    private fun <T> guarded(body: () -> T): T {
        if (failed) throw LinkException("session failed")
        return try {
            body()
        } catch (e: Exception) {
            failed = true
            throw e
        }
    }
}

/**
 * The phone's side of one direct connection, link/'s `LinkClient`. Blocking: call it off
 * the main thread. [connect] runs the handshake; [receive] then returns the Mac's messages,
 * the latest state first, and throws once the connection is closed, a frame fails, or the
 * Mac has been silent for [idleTimeoutMs] (it sends a heartbeat every 30 s). Sending a
 * heartbeat is not the phone's job.
 */
class LinkClient(
    private val address: InetSocketAddress, pairingId: ByteArray, phoneKey: KeyPair, macKey: ByteArray,
    private val idleTimeoutMs: Int = 90_000,
) : Closeable {
    private val handshake = LinkClientHandshake(pairingId, phoneKey, macKey)
    private val socket = Socket()
    private lateinit var input: DataInputStream
    private lateinit var output: OutputStream
    @Volatile private var session: LinkSession? = null
    private var first: ByteArray? = null

    /**
     * Connects, runs the handshake, opens the Mac's first frame (which proves the Mac holds
     * the pairing's key) and answers with [firstMessage] (the `seen` message), all within
     * 5 s, so it lands inside the Mac's handshake window. Throws when the Mac doesn't know
     * the pairing (it closes without a reply) or doesn't answer in time. Call once.
     */
    fun connect(firstMessage: ByteArray) {
        val deadline = System.nanoTime() + CONNECT_TIMEOUT_MS * 1_000_000L
        fun left(): Int {
            val ms = (deadline - System.nanoTime()) / 1_000_000
            if (ms <= 0) throw LinkException("timed out")
            return ms.toInt()
        }
        try {
            socket.tcpNoDelay = true
            socket.connect(address, left())
            input = DataInputStream(socket.getInputStream())
            output = socket.getOutputStream()
            output.write(handshake.message)
            output.flush()
            socket.soTimeout = left()
            val s = handshake.finish(read(REPLY_LENGTH))
            socket.soTimeout = left()
            first = s.open(read(MAX_FRAME))
            left()
            session = s
            send(firstMessage)
            socket.soTimeout = idleTimeoutMs
        } catch (e: Exception) {
            close()
            throw e
        }
    }

    /** The Mac's next message; blocks. Throws once the connection is gone. */
    fun receive(): ByteArray {
        first?.let {
            first = null
            return it
        }
        val s = session ?: throw LinkException("closed")
        return try {
            s.open(read(MAX_FRAME))
        } catch (e: Exception) {
            close()
            throw e
        }
    }

    /** Sends one message as a frame. Throws if [connect] hasn't succeeded or the connection is closed. */
    fun send(message: ByteArray) {
        val s = session ?: throw LinkException("closed")
        try {
            synchronized(this) {
                output.write(s.seal(message))
                output.flush()
            }
        } catch (e: Exception) {
            close()
            throw e
        }
    }

    /** Closes for good; a blocked [receive] then throws. */
    override fun close() {
        session = null
        runCatching { socket.close() }
    }

    /** One length-prefixed message, prefix included; throws on EOF or a length over [max]. */
    private fun read(max: Int): ByteArray {
        val n = input.readInt()
        if (n <= 0 || n > max) throw LinkException("frame failed")
        return ByteBuffer.allocate(4 + n).putInt(n).also { input.readFully(it.array(), 4, n) }.array()
    }

    private companion object {
        const val CONNECT_TIMEOUT_MS = 5_000
    }
}
