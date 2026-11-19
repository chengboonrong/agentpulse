package io.github.chengboonrong.agentpulse.link

import java.io.DataInputStream
import java.io.OutputStream
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.ServerSocket
import java.nio.ByteBuffer
import java.nio.file.Files
import kotlin.concurrent.thread
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNull
import kotlin.test.assertTrue
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive

/** The direct connection against a Mac side written from link/'s `LinkServerHandshake`. */
class DirectTest {
    private val id = ByteArray(16) { it.toByte() }
    private val mac = KeyPair.generate()
    private val phone = KeyPair.generate()

    /** The Mac's reply and session for message 1, or null where the Mac closes without a reply. */
    private fun respond(message: ByteArray, pairingId: ByteArray = id): Pair<ByteArray, LinkSession>? {
        val body = message.copyOfRange(4, message.size)
        if (message.size != 4 + HELLO_LENGTH || !body.copyOf(4).contentEquals(MAGIC)) return null
        val ep = body.copyOfRange(4, 36)
        if (!idTag(dh(mac.privateKey, phone.publicKey), pairingId, ep).contentEquals(body.copyOfRange(36, 52))) return null
        val em = KeyPair.generate()
        val m = mac.privateKey
        val ikm = dh(em.privateKey, ep) + dh(m, ep) + dh(em.privateKey, phone.publicKey) + dh(m, phone.publicKey)
        return prefixed(em.publicKey) to session(pairingId, mac.publicKey, phone.publicKey, ep, em.publicKey, ikm, isMac = true)
    }

    @Test
    fun handshakeGivesEachDirectionItsOwnKey() {
        val client = LinkClientHandshake(id, phone, mac.publicKey)
        val (reply, macSession) = respond(client.message)!!
        val phoneSession = client.finish(reply)
        assertFailsWith<LinkException> { client.finish(reply) }

        val down = macSession.seal("state".toByteArray())
        assertContentEquals("state".toByteArray(), phoneSession.open(down))
        assertContentEquals("seen".toByteArray(), macSession.open(phoneSession.seal("seen".toByteArray())))
        // A frame reflected back to its sender, or replayed, doesn't open; the session is then done.
        val second = macSession.seal("again".toByteArray())
        assertFailsWith<LinkException> { macSession.open(second) }
        assertFailsWith<LinkException> { phoneSession.open(down) }
        assertFailsWith<LinkException> { phoneSession.open(second) }
    }

    @Test
    fun aTamperedFrameFails() {
        val client = LinkClientHandshake(id, phone, mac.publicKey)
        val (reply, macSession) = respond(client.message)!!
        val frame = macSession.seal("state".toByteArray())
        frame[frame.size - 1] = (frame[frame.size - 1].toInt() xor 1).toByte()
        assertFailsWith<LinkException> { client.finish(reply).open(frame) }
    }

    @Test
    fun theTagNamesThePairingWithoutSendingIt() {
        val message = LinkClientHandshake(id, phone, mac.publicKey).message
        assertEquals(4 + HELLO_LENGTH, message.size)
        assertNull(respond(message, ByteArray(16)))
        // Fresh ephemeral keys every time, so two connections can't be linked.
        assertTrue(!message.contentEquals(LinkClientHandshake(id, phone, mac.publicKey).message))
    }

    /** One connection to a Mac on loopback; [macSide] runs on the accepted socket. */
    private fun withMac(macSide: (DataInputStream, OutputStream) -> Unit, phoneSide: (InetSocketAddress) -> Unit) {
        ServerSocket(0, 1, InetAddress.getLoopbackAddress()).use { server ->
            var failure: Throwable? = null
            val t = thread {
                try {
                    server.accept().use { macSide(DataInputStream(it.getInputStream()), it.getOutputStream()) }
                } catch (e: Throwable) {
                    failure = e
                }
            }
            phoneSide(InetSocketAddress(server.inetAddress, server.localPort))
            t.join(10_000)
            failure?.let { throw it }
        }
    }

    private fun read(input: DataInputStream): ByteArray {
        val n = input.readInt()
        return ByteBuffer.allocate(4 + n).putInt(n).also { input.readFully(it.array(), 4, n) }.array()
    }

    @Test
    fun connectsStoresStatesAndSendsSeenFirst() {
        val dir = Files.createTempDirectory("direct-test").toFile()
        try {
            val store = PhoneStore(dir, MemorySecrets())
            val pairing = Pairing(Base64Url.encode(id), "https://relay.example", "Studio", Base64Url.encode(mac.publicKey))
            store.add(pairing, phone, ByteArray(32))
            val client = PhoneClient(store)
            var seen: String? = null
            withMac({ input, output ->
                val (reply, s) = respond(read(input))!!
                output.write(reply + s.seal("""{"seq":3,"type":"state","payload":{"version":1}}""".toByteArray()))
                seen = s.open(read(input)).decodeToString()
                output.write(s.seal("""{"seq":3,"type":"heartbeat","sentAtMs":1}""".toByteArray()))
                output.write(s.seal("""{"seq":2,"type":"state","payload":{"version":0}}""".toByteArray()))
                output.flush()
            }) { address ->
                client.connect(pairing, address).use { c ->
                    val first = c.next()
                    assertEquals(3L, first.downSeq)
                    assertEquals("""{"version":1}""", first.lastState)
                    // A heartbeat and an older state change nothing.
                    assertEquals(first, c.next())
                    assertEquals(first, c.next())
                    // The Mac closed.
                    assertFailsWith<Exception> { c.next() }
                }
            }
            val m = Json.parseToJsonElement(seen!!).jsonObject
            assertEquals("seen", m["type"]?.jsonPrimitive?.content)
            assertEquals("1", m["seq"]?.jsonPrimitive?.content)
            assertEquals(1L, store.pairing(pairing.id)?.upSeq)
        } finally {
            dir.deleteRecursively()
        }
    }

    @Test
    fun aMacWithoutThePairingClosesWithoutAReply() {
        withMac({ input, _ -> read(input) }) { address ->
            val c = LinkClient(address, ByteArray(16) { 7 }, phone, mac.publicKey)
            assertFailsWith<Exception> { c.connect("seen".toByteArray()) }
            assertFailsWith<LinkException> { c.send("x".toByteArray()) }
        }
    }

    @Test
    fun aSilentMacTimesOut() {
        // The handshake within 5 s.
        withMac({ input, _ ->
            read(input)
            Thread.sleep(6_000)
        }) { address ->
            val started = System.nanoTime()
            assertFailsWith<Exception> { LinkClient(address, id, phone, mac.publicKey).connect("seen".toByteArray()) }
            assertTrue((System.nanoTime() - started) / 1_000_000 in 4_500..7_000)
        }
        // Then frames within the idle timeout.
        withMac({ input, output ->
            val (reply, s) = respond(read(input))!!
            output.write(reply + s.seal("state".toByteArray()))
            output.flush()
            read(input)
            Thread.sleep(1_000)
        }) { address ->
            val c = LinkClient(address, id, phone, mac.publicKey, idleTimeoutMs = 200)
            c.connect("seen".toByteArray())
            assertContentEquals("state".toByteArray(), c.receive())
            assertFailsWith<Exception> { c.receive() }
        }
    }
}
