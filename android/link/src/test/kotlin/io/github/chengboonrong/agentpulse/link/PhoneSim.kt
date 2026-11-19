// The phone for scripts/e2e-android.sh, on the Android port (`./gradlew :link:phoneSim`), as
// link/'s phone-sim: reads the pairing link (`LINK <link>`) from stdin, says hello, prints
// `CODE <code>`, waits for the Mac's state, sends `seen` through the relay and as the first
// frame of a direct connection to an `_agentpulse._tcp` service (found with macOS's
// `dns-sd`: the JVM has no DNS-SD, the app uses Android's NSD), reads the Mac's first direct
// frame, then removes the pairing (`unpair`). Secrets stay in memory and the store in a temp
// directory. Exits 0, or 1 with the reason on stderr.
package io.github.chengboonrong.agentpulse.link

import java.net.InetSocketAddress
import java.nio.file.Files
import java.util.concurrent.TimeUnit
import kotlin.concurrent.thread
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

        // Directly. Every AgentPulse service is tried: a Mac that doesn't know the pairing
        // closes without a reply.
        var tried = 0
        var lastError: Exception? = null
        connected@ while (true) {
            check(System.currentTimeMillis() < deadline) {
                "direct: $tried attempts failed; last error: ${lastError ?: "no _agentpulse._tcp service found"}"
            }
            for (address in macs()) {
                tried++
                val c = try {
                    phone.connect(pairing, address)
                } catch (e: Exception) {
                    lastError = e
                    continue
                }
                c.use {
                    println("DIRECT state seq ${it.next().downSeq} from $address")
                    // Stays open a moment, as the app would, so the Mac shows it connected.
                    Thread.sleep(3000)
                }
                break@connected
            }
            Thread.sleep(1000)
        }

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

/** The `_agentpulse._tcp` services with TXT `v=1` on the network now. */
private fun macs(): List<InetSocketAddress> {
    val add = Regex("""\sAdd\s+\d+\s+\d+\s+\S+\s+_agentpulse\._tcp\.\s+(.+)$""")
    val names = dnsSd(2, "-B", "_agentpulse._tcp", "local.").mapNotNull { add.find(it)?.groupValues?.get(1) }.distinct()
    val at = Regex("""can be reached at (\S+):(\d+)""")
    return names.mapNotNull { name ->
        val lines = dnsSd(3, "-L", name, "_agentpulse._tcp", "local.") { it == "v=1" }
        val (host, port) = lines.firstNotNullOfOrNull { at.find(it)?.destructured } ?: return@mapNotNull null
        if ("v=1" !in lines) return@mapNotNull null
        InetSocketAddress(host.removeSuffix("."), port.toInt())
    }.distinct()
}

/** `dns-sd`'s lines, trimmed, for at most [seconds] or until [done]; through a pty, which it doesn't buffer. */
private fun dnsSd(seconds: Long, vararg args: String, done: (String) -> Boolean = { false }): List<String> {
    val p = ProcessBuilder("script", "-q", "/dev/null", "dns-sd", *args).redirectErrorStream(true).start()
    // dns-sd first: once `script` is gone, it is no longer among its descendants.
    fun stop() {
        p.descendants().forEach { it.destroy() }
        p.destroy()
    }
    val lines = mutableListOf<String>()
    val reader = thread {
        // Ends with "Stream closed" once stopped.
        runCatching {
            p.inputStream.bufferedReader().forEachLine {
                val l = it.trim()
                synchronized(lines) { lines += l }
                if (done(l)) stop()
            }
        }
    }
    p.waitFor(seconds, TimeUnit.SECONDS)
    stop()
    reader.join(1000)
    return synchronized(lines) { lines.toList() }
}
