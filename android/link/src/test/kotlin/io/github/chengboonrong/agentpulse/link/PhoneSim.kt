// The phone for scripts/e2e-android.sh, on the Android port (`./gradlew :link:phoneSim`), as
// link/'s phone-sim but through the relay only: reads the pairing link (`LINK <link>`) from
// stdin, says hello, prints `CODE <code>`, waits for the Mac's state, sends `seen`, waits for
// the next state (the Mac has read `seen` by then), then removes the pairing (`unpair`).
// Secrets stay in memory and the store in a temp directory. Exits 0, or 1 with the reason
// on stderr.
package io.github.chengboonrong.agentpulse.link

import java.nio.file.Files
import kotlin.system.exitProcess

fun main() {
    val dir = Files.createTempDirectory("phone-sim").toFile()
    try {
        val line = readlnOrNull() ?: error("no link on stdin")
        val phone = PhoneClient(PhoneStore(dir, MemorySecrets()))
        val deadline = System.currentTimeMillis() + 80_000

        val (code, paired) = phone.pair(line.removePrefix("LINK "), "E2E Android")
        println("CODE $code")

        // The Mac's state through the relay, once the user confirmed.
        var pairing = paired
        while (pairing.downSeq == null) {
            check(System.currentTimeMillis() < deadline) { "no state from the relay" }
            Thread.sleep(1000)
            pairing = phone.fetchDown(pairing)
        }
        println("RELAY state seq ${pairing.downSeq}")
        phone.post(PhoneClient.Up.SEEN, pairing)
        println("SEEN sent")

        // The Mac reads `up` when it publishes (its second state, 10 s after the first) or
        // every 30 s. Unpairing before that read would hand it `seen` and `unpair` at once,
        // and the pairing would be gone before the driver saw it seen.
        val first = pairing.downSeq!!
        while (pairing.downSeq == first) {
            check(System.currentTimeMillis() < deadline) { "no second state from the relay" }
            Thread.sleep(1000)
            pairing = phone.fetchDown(pairing)
        }
        println("RELAY state seq ${pairing.downSeq}")
        Thread.sleep(2000)

        phone.remove(pairing)
        check(phone.store.pairings().isEmpty()) { "the pairing is still stored after remove" }
        println("UNPAIRED")
    } catch (e: Exception) {
        System.err.println("phone-sim: $e")
        exitProcess(1)
    } finally {
        dir.deleteRecursively()
    }
}
