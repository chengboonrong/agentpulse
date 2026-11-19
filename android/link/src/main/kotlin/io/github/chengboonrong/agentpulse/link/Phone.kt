package io.github.chengboonrong.agentpulse.link

import java.io.Closeable
import java.io.File
import java.io.FileOutputStream
import java.net.HttpURLConnection
import java.net.InetSocketAddress
import java.net.URI
import java.nio.file.Files
import java.nio.file.StandardCopyOption
import java.security.MessageDigest
import java.security.SecureRandom
import kotlinx.serialization.Serializable
import kotlinx.serialization.builtins.ListSerializer
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.longOrNull
import kotlinx.serialization.json.put

/** What the phone tells the user when the relay says no, link/'s `PhoneError`. */
class PhoneException(val reason: Reason, val status: Int = 0) : Exception(reason.text(status)) {
    enum class Reason {
        /** 401 or 404 for a pairing: the Mac removed it (or the relay dropped it). */
        REMOVED,
        /** 409 on the hello. */
        TAKEN,
        /** 401 or 404 on the hello: the code expired or was never valid. */
        EXPIRED,
        /** Still 429 after waiting. */
        BUSY,
        BAD_RESPONSE;

        fun text(status: Int) = when (this) {
            REMOVED -> "This Mac removed the pairing."
            TAKEN -> "Someone else answered this code."
            EXPIRED -> "This code expired. Show a new one on your Mac."
            BUSY -> "The relay is busy. Try again in a moment."
            BAD_RESPONSE -> "The relay answered HTTP $status."
        }
    }
}

/**
 * One paired Mac, as the phone keeps it. Its secrets (the phone's private key and relay
 * token) are not here but in the store's [SecretStore].
 */
@Serializable
data class Pairing(
    /** The pairing id, base64url. */
    val id: String,
    val relay: String,
    val macName: String,
    /** The Mac's raw public key, base64url. */
    val macKey: String,
    /** The latest accepted state's `payload` (widget.json version 1), as JSON text. */
    val lastState: String? = null,
    val lastStateAtMs: Long? = null,
    /** The `down` seq record: the last accepted state's seq. */
    val downSeq: Long? = null,
    /** The last `up` seq sent; the hello is 0. */
    val upSeq: Long = 0,
)

/** Where the phone's secrets live: under an Android Keystore key in the app, memory in tests. */
interface SecretStore {
    fun read(account: String): ByteArray?
    fun write(account: String, data: ByteArray)
    fun delete(account: String)
}

/**
 * The phone's pairings: `pairings.json` in [dir], shared by the app and its widgets so
 * none of them accepts a state older than one already shown; the secrets in [secrets].
 */
class PhoneStore(private val dir: File, private val secrets: SecretStore) {
    private val file = File(dir, "pairings.json")

    /** Throws when the file can't be read or decoded, so that shows instead of no Macs. */
    fun pairings(): List<Pairing> = locked { load() }

    fun pairing(id: String): Pairing? = pairings().firstOrNull { it.id == id }

    /** The phone's key pair and relay token for a pairing. */
    internal fun keys(id: String): Pair<KeyPair, ByteArray> {
        val d = secrets.read(id)?.takeIf { it.size == 64 } ?: throw PhoneException(PhoneException.Reason.REMOVED)
        return KeyPair(d.copyOfRange(0, 32)) to d.copyOfRange(32, 64)
    }

    internal fun add(pairing: Pairing, key: KeyPair, token: ByteArray) {
        secrets.write(pairing.id, key.privateKey + token)
        locked { save(load().filter { it.id != pairing.id } + pairing) }
    }

    /** Changes one pairing under the lock, writing only if it changed; null if it is gone. */
    internal fun update(id: String, change: (Pairing) -> Pairing): Pairing? = locked {
        val all = load()
        val i = all.indexOfFirst { it.id == id }
        if (i < 0) return@locked null
        val changed = change(all[i])
        if (changed != all[i]) save(all.toMutableList().also { it[i] = changed })
        changed
    }

    /** Deletes a pairing and its secrets. */
    fun delete(id: String) {
        locked { save(load().filter { it.id != id }) }
        secrets.delete(id)
    }

    private fun load(): List<Pairing> =
        if (file.exists()) json.decodeFromString(list, file.readText()) else emptyList()

    private fun save(pairings: List<Pairing>) {
        val tmp = File(dir, "pairings.json.tmp")
        // Synced before the rename, so a power loss can't leave an empty file behind it.
        FileOutputStream(tmp).use {
            it.write(json.encodeToString(list, pairings).toByteArray())
            it.fd.sync()
        }
        Files.move(tmp.toPath(), file.toPath(), StandardCopyOption.REPLACE_EXISTING, StandardCopyOption.ATOMIC_MOVE)
    }

