package io.github.chengboonrong.agentpulse.link

import com.google.crypto.tink.hybrid.internal.HpkeDecrypt
import java.io.File
import java.security.SecureRandom
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.long

/**
 * Messages sealed by one side, opened by the other: link/Tests/vectors/swift.json comes
 * from link/'s Swift tests (`AGENTPULSE_WRITE_VECTORS=1 swift test`), kotlin.json from here
 * (`./gradlew :link:test -PwriteVectors --rerun`). Both suites open both files.
 */
@Serializable
data class Vectors(
    val pairingId: String,
    val macPrivateKey: String,
    val phonePrivateKey: String,
    val secret: String,
    val phoneName: String,
    val sentAtMs: Long,
    val plaintext: String,
    val down: String,
    val up: String,
    val hello: String,
    val code: String,
)

class VectorTest {
    private val dir = File(System.getProperty("vectors"))
    private val json = Json { prettyPrint = true }

    @Test
    fun opensSwiftVectors() = check(read("swift.json"))

    @Test
    fun opensKotlinVectors() {
        if (System.getProperty("writeVectors") == "true") {
            File(dir, "kotlin.json").writeText(json.encodeToString(Vectors.serializer(), make()) + "\n")
        }
        check(read("kotlin.json"))
    }

    private fun read(name: String) = json.decodeFromString(Vectors.serializer(), File(dir, name).readText())

    private fun bytes(s: String) = Base64Url.decode(s)!!

    private fun check(v: Vectors) {
        val id = bytes(v.pairingId)
        val mac = KeyPair(bytes(v.macPrivateKey))
        val phone = KeyPair(bytes(v.phonePrivateKey))
        val text = v.plaintext.toByteArray()

        assertContentEquals(text, RelayMessage.open(v.down, phone, mac.publicKey, Direction.DOWN, id))
        assertContentEquals(text, RelayMessage.open(v.up, mac, phone.publicKey, Direction.UP, id))

        // The Mac's side of the hello, which the phone never opens.
        val opened = HpkeDecrypt.create(RelayMessage.privateKey(mac))
            .decrypt(bytes(v.hello), RelayMessage.info(Direction.UP, id))
        val hello = Json.parseToJsonElement(opened.decodeToString()).jsonObject
        assertEquals(0, hello.getValue("seq").jsonPrimitive.long)
        assertEquals(v.secret, hello.getValue("secret").jsonPrimitive.content)
        assertEquals(Base64Url.encode(phone.publicKey), hello.getValue("phoneKey").jsonPrimitive.content)
        assertEquals(v.phoneName, hello.getValue("phoneName").jsonPrimitive.content)
        assertEquals(v.sentAtMs, hello.getValue("sentAtMs").jsonPrimitive.long)

        assertEquals(v.code, verificationCode(id, mac.publicKey, phone.publicKey))
    }

    private fun make(): Vectors {
        val random = SecureRandom()
        val id = ByteArray(16).also(random::nextBytes)
        val secret = ByteArray(32).also(random::nextBytes)
        val mac = KeyPair.generate()
        val phone = KeyPair.generate()
        val text = """{"seq":1,"type":"seen","sentAtMs":1763000000000}"""
        val name = "Pixel Tablet"
        val sentAtMs = 1_763_000_000_000
        return Vectors(
            pairingId = Base64Url.encode(id),
            macPrivateKey = Base64Url.encode(mac.privateKey),
            phonePrivateKey = Base64Url.encode(phone.privateKey),
            secret = Base64Url.encode(secret),
            phoneName = name,
            sentAtMs = sentAtMs,
            plaintext = text,
            down = RelayMessage.seal(text.toByteArray(), phone.publicKey, mac, Direction.DOWN, id),
            up = RelayMessage.seal(text.toByteArray(), mac.publicKey, phone, Direction.UP, id),
            hello = RelayMessage.sealHello(Hello.of(secret, phone.publicKey, name, sentAtMs), mac.publicKey, id),
            code = verificationCode(id, mac.publicKey, phone.publicKey),
        )
    }
}
