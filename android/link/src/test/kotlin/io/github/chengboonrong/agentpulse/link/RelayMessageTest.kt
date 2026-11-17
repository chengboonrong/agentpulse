package io.github.chengboonrong.agentpulse.link

import java.security.SecureRandom
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFails
import kotlin.test.assertFailsWith
import kotlin.test.assertNull

class RelayMessageTest {
    private val mac = KeyPair.generate()
    private val phone = KeyPair.generate()
    private val id = ByteArray(16).also(SecureRandom()::nextBytes)
    private val text = """{"seq":1}""".toByteArray()

    @Test
    fun roundTripBothDirections() {
        val down = RelayMessage.seal(text, phone.publicKey, mac, Direction.DOWN, id)
        assertContentEquals(text, RelayMessage.open(down, phone, mac.publicKey, Direction.DOWN, id))
        val up = RelayMessage.seal(text, mac.publicKey, phone, Direction.UP, id)
        assertContentEquals(text, RelayMessage.open(up, mac, phone.publicKey, Direction.UP, id))
        assertEquals(32 + text.size + 16, Base64Url.decode(down)!!.size)
    }

    @Test
    fun wrongContextFails() {
        val down = RelayMessage.seal(text, phone.publicKey, mac, Direction.DOWN, id)
        assertFails { RelayMessage.open(down, phone, mac.publicKey, Direction.UP, id) }
        assertFails { RelayMessage.open(down, phone, mac.publicKey, Direction.DOWN, ByteArray(16)) }
        assertFails { RelayMessage.open(down, phone, KeyPair.generate().publicKey, Direction.DOWN, id) }
        assertFails { RelayMessage.open(down, KeyPair.generate(), mac.publicKey, Direction.DOWN, id) }
        assertFailsWith<LinkException> { RelayMessage.open("AAAA", phone, mac.publicKey, Direction.DOWN, id) }
        assertFailsWith<LinkException> { RelayMessage.seal(text, phone.publicKey, mac, Direction.DOWN, ByteArray(15)) }
    }

    @Test
    fun base64UrlIsStrict() {
        assertContentEquals(byteArrayOf(-5, -1), Base64Url.decode("-_8"))
        assertNull(Base64Url.decode("-_8=")) // padding
        assertNull(Base64Url.decode("+/8")) // standard alphabet
        assertNull(Base64Url.decode("-_9")) // not canonical
        assertNull(Base64Url.decode("A")) // impossible length
    }

    @Test
    fun helloNeedsASecretAndAName() {
        assertFailsWith<LinkException> { Hello.of(ByteArray(31), phone.publicKey, "Pixel", 0) }
        assertFailsWith<LinkException> { Hello.of(ByteArray(32), phone.publicKey, "  ", 0) }
    }
}
