package io.github.chengboonrong.agentpulse.link

import com.sun.net.httpserver.HttpServer
import java.net.InetSocketAddress
import java.nio.file.Files
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNotNull
import kotlin.test.assertNull

class PhoneTest {
    private val id = Base64Url.encode(ByteArray(16) { it.toByte() })
    private val key = Base64Url.encode(ByteArray(32) { 1 })
    private val secret = Base64Url.encode(ByteArray(32) { 2 })

    private fun link(relay: String = "https://relay.example/", name: String = "Chris%E2%80%99s%20Mac", extra: String = "") =
        "agentpulse://pair?v=1&r=$relay&id=$id&k=$key&s=$secret&n=$name$extra"

    @Test
    fun parsesALink() {
        val l = PairingLink.parse(link())
        assertEquals("https://relay.example/", l.relay)
        assertEquals("Chris’s Mac", l.macName)
        assertEquals(16, l.id.size)
        // Percent-decoding only: `+` and an encoded `&` stay in the name.
        assertEquals("a+b&c", PairingLink.parse(link(name = "a+b%26c")).macName)
        assertEquals("ok", PairingLink.parse(link("http://localhost:8787", "ok")).macName)
    }

    @Test
    fun refusesABadLink() {
        fun bad(field: String, s: String) =
            assertEquals("invalid link: $field", assertFailsWith<LinkException> { PairingLink.parse(s) }.message)
        bad("url", "https://pair?v=1")
        bad("url", link().replace("//pair?", "//pair/?"))
        bad("r", link("http://relay.example"))
        bad("r", link("https://user@relay.example"))
        bad("r", link("https://relay.example/?q"))
        bad("v", link().replace("v=1", "v=2"))
        bad("id", link().replace("id=$id", "id=${id}AA"))
        bad("s", link(extra = "&s=$secret"))
        bad("n", link(name = "%E2%80%AE%0A%20"))
        bad("url", link(name = "%ZZ"))
    }

    @Test
    fun sanitisesNames() {
        assertEquals("evil", sanitizedName("‮ev​il\n"))
        assertNull(sanitizedName(" ‎\t "))
        assertEquals("x".repeat(64), sanitizedName("x".repeat(70)))
        // A base letter with its combining mark is kept or cut whole.
        assertEquals("x".repeat(63), sanitizedName("x".repeat(63) + "é"))
    }

    @Test
    fun keepsOnlyNewerStates() {
        val dir = Files.createTempDirectory("phone-test").toFile()
        try {
            val phone = PhoneClient(PhoneStore(dir, MemorySecrets()))
            val p = Pairing(id, "https://relay.example", "Mac", key)
            phone.store.add(p, KeyPair.generate(), ByteArray(32))
            fun state(seq: Int, n: Int) = """{"seq":$seq,"type":"state","payload":{"n":$n}}""".toByteArray()

            assertEquals(3L, phone.take(state(3, 1), id)?.downSeq)
            // Not newer: the stored pairing comes back unchanged.
            assertEquals("""{"n":1}""", phone.take(state(3, 2), id)?.lastState)
            assertEquals("""{"n":1}""", phone.take(state(2, 3), id)?.lastState)
            assertNull(phone.take("""{"seq":9,"type":"heartbeat"}""".toByteArray(), id))
            assertNull(phone.take("not json".toByteArray(), id))
            assertEquals("""{"n":4}""", phone.take(state(4, 4), id)?.lastState)
            // A new store over the same file sees the record.
            assertEquals(4L, PhoneStore(dir, MemorySecrets()).pairing(id)?.downSeq)

            phone.store.delete(id)
            assertFailsWith<PhoneException> { phone.take(state(5, 5), id) }
            assertNotNull(dir.listFiles()?.singleOrNull { it.name == "pairings.json" })
        } finally {
            dir.deleteRecursively()
        }
    }
}

class PhoneHttpTest {
    @Test
    fun retriesA429AndDeletesWhenTheUnpairFails() {
        val requests = mutableListOf<String>()
        val server = HttpServer.create(InetSocketAddress("localhost", 0), 0)
        server.createContext("/") { x ->
            val route = "${x.requestMethod} ${x.requestURI.path.substringAfter("/v1/pairings/").substringAfter("/", "")}"
            synchronized(requests) { requests += route }
            val status = when {
                // The first `up` is rate limited, the second refused, so `remove` falls back to DELETE.
                route == "POST up" && requests.count { it == route } == 1 -> 429.also {
                    x.responseHeaders.add("Retry-After", "1")
                }
                route == "POST up" -> 500
                else -> 204
            }
            x.sendResponseHeaders(status, -1)
            x.close()
        }
        server.start()
        val dir = Files.createTempDirectory("phone-http-test").toFile()
        try {
            val phone = PhoneClient(PhoneStore(dir, MemorySecrets()))
            val id = ByteArray(16) { 7 }
            val mac = KeyPair.generate()
            val link = "agentpulse://pair?v=1&r=http://localhost:${server.address.port}&id=${Base64Url.encode(id)}" +
                "&k=${Base64Url.encode(mac.publicKey)}&s=${Base64Url.encode(ByteArray(32))}&n=Mac"
            val (_, pairing) = phone.pair(link, "Phone")
            phone.remove(pairing)
            assertEquals(listOf("POST hello", "POST up", "POST up", "DELETE "), requests)
            assertEquals(emptyList(), phone.store.pairings())
        } finally {
            server.stop(0)
            dir.deleteRecursively()
        }
    }
}

/** Secrets in memory only. */
class MemorySecrets : SecretStore {
    private val items = mutableMapOf<String, ByteArray>()
    override fun read(account: String) = synchronized(items) { items[account] }
    override fun write(account: String, data: ByteArray) = synchronized(items) { items[account] = data }
    override fun delete(account: String) {
        synchronized(items) { items.remove(account) }
    }
}