    // ponytail: one lock per process; the app, its widgets and WorkManager share one process.
    // A file lock if any of them ever runs in another.
    private fun <T> locked(body: () -> T): T = synchronized(lock) {
        dir.mkdirs()
        body()
    }

    private companion object {
        val lock = Any()
        val list = ListSerializer(Pairing.serializer())
        val json = Json { ignoreUnknownKeys = true }
    }
}

/**
 * The phone's side of the link (docs/mobile.md), link/'s `PhoneClient`: pairing, `GET down`,
 * `POST up`, removing, and the direct connection. Blocking; call it off the main thread.
 */
class PhoneClient(val store: PhoneStore) {
    enum class Up(val wire: String) { SEEN("seen"), UNPAIR("unpair") }

    /** Says hello for a scanned or pasted link and stores the pairing. Returns the six-digit code the Mac shows too. */
    fun pair(link: String, phoneName: String): Pair<String, Pairing> {
        val l = PairingLink.parse(link)
        val key = KeyPair.generate()
        val token = ByteArray(32).also { SecureRandom().nextBytes(it) }
        val hello = Hello.of(l.secret, key.publicKey, phoneName, System.currentTimeMillis())
        val pairing = Pairing(Base64Url.encode(l.id), l.relay, l.macName, Base64Url.encode(l.macKey))
        val body = buildJsonObject {
            put("secret", Base64Url.encode(l.secret))
            put("phoneTokenHash", Base64Url.encode(sha256(token)))
            put("message", RelayMessage.sealHello(hello, l.macKey, l.id))
        }
        when (val status = request("POST", pairing, "hello", null, body).first) {
            204 -> {}
            409 -> throw PhoneException(PhoneException.Reason.TAKEN)
            401, 404 -> throw PhoneException(PhoneException.Reason.EXPIRED)
            else -> throw PhoneException(PhoneException.Reason.BAD_RESPONSE, status)
        }
        store.add(pairing, key, token)
        return verificationCode(l.id, l.macKey, key.publicKey) to pairing
    }

    /** `GET down`: stores a newer state and returns the pairing as stored (unchanged on 204, 304 or an old seq). */
    fun fetchDown(pairing: Pairing): Pairing {
        val current = store.pairing(pairing.id) ?: pairing
        val (key, token) = store.keys(pairing.id)
        val (status, body) = request("GET", pairing, "down", token, etag = current.downSeq?.let { "\"$it\"" })
        when (status) {
            200 -> {}
            204, 304 -> return current
            401, 404 -> throw PhoneException(PhoneException.Reason.REMOVED)
            else -> throw PhoneException(PhoneException.Reason.BAD_RESPONSE, status)
        }
        val sealed = runCatching { Json.parseToJsonElement(body).jsonObject["message"]?.jsonPrimitive?.content }
            .getOrNull() ?: throw PhoneException(PhoneException.Reason.BAD_RESPONSE, status)
        val plaintext = RelayMessage.open(sealed, key, macKey(pairing), Direction.DOWN, rawId(pairing))
        return take(plaintext, pairing.id) ?: store.pairing(pairing.id) ?: current
    }

    /** `POST up` through the relay. */
    fun post(type: Up, pairing: Pairing) {
        val (key, token) = store.keys(pairing.id)
        val sealed = RelayMessage.seal(nextUp(type, pairing.id), macKey(pairing), key, Direction.UP, rawId(pairing))
        when (val status = request("POST", pairing, "up", token, buildJsonObject { put("message", sealed) }).first) {
            204 -> {}
            401, 404 -> throw PhoneException(PhoneException.Reason.REMOVED)
            else -> throw PhoneException(PhoneException.Reason.BAD_RESPONSE, status)
        }
    }

    /**
     * Tells the Mac (`unpair`; the Mac then deletes the pairing at the relay) and deletes the
     * pairing and its secrets here, always. If the `unpair` can't be posted, deletes the
     * pairing at the relay itself as a best effort, so the Mac sees it gone.
     */
    fun remove(pairing: Pairing) {
        try {
            post(Up.UNPAIR, pairing)
        } catch (_: Exception) {
            runCatching { request("DELETE", pairing, null, store.keys(pairing.id).second) }
        } finally {
            runCatching { store.delete(pairing.id) }
        }
    }

    /**
     * Connects directly to a Mac found on the network, `seen` as the first frame. Throws when
     * that Mac doesn't know the pairing or doesn't answer within 5 s.
     */
    fun connect(pairing: Pairing, address: InetSocketAddress): DirectConnection {
        val link = LinkClient(address, rawId(pairing), store.keys(pairing.id).first, macKey(pairing))
        link.connect(nextUp(Up.SEEN, pairing.id))
        return DirectConnection(this, link, pairing.id)
    }

    /**
     * A `down` plaintext: a `state` newer than the last accepted one is stored; anything
     * else (an old seq, a heartbeat, an unknown type) changes nothing. Returns the pairing
     * as stored, null if the plaintext isn't a message.
     */
    internal fun take(plaintext: ByteArray, id: String): Pairing? {
        val m = runCatching { Json.parseToJsonElement(plaintext.decodeToString()).jsonObject }.getOrNull()
            ?: return null
        val seq = m["seq"]?.let { runCatching { it.jsonPrimitive.longOrNull }.getOrNull() } ?: return null
        val payload = m["payload"] as? JsonObject
        if (m["type"]?.let { runCatching { it.jsonPrimitive.content }.getOrNull() } != "state" || payload == null) {
            return null
        }
        return store.update(id) { p ->
            if (p.downSeq != null && seq <= p.downSeq) p
            else p.copy(downSeq = seq, lastState = payload.toString(), lastStateAtMs = System.currentTimeMillis())
        } ?: throw PhoneException(PhoneException.Reason.REMOVED)
    }

    /** The next `up` message, its seq taken from the shared record. */
    private fun nextUp(type: Up, id: String): ByteArray {
        val seq = store.update(id) { it.copy(upSeq = it.upSeq + 1) }?.upSeq
            ?: throw PhoneException(PhoneException.Reason.REMOVED)
        return buildJsonObject {
            put("seq", seq)
            put("type", type.wire)
            if (type == Up.SEEN) put("sentAtMs", System.currentTimeMillis())
        }.toString().toByteArray()
    }

    private fun rawId(p: Pairing) = Base64Url.decode(p.id) ?: throw LinkException("invalid pairing id")

    private fun macKey(p: Pairing) = Base64Url.decode(p.macKey) ?: throw LinkException("invalid Mac key")

    /** One relay request; waits out a 429 (Retry-After) up to five times. */
    private fun request(
        method: String, pairing: Pairing, route: String?, token: ByteArray?, body: JsonObject? = null,
        etag: String? = null,
    ): Pair<Int, String> {
        val url = URI(pairing.relay.trimEnd('/') + "/v1/pairings/${pairing.id}" + (route?.let { "/$it" } ?: "")).toURL()
        repeat(5) {
            val c = url.openConnection() as HttpURLConnection
            try {
                c.requestMethod = method
                c.connectTimeout = 20_000
                c.readTimeout = 20_000
                c.useCaches = false
                if (token != null) c.setRequestProperty("Authorization", "Bearer ${Base64Url.encode(token)}")
                if (etag != null) c.setRequestProperty("If-None-Match", etag)
                if (body != null) {
                    c.doOutput = true
                    c.setRequestProperty("Content-Type", "application/json")
                    c.outputStream.use { it.write(body.toString().toByteArray()) }
                }
                val status = c.responseCode
                if (status != 429) {
                    val text = (if (status < 400) c.inputStream else c.errorStream)?.use { it.readBytes().decodeToString() }
                    return status to (text ?: "")
                }
                val wait = c.getHeaderField("Retry-After")?.toDoubleOrNull() ?: 1.0
                Thread.sleep((wait.coerceIn(1.0, 10.0) * 1000).toLong())
            } finally {
                c.disconnect()
            }
        }
        throw PhoneException(PhoneException.Reason.BUSY)
    }

    private fun sha256(d: ByteArray) = MessageDigest.getInstance("SHA-256").digest(d)
}

/** A live direct connection to a paired Mac. Blocking, like [PhoneClient]. */
class DirectConnection internal constructor(
    private val client: PhoneClient, private val link: LinkClient, private val id: String,
) : Closeable {
    /**
     * Waits for the Mac's next message (its latest state comes first) and returns the
     * pairing as stored after it: a newer state is stored first; a heartbeat or an old state
     * changes nothing. Throws once the connection closes, a frame fails, or the Mac has been
     * silent for 90 s.
     */
    fun next(): Pairing {
        val message = link.receive()
        return client.take(message, id) ?: client.store.pairing(id) ?: throw PhoneException(PhoneException.Reason.REMOVED)
    }

    override fun close() = link.close()
}
